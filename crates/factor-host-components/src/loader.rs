use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{Context};
use spin_core::wasmtime::{Engine, component::{Component, Linker, types::{ComponentItem}}};
use tokio::sync::Mutex;

use crate::{SharedInstancePre, linker::{HostComponentInstancePre}};

use super::error::convert_error;

/// Information about a loaded (but not yet instantiated) host component.
#[derive(Clone)]
pub struct HostComponent {
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
) -> anyhow::Result<HostComponent> {
    let bytes = source.read()
        .with_context(|| format!("failed to read host component from {source}"))?;
    load_host_component_from_bytes(engine, source, &bytes)
}

/// Load a host component from bytes, inspecting its exports.
fn load_host_component_from_bytes(
    engine: &Engine,
    source: &crate::ComponentSource,
    bytes: &[u8],
) -> anyhow::Result<HostComponent> {
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

    Ok(HostComponent {
        name: source.to_string(),
        component,
        exported_interfaces,
    })
}

/// Instantiate a loaded host component into its own Store, returning a SharedService.
///
/// If `data_dir` is provided, the host component gets read-write filesystem
/// access to `<data_dir>/<component_name>/` so it can persist state (e.g., via sqlite).
pub async fn instantiate_host_component<T: spin_factors::InitContext<crate::HostComponentsFactor>>(
    engine: &Engine,
    host_component: HostComponent,
    data_dir: Option<&Path>,
) -> anyhow::Result<SharedInstancePre<T::StoreData>> {
    let mut host_linker: Linker<T::StoreData> = Linker::new(engine);

    link_wasi_p2::<T>(&mut host_linker)?;
    link_wasi_p3::<T>(&mut host_linker)?;

    let instance_pre = host_linker.instantiate_pre(&host_component.component).unwrap();
    let exports = get_export_indices(&host_component, &instance_pre).unwrap();

    let service = HostComponentInstancePre {
        instance_pre,
        exports,
    };

    Ok(crate::SharedInstancePre(Arc::new(Mutex::new(service))))
}

fn link_wasi_p2<T: spin_factors::InitContext<crate::HostComponentsFactor>>(host_linker: &mut Linker<T::StoreData>) -> spin_core::wasmtime::Result<()> {
    use wasmtime_wasi::p2::bindings;

    bindings::cli::environment::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::exit::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::stderr::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::stdin::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::stdout::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_input::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_output::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_stderr::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_stdin::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_stdout::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;

    bindings::clocks::monotonic_clock::add_to_linker::<T::StoreData, wasmtime_wasi::clocks::WasiClocks>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::clocks::WasiClocksCtxView { ctx: inst_st.wasi().clocks(), table }
    })?;
    bindings::clocks::wall_clock::add_to_linker::<T::StoreData, wasmtime_wasi::clocks::WasiClocks>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::clocks::WasiClocksCtxView { ctx: inst_st.wasi().clocks(), table }
    })?;

    bindings::filesystem::preopens::add_to_linker::<T::StoreData, wasmtime_wasi::filesystem::WasiFilesystem>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::filesystem::WasiFilesystemCtxView { ctx: inst_st.wasi().filesystem(), table }
    })?;
    bindings::filesystem::types::add_to_linker::<T::StoreData, wasmtime_wasi::filesystem::WasiFilesystem>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::filesystem::WasiFilesystemCtxView { ctx: inst_st.wasi().filesystem(), table }
    })?;

    bindings::io::error::add_to_linker::<T::StoreData, HasIo>(host_linker, |sd| {
        let (_inst_st, table) = T::get_data_with_table(sd);
        table
    })?;
    bindings::io::poll::add_to_linker::<T::StoreData, HasIo>(host_linker, |sd| {
        let (_inst_st, table) = T::get_data_with_table(sd);
        table
    })?;
    bindings::io::streams::add_to_linker::<T::StoreData, HasIo>(host_linker, |sd| {
        let (_inst_st, table) = T::get_data_with_table(sd);
        table
    })?;

    bindings::random::insecure::add_to_linker::<T::StoreData, wasmtime_wasi::random::WasiRandom>(host_linker, |sd| {
        let (inst_st, _table) = T::get_data_with_table(sd);
        inst_st.wasi().random()
    })?;
    bindings::random::insecure_seed::add_to_linker::<T::StoreData, wasmtime_wasi::random::WasiRandom>(host_linker, |sd| {
        let (inst_st, _table) = T::get_data_with_table(sd);
        inst_st.wasi().random()
    })?;
    bindings::random::random::add_to_linker::<T::StoreData, wasmtime_wasi::random::WasiRandom>(host_linker, |sd| {
        let (inst_st, _table) = T::get_data_with_table(sd);
        inst_st.wasi().random()
    })?;

    bindings::sockets::instance_network::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::ip_name_lookup::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::network::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, &bindings::sockets::network::LinkOptions::default(), |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::tcp::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::tcp_create_socket::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::udp::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::udp_create_socket::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;

    Ok(())
}

fn link_wasi_p3<T: spin_factors::InitContext<crate::HostComponentsFactor>>(host_linker: &mut Linker<T::StoreData>) -> spin_core::wasmtime::Result<()> {
    use wasmtime_wasi::p3::bindings;

    bindings::cli::environment::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::exit::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::stderr::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::stdin::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::stdout::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_input::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_output::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_stderr::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_stdin::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;
    bindings::cli::terminal_stdout::add_to_linker::<T::StoreData, wasmtime_wasi::cli::WasiCli>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::cli::WasiCliCtxView { ctx: inst_st.wasi().cli(), table }
    })?;

    bindings::clocks::monotonic_clock::add_to_linker::<T::StoreData, wasmtime_wasi::clocks::WasiClocks>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::clocks::WasiClocksCtxView { ctx: inst_st.wasi().clocks(), table }
    })?;
    bindings::clocks::system_clock::add_to_linker::<T::StoreData, wasmtime_wasi::clocks::WasiClocks>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::clocks::WasiClocksCtxView { ctx: inst_st.wasi().clocks(), table }
    })?;
    // bindings::clocks::timezone::add_to_linker::<T::StoreData, wasmtime_wasi::clocks::WasiClocks>(host_linker, &bindings::LinkOptions::default(), |sd| {
    //     let (inst_st, table) = T::get_data_with_table(sd);
    //     wasmtime_wasi::clocks::WasiClocksCtxView { ctx: inst_st.wasi().clocks(), table }
    // })?;
    bindings::clocks::types::add_to_linker::<T::StoreData, wasmtime_wasi::clocks::WasiClocks>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::clocks::WasiClocksCtxView { ctx: inst_st.wasi().clocks(), table }
    })?;

    bindings::filesystem::preopens::add_to_linker::<T::StoreData, wasmtime_wasi::filesystem::WasiFilesystem>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::filesystem::WasiFilesystemCtxView { ctx: inst_st.wasi().filesystem(), table }
    })?;
    bindings::filesystem::types::add_to_linker::<T::StoreData, wasmtime_wasi::filesystem::WasiFilesystem>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::filesystem::WasiFilesystemCtxView { ctx: inst_st.wasi().filesystem(), table }
    })?;

    bindings::random::insecure::add_to_linker::<T::StoreData, wasmtime_wasi::random::WasiRandom>(host_linker, |sd| {
        let (inst_st, _table) = T::get_data_with_table(sd);
        inst_st.wasi().random()
    })?;
    bindings::random::insecure_seed::add_to_linker::<T::StoreData, wasmtime_wasi::random::WasiRandom>(host_linker, |sd| {
        let (inst_st, _table) = T::get_data_with_table(sd);
        inst_st.wasi().random()
    })?;
    bindings::random::random::add_to_linker::<T::StoreData, wasmtime_wasi::random::WasiRandom>(host_linker, |sd| {
        let (inst_st, _table) = T::get_data_with_table(sd);
        inst_st.wasi().random()
    })?;

    bindings::sockets::ip_name_lookup::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;
    bindings::sockets::types::add_to_linker::<T::StoreData, wasmtime_wasi::sockets::WasiSockets>(host_linker, |sd| {
        let (inst_st, table) = T::get_data_with_table(sd);
        wasmtime_wasi::sockets::WasiSocketsCtxView { ctx: inst_st.wasi().sockets(), table }
    })?;

    Ok(())
}

fn get_export_indices<SD>(host_component: &HostComponent, instance: &spin_core::InstancePre<SD>) -> anyhow::Result<HashMap<String, (spin_core::wasmtime::component::ComponentExportIndex, HashMap<String, spin_core::wasmtime::component::ComponentExportIndex>)>> {
    use anyhow::anyhow;

    let mut export_indices = HashMap::new();

    for iface in &host_component.exported_interfaces {
        let iface_index = instance
            .component()
            .get_export_index(None, &iface.name)
            .ok_or_else(|| {
                anyhow!(
                    "host component '{}' missing expected export '{}'",
                    host_component.name,
                    iface.name
                )
            })?;

        let mut func_indices = HashMap::new();
        for (func_name, _) in &iface.functions {
            let func_index = instance
                .component()
                .get_export_index(Some(&iface_index), func_name)
                .ok_or_else(|| {
                    anyhow!(
                        "host component '{}' interface '{}' missing function '{}'",
                        host_component.name,
                        iface.name,
                        func_name
                    )
                })?;
            func_indices.insert(func_name.clone(), func_index);
        }

        export_indices.insert(iface.name.clone(), (iface_index, func_indices));
    }

    Ok(export_indices)
}

struct HasIo;

impl spin_core::wasmtime::component::HasData for HasIo {
    type Data<'a> = &'a mut wasmtime_wasi::ResourceTable;
}
