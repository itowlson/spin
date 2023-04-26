use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::{Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use spin_trigger::{
    TriggerAppEngine, TriggerExecutor, cli::NoArgs, EitherInstance,
};

// wasmtime::component::bindgen!({
//     path: "../../wit/preview2/inbound-worker.wit",
//     world: "inbound-worker",
//     async: true
// });

pub(crate) type RuntimeData = ();
pub(crate) type _Store = spin_core::Store<RuntimeData>;

// const TRIGGER_METADATA_KEY: MetadataKey<TriggerMetadata> = MetadataKey::new("trigger");

pub struct WorkerTrigger {
    engine: TriggerAppEngine<Self>,
    component_queue_dirs: HashMap<String, PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTriggerConfig {
    /// Component ID to invoke
    pub component: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TriggerMetadata {
    r#type: String,
}

#[async_trait]
impl TriggerExecutor for WorkerTrigger {
    const TRIGGER_TYPE: &'static str = "worker";
    type RuntimeData = RuntimeData;
    type TriggerConfig = WorkerTriggerConfig;
    type RunConfig = NoArgs;

    async fn new(engine: TriggerAppEngine<Self>) -> Result<Self> {
        let component_queue_dirs = match engine.runtime_config.state_dir() {
            None => anyhow::bail!("workers require a state directory and this app doesn't have one"),
            Some(sd) =>
                engine.app().components().map(|c| (c.id().to_owned(), sd.join(c.id()))).collect(),
        };

        Ok(Self {
            engine,
            component_queue_dirs,
        })
    }

    async fn run(self, _config: Self::RunConfig) -> Result<()> {
        let ctrlc = tokio::spawn(async move {
            tokio::signal::ctrl_c().await.unwrap();
            // std::process::exit(0);
        });

        let engine = Arc::new(self.engine);

        let mut loops = self.component_queue_dirs.into_iter().map(|(id, queue_dir)| {
            Self::start_receive_loop(engine.clone(), id, queue_dir)
        }).collect::<Vec<_>>();

        let mut tasks = vec![ctrlc];
        tasks.append(&mut loops);

        let (_, _, rest) = futures::future::select_all(tasks).await;
        drop(rest);

        Ok(())
    }
}

impl WorkerTrigger {
    fn start_receive_loop(engine: Arc<TriggerAppEngine<Self>>, id: String, queue_dir: PathBuf) -> tokio::task::JoinHandle<()> {
        let future = Self::receive(engine, id, queue_dir);
        tokio::task::spawn(future)
    }

    // This doesn't return a Result because we don't want a thoughtless `?` to exit the loop
    // and terminate the entire trigger.
    async fn receive(engine: Arc<TriggerAppEngine<Self>>, id: String, queue_dir: PathBuf) -> () {
        let mut receiver = match yaque::Receiver::open(&queue_dir) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Can't watch queue for {id} in {}: {e}", queue_dir.display());
                return;
            }
        };

        loop {
            match receiver.recv_timeout(futures_timer::Delay::new(std::time::Duration::from_millis(100))).await {
                Err(e) => eprintln!("OH NO {e:?}"),
                Ok(None) => futures_timer::Delay::new(std::time::Duration::from_millis(100)).await,
                Ok(Some(r)) => {
                    // Can I do this once outside the loop and reuse, or do I have to do it every time?
                    let (instance, mut store) = match engine.prepare_instance(&id).await {
                        Ok(is) => is,
                        Err(e) => {
                            tracing::error!("Failed to prepare Wasm instance for {id}: {e}");
                            _ = r.rollback(); // TODO: HEY YOU THE DEAD LETTER QUEUE
                            continue;
                        }
                    };
                    let EitherInstance::Component(instance) = instance else {
                        unreachable!()
                    };

                    let func = instance
                        .exports(&mut store)
                        .instance("inbound-worker")
                        .ok_or_else(|| anyhow::anyhow!("no inbound-worker instance found"))
                        .unwrap()  // ALERT ALERT ALERT
                        .typed_func::<
                            (spin_core::inbound_worker::Payload,),
                            (core::result::Result<(), spin_core::inbound_worker::Error>,)
                        >("execute")
                        .unwrap();  // ALERT ALERT ALERT;
        
                    // let instance = match spin_core::inbound_worker::InboundWorker::new(&mut store, &instance) {
                    //     Ok(i) => i,
                    //     Err(e) => {
                    //         tracing::error!("Failed to create Wasm instance for {id}: {e}");
                    //         _ = r.rollback(); // TODO: HEY YOU THE DEAD LETTER QUEUE
                    //         continue;
                    //     }
                    // };

                    let payload = r.as_ref();
                    // TODO: spawn this instead of awaiting it
                    let er = func.call_async(&mut store, (payload,)).await;

                    match er {
                        Err(e) => {
                            eprintln!("call failed {e:?}");
                            _ = r.rollback();
                        }
                        Ok((Err(e),)) => {
                            eprintln!("exec returned error {e:?}");
                            _ = r.rollback();
                        }
                        // BEHOLD THE UNPARALLELED ERGONOMICS OF THE COMPONENT MODEL
                        Ok((Ok(()),)) => { _ = r.commit(); },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
}
