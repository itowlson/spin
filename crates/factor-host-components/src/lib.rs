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

#[derive(Clone, Debug)]
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

// struct HCReq<'a> {
//     is_async: bool,
//     itf_name: String,
//     func_name: String,
//     params: &'a [spin_core::wasmtime::component::Val],
//     results: &'a mut [spin_core::wasmtime::component::Val],
// }
struct HCReq {
    is_async: bool,
    itf_name: String,
    func_name: String,
    params: Vec<spin_core::wasmtime::component::Val>,
    result_count: usize,
    resp_tx: tokio::sync::oneshot::Sender<HCResp>,
}
#[derive(Debug)]
struct HCResp {
    results: Vec<spin_core::wasmtime::component::Val>,
}

impl Factor for HostComponentsFactor {
    type RuntimeConfig = ();
    type AppState = AppState;
    type InstanceBuilder = InstanceState;

    fn init<T: InitContext<Self>>(&mut self, ctx: &mut T) -> anyhow::Result<()> {
        let linker = ctx.linker();
        // let engine = linker.engine().clone();
        let engine = hosting::create_host_engine()?;

        let mut hcs = vec![];

        for cs in &self.component_sources {
            let (tx, mut rx) = tokio::sync::mpsc::channel::<HCReq>(1024);
            let cs_cl = cs.clone();
            let engine_cl = engine.clone();

            let t = std::thread::spawn(|| {
                let rt = tokio::runtime::Builder::new_multi_thread().enable_all().name("host-components-worker").build().unwrap();
                rt.block_on(async move {
                    let hc = loader::load_host_component(&engine_cl, &cs_cl).unwrap();
                    let hci = instantiate_host_component(engine_cl, hc.clone(), None).await.unwrap();
                    while let Some(msg) = rx.recv().await {
                        let mut guard = hci.lock().await;
                        let mut results = vec![spin_core::wasmtime::component::Val::Bool(false); msg.result_count];
                        let res = if msg.is_async {
                            guard.call_func(&msg.itf_name, &msg.func_name, &msg.params, &mut results).await
                        } else {
                            guard.call_func_concurrent(&msg.itf_name, &msg.func_name, &msg.params, &mut results).await
                        };
                        res.unwrap();
                        msg.resp_tx.send(HCResp { results }).unwrap();
                    }
                });
            });

            let hc = loader::load_host_component(&engine, &cs).unwrap();
            for interface in &hc.exported_interfaces {
                linker::link_bindings2(linker, interface, tx.clone())?;
            }

            hcs.push(t);
        }

        // TODO: async or parallelise
        self.host_components = self.component_sources
            .iter()
            .map(|cs| loader::load_host_component(&engine, cs))
            .collect::<Result<_, _>>()?;

        // let tokio_rt = tokio::runtime::Handle::current();

        // for hc in &self.host_components {
        //     let instance_fut = instantiate_host_component(engine.clone(), hc.clone(), None);  // TODO: data dir?
        //     let instance = tokio::task::block_in_place(|| tokio_rt.block_on(instance_fut))?;

        //     for interface in &hc.exported_interfaces {
        //         linker::link_bindings(linker, interface, instance.clone())?;
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
