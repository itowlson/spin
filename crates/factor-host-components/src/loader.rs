use std::{collections::HashMap, path::{Path}, sync::Arc};

use anyhow::{anyhow, Context};
use spin_core::wasmtime::{Engine, Store, component::{Component, Linker, types::{ComponentItem}}};
use tokio::sync::Mutex;
use wasmtime_wasi::{DirPerms, FilePerms};

use crate::{SharedService, linker::{HostComponentInstance, HostComponentStoreData}};

use super::error::convert_error;

/// Information about a loaded (but not yet instantiated) host component.
#[derive(Clone)]
pub struct LoadedHostComponent {
    pub name: String,
    pub component: Component,
    pub exported_interfaces: Vec<ExportedInterface>,
}

/// Metadata about a host component's exports, discovered from the Component type before instantiation.
#[derive(Clone)]
pub struct ExportedInterface {
    /// Fully qualified interface name (e.g., "example:cache/store@0.1.0")
    pub name: String,
    /// Function names within this interface and if they are async // TODO: express this more maintainably
    pub functions: Vec<(String, bool)>,
}

/// Load a host component from a Wasm file, inspecting its exports.
pub fn load_host_component(
    engine: &Engine,
    source: &crate::ComponentSource,
) -> anyhow::Result<LoadedHostComponent> {
    let bytes = source.read()
        .with_context(|| format!("failed to read host component from {source}"))?;
    load_host_component_from_bytes(engine, source, &bytes)
}

/// Load a host component from bytes, inspecting its exports.
fn load_host_component_from_bytes(
    engine: &Engine,
    source: &crate::ComponentSource,
    bytes: &[u8],
) -> anyhow::Result<LoadedHostComponent> {
    let component = Component::new(engine, bytes)
        .map_err(convert_error)
        .with_context(|| format!("failed to compile host component '{source}'"))?;

    let component_type = component.component_type();
    let mut exported_interfaces = Vec::new();

    for (export_name, item) in component_type.exports(engine) {
        if let ComponentItem::ComponentInstance(instance) = item.ty {
            let mut functions = Vec::new();
            for (func_name, func_item) in instance.exports(engine) {
                if let ComponentItem::ComponentFunc(_) = func_item.ty {
                    let func_name = func_name.to_string();
                    let is_async = {
                        // TODO: fewer crimes
                        let ComponentItem::ComponentFunc(fff) = func_item.ty else {
                            panic!();
                        };
                        fff.async_()
                    };
                    functions.push((func_name, is_async));
                }
            }
            if !functions.is_empty() {
                exported_interfaces.push(ExportedInterface {
                    name: export_name.to_string(),
                    functions,
                });
            }
        }
    }

    tracing::info!(
        "Loaded host component '{source}' with {} exported interface(s): [{}]",
        exported_interfaces.len(),
        exported_interfaces
            .iter()
            .map(|i| format!("{} ({} funcs)", i.name, i.functions.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok(LoadedHostComponent {
        name: source.to_string(),
        component,
        exported_interfaces,
    })
}

/// Instantiate a loaded host component into its own Store, returning a SharedService.
///
/// If `data_dir` is provided, the host component gets read-write filesystem
/// access to `<data_dir>/<component_name>/` so it can persist state (e.g., via sqlite).
pub async fn instantiate_host_component<SD: 'static>(
    engine: Engine,
    loaded: LoadedHostComponent,
    data_dir: Option<&Path>,
) -> anyhow::Result<SharedService<SD>> {
    let mut host_linker: Linker<HostComponentStoreData> = Linker::new(&engine);
    let mut host_linker2: Linker<SD> = Linker::new(&engine);

    wasmtime_wasi::p2::add_to_linker_async(&mut host_linker)
        .map_err(convert_error)
        .context("failed to add WASI P2 to host component linker")?;
    wasmtime_wasi::p3::add_to_linker(&mut host_linker).map_err(convert_error).context("failed to add WASI P3 to host component linker")?;
    // wasmtime_wasi::p2::add_to_linker_async(&mut host_linker2)
    //     .map_err(convert_error)
    //     .context("failed to add WASI P2 to host component linker")?;
    // wasmtime_wasi::p3::add_to_linker(&mut host_linker2).map_err(convert_error).context("failed to add WASI P3 to host component linker")?;

    let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
    wasi_builder.inherit_stderr();

    // If a data directory is provided, give the host component read-write
    // filesystem access to its own subdirectory.
    if let Some(base_dir) = data_dir {
        let component_dir = base_dir.join(&loaded.name);
        tokio::fs::create_dir_all(&component_dir).await.with_context(|| {
            format!(
                "failed to create data directory for host component '{}' at {}",
                loaded.name,
                component_dir.display()
            )
        })?;
        wasi_builder
            .preopened_dir(&component_dir, "/data", DirPerms::all(), FilePerms::all())
            .map_err(|e| anyhow::anyhow!(e))
            .with_context(|| {
                format!(
                    "failed to preopen data directory {} for host component '{}'",
                    component_dir.display(),
                    loaded.name
                )
            })?;

        tracing::info!(
            "Host component '{}' has read-write access to {} (mounted at /data)",
            loaded.name,
            component_dir.display()
        );
    }

    let instance_pre = host_linker2.instantiate_pre(&loaded.component).unwrap();

    let wasi = wasi_builder.build();

    let store_data = HostComponentStoreData::new(wasi);

    let mut store = Store::new(&engine, store_data);

    let instance = host_linker
        .instantiate_async(&mut store, &loaded.component)
        .await
        .map_err(convert_error)
        .with_context(|| {
            format!("failed to instantiate host component '{}'", loaded.name)
        })?;

    // Build export index cache for fast lookup
    let mut export_indices = HashMap::new();
    for iface in &loaded.exported_interfaces {
        let iface_index = instance
            .get_export_index(&mut store, None, &iface.name)
            .ok_or_else(|| {
                anyhow!(
                    "host component '{}' missing expected export '{}'",
                    loaded.name,
                    iface.name
                )
            })?;

        let mut func_indices = HashMap::new();
        for (func_name, _) in &iface.functions {
            let func_index = instance
                .get_export_index(&mut store, Some(&iface_index), func_name)
                .ok_or_else(|| {
                    anyhow!(
                        "host component '{}' interface '{}' missing function '{}'",
                        loaded.name,
                        iface.name,
                        func_name
                    )
                })?;
            func_indices.insert(func_name.clone(), func_index);
        }

        export_indices.insert(iface.name.clone(), (iface_index, func_indices));
    }

    let service = HostComponentInstance {
        store,
        instance,
        instance_pre,
        export_indices,
    };

    Ok(Arc::new(Mutex::new(service)))
}
