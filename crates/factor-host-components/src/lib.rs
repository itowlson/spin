mod error;
mod hosting;
mod linker;
mod loader;

use std::{path::PathBuf, sync::{Arc}};

use spin_factors::{
    ConfigureAppContext, Factor, InitContext, PrepareContext, RuntimeFactors,
    SelfInstanceBuilder, anyhow,
};
use tokio::sync::Mutex;

use crate::{linker::HostComponentInstance, loader::{LoadedHostComponent, instantiate_host_component}};

const TEST_TEST_TEST_PATHS: &[&str] = &["/home/ivan/testing/wondercomp/target/wasm32-wasip2/release/wondercomp.wasm"];

type ComponentSource = PathBuf;

type SharedService = Arc<Mutex<HostComponentInstance>>;

/// A factor for providing variables to components.
#[derive(Default)]
pub struct HostComponentsFactor {
    component_sources: Vec<ComponentSource>,
    // engine: spin_core::wasmtime::Engine,
    host_components: Vec<LoadedHostComponent>,
    // interfaces: HashMap<String, LazyService>,
}

impl HostComponentsFactor {
    /// Creates a new `HostComponentsFactor`.
    pub fn new() -> Self {
        let component_sources = TEST_TEST_TEST_PATHS.iter().map(PathBuf::from).collect();
        // let engine = hosting::create_host_engine().unwrap();
        Self { component_sources, host_components: Default::default() }
    }
}

impl Factor for HostComponentsFactor {
    type RuntimeConfig = ();
    type AppState = AppState;
    type InstanceBuilder = InstanceState;

    fn init(&mut self, ctx: &mut impl InitContext<Self>) -> anyhow::Result<()> {
        let linker = ctx.linker();
        // let engine = linker.engine().clone();
        let engine = hosting::create_host_engine()?;

        // TODO: async or parallelise
        self.host_components = self.component_sources
            .iter()
            .map(|cs| loader::load_host_component(&engine, cs))
            .collect::<Result<_, _>>()?;

        let tokio_rt = tokio::runtime::Handle::current();

        for hc in &self.host_components {
            let instance_fut = instantiate_host_component(engine.clone(), hc.clone(), None);  // TODO: data dir?
            let instance = tokio::task::block_in_place(|| tokio_rt.block_on(instance_fut))?;

            for interface in &hc.exported_interfaces {
                linker::link_bindings(linker, interface, instance.clone())?;
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
    ) -> anyhow::Result<InstanceState> {
        Ok(InstanceState {
        })
    }
}

pub struct AppState {
}

pub struct InstanceState {
}

impl SelfInstanceBuilder for InstanceState {}
