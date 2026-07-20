use std::{collections::HashMap};

use anyhow::{Context};
use spin_core::{Instance, wasmtime::{Store, component::{ComponentExportIndex, Linker, Val}}};
use spin_core::wasmtime;

use crate::{SharedService, error::convert_error, loader::ExportedInterface};

pub fn link_bindings<T: Send>(linker: &mut Linker<T>, interface: &ExportedInterface, handler: SharedService) -> anyhow::Result<()> { //interface: &ExportedInterface) -> anyhow::Result<()> {
    let mut linker_instance = linker
        .instance(&interface.name)
        .map_err(convert_error)
        .with_context(|| {
            format!(
                "failed to create linker instance for host component interface '{}'",
                interface.name
            )
        })?;

    for (func_name, is_async) in &interface.functions {
        let interface_name = interface.name.clone();
        let func_name_clone = func_name.clone();
        let handler_clone = handler.clone();

        if *is_async {
            linker_instance.func_new_concurrent(&func_name, move |accessor, _func, params, results| {
                let interface_name = interface_name.clone();
                let func_name_clone = func_name_clone.clone();
                let fut = forward_to_host_component_concurrent(handler_clone.clone(), accessor, interface_name, func_name_clone, params, results);
                Box::pin(fut)
            })
            .map_err(convert_error)
            .with_context(|| format!("failed to link function {}/{}", interface.name, func_name))?;
        } else {
            linker_instance.func_new_async(&func_name, move |_store_ctx, _func_type, params, results| {
                let interface_name = interface_name.clone();
                let func_name_clone = func_name_clone.clone();
                Box::new(forward_to_host_component(handler_clone.clone(), interface_name, func_name_clone, params, results))
            })
            .map_err(convert_error)
            .with_context(|| format!("failed to link function {}/{}", interface.name, func_name))?;
        }
    }

    Ok(())
}

async fn forward_to_host_component(handler: SharedService, interface_name: String, func_name: String, params: &[Val], results: &mut [Val]) -> spin_core::wasmtime::Result<()> {
    handler.lock().await.call_func(&interface_name, &func_name, params, results).await
        .map_err(|e| spin_core::wasmtime::Error::msg(e.to_string()))
}

async fn forward_to_host_component_concurrent<T: Send>(handler: SharedService, _accessor: &wasmtime::component::Accessor<T>, interface_name: String, func_name: String, params: &[Val], results: &mut [Val]) -> spin_core::wasmtime::Result<()> {
    handler.lock().await.call_func_concurrent(/*accessor,*/ &interface_name, &func_name, params, results).await
        .map_err(|e| spin_core::wasmtime::Error::msg(e.to_string()))
}

/// A running host component service with its own Store and Instance.
///
/// The Instance is long-lived and shared across all guest requests,
/// so it can maintain state (e.g., an in-memory cache).
pub struct HostComponentInstance {
    pub store: Store<HostComponentStoreData>,
    pub instance: Instance,
    /// Cached export indices: interface_name -> (interface_index, {func_name -> func_index})
    pub export_indices: HashMap<String, (ComponentExportIndex, HashMap<String, ComponentExportIndex>)>,
}

/// State held in the host component's dedicated Store.
pub struct HostComponentStoreData {
    wasi: wasmtime_wasi::WasiCtx,
    table: spin_core::wasmtime::component::ResourceTable,
}

impl wasmtime_wasi::WasiView for HostComponentStoreData {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl HostComponentInstance {
    fn get_func(&mut self, interface_name: &str, func_name: &str) -> Result<wasmtime::component::Func, wasmtime::Error> {
        let (_, func_indices) = self
            .export_indices
            .get(interface_name)
            .ok_or_else(|| wasmtime::Error::msg(format!("host component does not export interface '{interface_name}'")))?;
        let func_index = func_indices
            .get(func_name)
            .ok_or_else(|| {
                wasmtime::Error::msg(format!(
                    "interface '{interface_name}' does not export function '{func_name}'"
                ))
            })?;
        let func = self
            .instance
            .get_func(&mut self.store, func_index)
            .ok_or_else(|| {
                wasmtime::Error::msg(format!("failed to get function '{func_name}' from interface '{interface_name}'"))
            })?;
        Ok(func)
    }
    
    pub async fn call_func(&mut self, interface_name: &str, func_name: &str, params: &[Val], results: &mut [Val]) -> wasmtime::Result<()> {
        let func = self.get_func(interface_name, func_name)?;
        func.call_async(&mut self.store, params, results).await?;
        Ok(())
    }

    pub async fn call_func_concurrent(&mut self, /*accessor: &wasmtime::component::Accessor<T>,*/ interface_name: &str, func_name: &str, params: &[Val], results: &mut [Val]) -> wasmtime::Result<()> {
        let func = self.get_func(interface_name, func_name)?;

        // Gives: Recursive `StoreContextMut::run_concurrent` calls not supported
        let res = self.store.run_concurrent(async |accessor| {
           func.call_concurrent(accessor, params, results).await 
        }).await;
        res??;

        // Gives: object used with the wrong store
        // func.call_concurrent(accessor, params, results).await?;

        Ok(())
    }
}

impl HostComponentStoreData {
    pub fn new(wasi: wasmtime_wasi::WasiCtx) -> Self {
        Self { wasi, table: spin_core::wasmtime::component::ResourceTable::new() }
    }
}
