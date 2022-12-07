use anyhow::Error;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<(), Error> {
    SpinGenerateApp::parse().run().await
}

#[derive(Parser)]
#[clap(
    name = "spin-gen",
)]
enum SpinGenerateApp {
    AppManifest(AppManifestCommand),
}

impl SpinGenerateApp {
    /// The main entry point to Spin.
    pub async fn run(self) -> Result<(), Error> {
        match self {
            Self::AppManifest(cmd) => cmd.run().await,
        }
    }
}

#[derive(Parser, Debug)]
#[clap(about = "Generate the application manifest JSON Schema")]
struct AppManifestCommand;

impl AppManifestCommand {
    async fn run(&self) -> Result<(), Error> {
        let schema = schemars::schema_for!(spin_loader::local::config::RawAppManifestAnyVersion);
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
        Ok(())
    }
}
