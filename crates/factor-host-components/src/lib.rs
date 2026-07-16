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

enum ComponentSource {
    Local { path: PathBuf },
}

type SharedService = Arc<Mutex<HostComponentInstance>>;

/// A factor for providing variables to components.
#[derive(Default)]
pub struct HostComponentsFactor {
    component_sources: Vec<ComponentSource>,
    // engine: spin_core::wasmtime::Engine,
    host_components: Vec<LoadedHostComponent>,
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
        Self { component_sources, host_components: Default::default() }
    }
}

impl Factor for HostComponentsFactor {
    type RuntimeConfig = ();
    type AppState = AppState;
    type InstanceBuilder = InstanceBuilder;

    fn init<T: InitContext<Self>>(&mut self, ctx: &mut T) -> anyhow::Result<()> {
        let engine = hosting::create_host_engine()?;

        // TODO: async or parallelise
        self.host_components = self.component_sources
            .iter()
            .map(|cs| loader::load_host_component(&engine, cs))
            .collect::<Result<_, _>>()?;

        let tokio_rt = tokio::runtime::Handle::current();

        for hc in &self.host_components {
            let instance_fut = instantiate_host_component(engine.clone(), hc.clone(), None);  // TODO: data dir?
            let instance: SharedService = tokio::task::block_in_place(|| tokio_rt.block_on(instance_fut))?;

            for interface in &hc.exported_interfaces {
                let instance2 = instance.clone();
                ctx.link_bindings(move |linker, store_data_to_instance_state_fn| {
                    let mut linker_instance = linker.instance(&interface.name).unwrap();
                    for (func_name, is_async) in &interface.functions {
                        let instance3 = instance2.clone();
                        linker_instance.func_new_async(&func_name, move |mut store_ctx, f, params, results| {
                            let instance4 = instance3.clone();
                            let fut = async move {
                                let inst_state = store_data_to_instance_state_fn(store_ctx.data_mut());
                                let iii = instance4.lock().await.instance_pre.clone();
                                let fie: spin_core::wasmtime::InstancePre<T::StoreData> = iii;

                                // okay StoreContextMut does fulfil AsContextMut but is the StoreData the wrong type?
                                // the trouble is we can't prove that InitContext::StoreData is InstanceState
                                
                                // This doesn't work and is way too late anyway. If it *does* work,
                                // we need to cache it in the InstanceState so you don't get different
                                // instances on every call, but still, ugh.
                                //
                                // But since this *doesn't* work, it's moot.
                                //
                                let wat = fie.instantiate_async(store_ctx).await.unwrap();
                                Ok(())
                            };    
                            Box::new(fut)
                        }).unwrap();
                    }
                    Ok(())
                })?;
            }
        }

        // {
        //     ctx.link_bindings(|linker, store_data_to_instance_state_fn| {
        //         let mut linker_instance = linker.instance("arse").unwrap();
        //         linker_instance.func_new_async("spork", move |mut store_ctx, f, params, results| {
        //             let istate = store_data_to_instance_state_fn(store_ctx.data_mut());
        //             // BWAHAHAHA
        //             todo!()
        //         });
        //         Ok(())
        //     })?;
        // }
        // let linker = ctx.linker();
        // // let engine = linker.engine().clone();

        // for hc in &self.host_components {
        //     let instance_fut = instantiate_host_component(engine.clone(), hc.clone(), None);  // TODO: data dir?
        //     let instance = tokio::task::block_in_place(|| tokio_rt.block_on(instance_fut))?;

        //     for interface in &hc.exported_interfaces {
        //         linker::link_bindings::<T>(linker, interface, instance.clone())?;
        //     }
        // }

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
}

impl spin_factors::FactorInstanceBuilder for InstanceBuilder {
    type InstanceState = InstanceState;

    fn build(mut self) -> anyhow::Result<Self::InstanceState> {
        Ok(Self::InstanceState {
            wasi: self.wasi_builder.build(),
            table: wasmtime_wasi::ResourceTable::with_capacity(100),
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
    pub fn biscuits(&self) -> String {
        "biscuits!".to_string()
    }
}
