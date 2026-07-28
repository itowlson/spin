mod error;
mod hosting;
mod linker;
mod loader;

use std::{collections::HashMap, path::PathBuf, sync::Arc};

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
                let hc2 = hc.clone();
                let interface2 = interface.clone();
                let inst_pre_ur = tokio::task::block_in_place(|| tokio_rt.block_on(async { preinst.lock().await.instance_pre.clone() }));
                let mut linker_instance = ctx.linker().instance(&interface.name).unwrap();
                for (func_name, is_async) in &interface.functions {
                    let inst_pre = inst_pre_ur.clone();
                    let func_name_1 = func_name.clone();
                    let func_name_2 = func_name.clone();
                    let hc3 = hc2.clone();
                    let interface3 = interface2.clone();
                    if *is_async {
                        linker_instance.func_new_concurrent(&func_name_1, move |accessor, _f, params, results| {
                            let interface4 = interface3.clone();
                            let hc4 = hc3.clone();
                            let (_hc_instance, func) = accessor.with(|mut access| {
                                use spin_core::wasmtime::AsContextMut;
                                let tokio_rt_cb = tokio::runtime::Handle::current();
                                let hc_instance = tokio::task::block_in_place(|| tokio_rt_cb.block_on(inst_pre.instantiate_async(access.as_context_mut()))).unwrap();
                                let export_indices = snort_export_indices(&hc4, &hc_instance, access.as_context_mut()).unwrap();
                                let (_ei, fmap) = export_indices.get(&interface4.name).expect("itf name->fmap lookup failed");
                                let fi = fmap.get(&func_name_2).expect("fun name->index lookup failed");
                                let func = hc_instance.get_func(access.as_context_mut(), fi).expect("fun index->THE THINGY lookup failed");
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
                            let hc4 = hc3.clone();
                            let interface4 = interface3.clone();
                            let func_name_3 = func_name_2.clone();
                            let fut = async move {
                                //eprintln!("instantiating for normal fn");
                                let hc_instance = inst_pre2.instantiate_async(&mut store_ctx).await.unwrap();
                                let export_indices = snort_export_indices(&hc4, &hc_instance, &mut store_ctx).unwrap();
                                //eprintln!("get func {func_name_3}");
                                let (_ei, fmap) = export_indices.get(&interface4.name).expect("itf name->fmap lookup failed");
                                let fi = fmap.get(&func_name_3).expect("fun name->index lookup failed");
                                let func = hc_instance.get_func(&mut store_ctx, fi).expect("fun index->THE THINGY lookup failed");
                                // eprintln!("got func {func_name_3}, calling");
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
        let mut wasi_builder = wasmtime_wasi::WasiCtxBuilder::new();
        wasi_builder.inherit_stderr();
        // TODO: perms

        Ok(InstanceBuilder { wasi_builder })
    }
}

fn snort_export_indices(loaded: &LoadedHostComponent, instance: &spin_core::Instance, mut store: impl spin_core::wasmtime::AsContextMut) -> anyhow::Result<HashMap<String, (spin_core::wasmtime::component::ComponentExportIndex, HashMap<String, spin_core::wasmtime::component::ComponentExportIndex>)>> {
    use anyhow::anyhow;

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

    // for (itf, (_iindex, fmap)) in &export_indices {
    //     eprintln!("--- ITF {itf} ---");
    //     for (fname, fin) in fmap {
    //         eprintln!("* {fname} (idx={fin:?})");
    //     }
    // }

    Ok(export_indices)
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
