mod error;
mod linker;
mod loader;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use spin_factors::{
    ConfigureAppContext, Factor, InitContext, PrepareContext, RuntimeFactors,
    anyhow,
};
use tokio::sync::Mutex;

use crate::{linker::HostComponentInstancePre, loader::{LoadedHostComponent, instantiate_host_component}};

enum ComponentSource {
    Local { path: PathBuf },
}

struct SharedService<SD: 'static>(Arc<Mutex<HostComponentInstancePre<SD>>>);

impl<SD> SharedService<SD> {
    fn instance_pre(&self) -> spin_core::InstancePre<SD> {
        let tokio_rt = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| tokio_rt.block_on(async { self.0.lock().await.instance_pre.clone() }))
    }
}

/// A factor for providing variables to components.
#[derive(Default)]
pub struct HostComponentsFactor {
    component_sources: Vec<ComponentSource>,
    // engine: spin_core::wasmtime::Engine,
    host_components: Vec<LoadedHostComponent>,
    // interfaces: HashMap<String, SharedServiceKindOfThingButNotGeneric>,
    // interfaces: HashMap<String, LazyService>,
}

impl ComponentSource {
    fn read(&self) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Local { path } => Ok(std::fs::read(path)?)
        }
    }
}

impl std::fmt::Display for ComponentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentSource::Local { path } => path.display().fmt(f),
        }
    }
}

impl HostComponentsFactor {
    /// Creates a new `HostComponentsFactor`.
    pub fn new(sources: &[String]) -> Self {
        let component_sources = sources.iter().map(|s| ComponentSource::Local { path: PathBuf::from(s) }).collect();
        // let engine = hosting::create_host_engine().unwrap();
        Self { component_sources, host_components: Default::default(), /*interfaces: Default::default()*/ }
    }
}

impl Factor for HostComponentsFactor {
    type RuntimeConfig = ();
    type AppState = AppState;
    type InstanceBuilder = InstanceBuilder;

    fn init<T: InitContext<Self>>(&mut self, ctx: &mut T) -> anyhow::Result<()> {
        let engine = ctx.linker().engine().clone();

        // TODO: async or parallelise
        self.host_components = self.component_sources
            .iter()
            .map(|cs| loader::load_host_component(&engine, cs))
            .collect::<Result<_, _>>()?;

        let tokio_rt = tokio::runtime::Handle::current();

        for hc in &self.host_components {
            let instance_pre_fut = instantiate_host_component::<T>(engine.clone(), hc.clone(), None);  // TODO: data dir?
            let shared_instance_pre: SharedService<T::StoreData> = tokio::task::block_in_place(|| tokio_rt.block_on(instance_pre_fut))?;

            for interface in &hc.exported_interfaces {
                linker::link_interface(ctx, hc, &shared_instance_pre, interface);
            }
        }

        Ok(())
    }

    fn configure_app<T: RuntimeFactors>(
        &self,
        _ctx: ConfigureAppContext<T, Self>,
    ) -> anyhow::Result<Self::AppState> {
        Ok(AppState {
        })
    }

    fn prepare<T: RuntimeFactors>(
        &self,
        _ctx: PrepareContext<T, Self>,
    ) -> anyhow::Result<Self::InstanceBuilder> {
        let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
        wasi_builder.inherit_stderr();
        // TODO: perms

        Ok(InstanceBuilder { wasi_builder })
    }
}

pub struct AppState {
}

pub struct InstanceBuilder {
    wasi_builder: wasmtime_wasi::WasiCtxBuilder,
}

pub struct InstanceState {
    wasi: wasmtime_wasi::WasiCtx,
    table: wasmtime_wasi::ResourceTable,
    interface_map: HashMap<String, (spin_core::wasmtime::component::Instance, HashMap<String, spin_core::wasmtime::component::Func>)>,
}

impl spin_factors::FactorInstanceBuilder for InstanceBuilder {
    type InstanceState = InstanceState;

    fn build(mut self) -> anyhow::Result<Self::InstanceState> {
        Ok(Self::InstanceState {
            wasi: self.wasi_builder.build(),
            table: wasmtime_wasi::ResourceTable::with_capacity(100),
            interface_map: Default::default()
        })
    }
}

impl wasmtime_wasi::WasiView for InstanceState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl InstanceState {
    pub fn wasi(&mut self) -> &mut wasmtime_wasi::WasiCtx {
        &mut self.wasi
    }

    // Returning a clone seems vexing, but returning a reference runs
    // means the store remains borrowed while trying to call the func, which
    // makes the borrow checked mad
    pub(crate) fn get_handler(&mut self, interface: &str, func_name: &str) -> ExistingFuncMapping { // Option<spin_core::wasmtime::component::Func> {
        let Some((handler_intance, func_map)) = self.interface_map.get(interface) else {
            return ExistingFuncMapping::None;
        };
        match func_map.get(func_name) {
            Some(func) => ExistingFuncMapping::Func(func.clone()),
            None => ExistingFuncMapping::Instance(handler_intance.clone()),
        }
    }

    pub(crate) fn set_handler(&mut self, interface: &str, func_name: &str, instance: spin_core::wasmtime::component::Instance, func: spin_core::wasmtime::component::Func) {
        match self.interface_map.entry(interface.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut func_map) => {
                match func_map.get_mut().1.entry(func_name.to_string()) {
                    std::collections::hash_map::Entry::Occupied(_) => {},
                    std::collections::hash_map::Entry::Vacant(func_entry) => { func_entry.insert(func); }
                }
            },
            std::collections::hash_map::Entry::Vacant(interface_handler_entry) => {
                let mut map = HashMap::default();
                map.insert(func_name.to_string(), func);
                interface_handler_entry.insert((instance, map));
            },
        }
    }
}

pub(crate) enum ExistingFuncMapping {
    None,
    Instance(spin_core::wasmtime::component::Instance),
    Func(spin_core::wasmtime::component::Func),
}
