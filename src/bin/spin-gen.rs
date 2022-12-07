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
        let mut schema = schemars::schema_for!(spin_loader::local::config::RawAppManifestAnyVersion);
        let mut post_processor = MangleOTronic3000{};
        schemars::visit::visit_root_schema(&mut post_processor, &mut schema);
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
        Ok(())
    }
}

struct MangleOTronic3000;

impl schemars::visit::Visitor for MangleOTronic3000 {
    fn visit_schema(&mut self, schema: &mut schemars::schema::Schema) {
        let ver_desc = Some("Version of the application.".to_owned());
        if let schemars::schema::Schema::Object(o) = schema {
            if let Some(met) = o.metadata.as_ref() {
                if let Some(schemars::schema::SingleOrVec::Single(ty)) = &o.instance_type {
                    if *ty.as_ref() == schemars::schema::InstanceType::String && met.description == ver_desc {
                        o.format = Some("semver".into());
                    }
                }
            }
        }
        schemars::visit::visit_schema(self, schema)
    }
}
