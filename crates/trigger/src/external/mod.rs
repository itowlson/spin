#![allow(missing_docs)]

use std::{path::PathBuf, collections::HashMap};

use serde::{Deserialize, Serialize};
use tonic::{transport::Server, Request, Response, Status};

use crate::{TriggerAppEngine, TriggerExecutor, cli::NoArgs};

pub struct ExternalTrigger {
    engine: TriggerAppEngine<Self>,
    program: PathBuf,
    component_infos: HashMap<String, HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExternalTriggerConfig {
    pub component: String,
    #[serde(flatten)]
    pub settings: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TriggerMetadata {
    r#type: String,
    program: String,
    #[serde(flatten)]
    pub settings: HashMap<String, String>,
}

#[async_trait::async_trait]
impl TriggerExecutor for ExternalTrigger {
    const TRIGGER_TYPE: & 'static str = "external";

    type RuntimeData = spin_external::SpinExternalData;

    type TriggerConfig = ExternalTriggerConfig;

    type RunConfig = NoArgs;

    fn new(engine: TriggerAppEngine<Self>) -> anyhow::Result<Self> {
        let program = engine.app().require_metadata::<TriggerMetadata>("trigger")?.program;

        let component_infos = engine
            .trigger_configs()
            .map(|(_, config)| (config.component.clone(), config.settings.clone()))
            .collect();

        Ok(Self {
            engine,
            program: PathBuf::from(program),
            component_infos,
        })
    }

    async fn run(self, config: Self::RunConfig) -> anyhow::Result<()> {
        println!("PROG = {}", self.program.display());
        println!("SETTINGS = {:?}", self.component_infos);

        // start gRPC server
        println!("RUNNING ME SOME GRPC");
        let addr = "[::1]:50051".parse().unwrap();
        let server_impl = MyProcessEvent { engine: self.engine };
        let grpc_server = Server::builder()
            .add_service(spinext::process_event_server::ProcessEventServer::new(server_impl))
            .serve(addr);

        println!("RUNNING ME SOME PROG");
        // launch program
        let mut listener_program = tokio::process::Command::new(&self.program).spawn()?;

        tokio::select! {
            _ = grpc_server => { }
            _ = listener_program.wait() => { }
        };

        println!("RUNNED ME SOME ENGINE YAY");
        Ok(())
    }
}

mod spinext {
    include!("spinext.rs");
}

struct MyProcessEvent {
    engine: TriggerAppEngine<ExternalTrigger>,
}

wit_bindgen_wasmtime::import!({paths: ["src/external/spin-external.wit"], async: *});

#[async_trait::async_trait]
impl spinext::process_event_server::ProcessEvent for MyProcessEvent {
    async fn event(&self, request: Request<spinext::EventInfo>) -> Result<Response<spinext::EventResponse>, tonic::Status> {
        let typeid = &request.get_ref().typeid;
        let body = &request.get_ref().body;

        let (instance, mut store) = self.engine.prepare_instance("madness")
            .await
            .map_err(|e| tonic::Status::internal(format!("Failed to prepare instance: {}", e.to_string())))?;

        let engine = spin_external::SpinExternal::new(&mut store, &instance, |data| data.as_mut())
            .map_err(|e| tonic::Status::internal(format!("Failed to instantiate engine: {}", e.to_string())))?;
        let raw_resp = engine.handle_external_event(&mut store, &body)
            .await
            .map_err(|e| tonic::Status::internal(format!("Wasm handler error: {}", e.to_string())))?;

        match raw_resp {
            Ok(r) => {
                let resp = spinext::EventResponse { something: r };
                Ok(Response::new(resp))
            }
            Err(e) => {
                Err(tonic::Status::internal(format!("Wasm module returned err {:?}", e)))
            }
        }
    }
}
