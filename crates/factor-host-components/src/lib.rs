mod error;
mod hosting;
mod linker;
mod loader;

use std::{path::PathBuf, sync::Arc};

use spin_factors::{
    ConfigureAppContext, Factor, InitContext, PrepareContext, RuntimeFactors,
    anyhow,
};
use tokio::sync::Mutex;

use crate::{linker::HostComponentInstance, loader::{LoadedHostComponent, instantiate_host_component}};

enum ComponentSource {
    Local { path: PathBuf },
}

type SharedService<SD> = Arc<Mutex<HostComponentInstance<SD>>>;

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
            let instance_fut = instantiate_host_component::<T>(engine.clone(), hc.clone(), None);  // TODO: data dir?
            let preinst: SharedService<T::StoreData> = tokio::task::block_in_place(|| tokio_rt.block_on(instance_fut))?;

            for interface in &hc.exported_interfaces {
                let inst_pre_ur = tokio::task::block_in_place(|| tokio_rt.block_on(async { preinst.lock().await.instance_pre.clone() }));
                let mut linker_instance = ctx.linker().instance(&interface.name).unwrap();
                for (func_name, is_async) in &interface.functions {
                    let inst_pre = inst_pre_ur.clone();
                    let func_name_1 = func_name.clone();
                    let func_name_2 = func_name.clone();
                    if *is_async {
                        linker_instance.func_new_concurrent(&func_name_1, move |accessor, _f, params, results| {
                            let (_hc_instance, func) = accessor.with(|mut access| {
                                use spin_core::wasmtime::AsContextMut;
                                let tokio_rt_cb = tokio::runtime::Handle::current();
                                let hc_instance = tokio::task::block_in_place(|| tokio_rt_cb.block_on(inst_pre.instantiate_async(access.as_context_mut()))).unwrap();
                                let func = hc_instance.get_func(access.as_context_mut(), &func_name_2).unwrap();
                                (hc_instance, func)
                            });
                            let fut = async move {
                                func.call_concurrent(accessor, params, results).await.unwrap();
                                Ok(())
                            };
                            Box::pin(fut)
                        }).unwrap();
                    } else {
                        linker_instance.func_new_async(&func_name_1, move |mut store_ctx, _f, params, results| {
                            let inst_pre2 = inst_pre.clone();
                            let func_name_3 = func_name_2.clone();
                            let fut = async move {
                                eprintln!("instantiating for normal fn");
                                let hc_instance = inst_pre2.instantiate_async(&mut store_ctx).await.unwrap();
                                eprintln!("get func {func_name_3}");
                                let func = hc_instance.get_func(&mut store_ctx, &func_name_3).unwrap();
                                eprintln!("got func {func_name_3}, calling");
                                func.call_async(&mut store_ctx, params, results).await.unwrap();
                                Ok(())
                            };
                            Box::new(fut)
                        }).unwrap();
                    }
                }
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
        // let wib = ctx.instance_builder::<spin_factor_wasi::WasiFactor>()?;
        // wib.
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
    // instances: HashMap<String, spin_core::wasmtime::component::Instance>,
}

impl spin_factors::FactorInstanceBuilder for InstanceBuilder {
    type InstanceState = InstanceState;

    fn build(mut self) -> anyhow::Result<Self::InstanceState> {
        Ok(Self::InstanceState {
            wasi: self.wasi_builder.build(),
            table: wasmtime_wasi::ResourceTable::with_capacity(100),
            // instances: Default::default()
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

    // pub fn clocks(&mut self) -> &mut wasmtime_wasi::clocks::WasiClocksCtx {
    //     self.wasi.clocks()
    // }

    // pub fn cli(&mut self) -> &mut wasmtime_wasi::cli::WasiCliCtx {
    //     self.wasi.cli()
    // }
}
