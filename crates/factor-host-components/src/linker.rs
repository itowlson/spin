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

    for func_name in &interface.functions {
        let interface_name = interface.name.clone();
        let func_name_clone = func_name.clone();
        let handler_clone = handler.clone();
        linker_instance.func_new_async(&func_name, move |_store_ctx, _func_type, params, results| {
            let interface_name = interface_name.clone();
            let func_name_clone = func_name_clone.clone();
            Box::new(forward_to_host_component(handler_clone.clone(), interface_name, func_name_clone, params, results))
        })
        .map_err(convert_error)
        .with_context(|| format!("failed to link function {}/{}", interface.name, func_name))?;
    }

    Ok(())
}

async fn forward_to_host_component(handler: SharedService, interface_name: String, func_name: String, params: &[Val], results: &mut [Val]) -> spin_core::wasmtime::Result<()> {
    handler.lock().await.call_func(&interface_name, &func_name, params, results).await
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
    async fn call_func(&mut self, interface_name: &str, func_name: &str, params: &[Val], results: &mut [Val]) -> wasmtime::Result<()> {
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

        func.call_async(&mut self.store, params, results).await?;

        Ok(())
    }
}

impl HostComponentStoreData {
    pub fn new(wasi: wasmtime_wasi::WasiCtx) -> Self {
        Self { wasi, table: spin_core::wasmtime::component::ResourceTable::new() }
    }
}
