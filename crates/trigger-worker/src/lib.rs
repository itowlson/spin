use anyhow::{Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use spin_trigger::{
    TriggerAppEngine, TriggerExecutor, cli::NoArgs,
};

pub(crate) type RuntimeData = ();
pub(crate) type Store = spin_core::Store<RuntimeData>;

// const TRIGGER_METADATA_KEY: MetadataKey<TriggerMetadata> = MetadataKey::new("trigger");

/// The Spin HTTP trigger.
pub struct WorkerTrigger {
    engine: TriggerAppEngine<Self>,
}

/// Configuration for the HTTP trigger
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
        Ok(Self {
            engine,
        })
    }

    async fn run(self, config: Self::RunConfig) -> Result<()> {

        Ok(())
    }
}

impl WorkerTrigger {
}

#[cfg(test)]
mod tests {
}
