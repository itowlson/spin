use std::path::PathBuf;

use anyhow::{anyhow, Context};
use spin_common::ui::quoted_path;
use wasm_pkg_loader::PackageRef;

#[derive(Debug, Eq, Hash, PartialEq)]
struct TargetWorld {
    wit_package: PackageRef,
    package_ver: String, // TODO: tidy to semver::Version
    world_name: String,
}

impl TargetWorld {
    fn versioned_name(&self) -> String {
        format!("{}/{}@{}", self.wit_package, self.world_name, self.package_ver)
    }
}

type TriggerType = String;

struct ComponentToValidate<'a> {
    id: &'a str,
    source: &'a spin_manifest::schema::v2::ComponentSource,
}

pub struct TargetEnvironment {
    name: String,
    environments: std::collections::HashMap<TriggerType, TargetWorld>,
}

pub struct ResolutionContext {
    pub base_dir: PathBuf,
}

fn component_source<'a>(app: &'a spin_manifest::schema::v2::AppManifest, trigger: &'a spin_manifest::schema::v2::Trigger) -> anyhow::Result<ComponentToValidate<'a>> {
    let component_spec = trigger.component.as_ref().ok_or_else(|| anyhow!("No component specified for trigger {}", trigger.id))?;
    let (id, csrc) = match component_spec {
        spin_manifest::schema::v2::ComponentSpec::Inline(c) => (trigger.id.as_str(), &c.source),
        spin_manifest::schema::v2::ComponentSpec::Reference(r) => (r.as_ref(), &app.components.get(r).ok_or_else(|| anyhow!("Component {r} specified for trigger {} does not exist", trigger.id))?.source),
    };
    Ok(ComponentToValidate { id, source: csrc })
}

pub async fn validate_application_against_environment_ids(env_ids: impl Iterator<Item = &str>, app: &spin_manifest::schema::v2::AppManifest, resolution_context: &ResolutionContext) -> anyhow::Result<()> {
    let envs = futures::future::join_all(env_ids.map(|id| resolve_environment_id(id))).await;
    let envs: Vec<_> = envs.into_iter().collect::<Result<_, _>>()?;
    validate_application_against_environments(&envs, app, resolution_context).await
}

async fn resolve_environment_id(id: &str) -> anyhow::Result<TargetEnvironment> {
    if id == "spin-cli@2.5.0" {
        let mut environments = std::collections::HashMap::new();
        environments.insert("http".into(), TargetWorld { wit_package: PackageRef::try_from("fermyon:spin".to_string())?, package_ver: "2.0.0".to_string(), world_name: "http-trigger".into() });
        environments.insert("redis".into(), TargetWorld { wit_package: PackageRef::try_from("fermyon:spin".to_string())?, package_ver: "2.0.0".to_string(), world_name: "redis-trigger".into() });
        Ok(TargetEnvironment {
            name: id.to_string(),
            environments,
        })
    } else {
        Err(anyhow!("Unknown target environment {id}"))
    }
}

pub async fn validate_application_against_environments(envs: &[TargetEnvironment], app: &spin_manifest::schema::v2::AppManifest, resolution_context: &ResolutionContext) -> anyhow::Result<()> {
    for trigger_type in app.triggers.keys() {
        if let Some(env) = envs.iter().find(|e| !e.environments.contains_key(trigger_type)) {
            anyhow::bail!("Environment {} does not support trigger type {trigger_type}", env.name);
        }
    }

    let components_by_trigger_type = app.triggers.iter()
        .map(|(ty, ts)|
            ts.iter()
                .map(|t| component_source(app, t))
                .collect::<Result<Vec<_>, _>>()
                .map(|css| (ty, css))
        )
        .collect::<Result<Vec<_>, _>>()?;

    for (trigger_type, component) in components_by_trigger_type {
        for component in &component {
            validate_component_against_environments(envs, trigger_type, &component, resolution_context).await?;
        }
    }

    Ok(())
}

async fn validate_component_against_environments(envs: &[TargetEnvironment], trigger_type: &TriggerType, component: &ComponentToValidate<'_>, resolution_context: &ResolutionContext) -> anyhow::Result<()> {
    let worlds = envs.iter()
        .map(|e| e.environments
            .get(trigger_type)
            .ok_or(anyhow!("env {} doesn't support trigger type {trigger_type}", e.name))
            .map(|w| (e.name.as_str(), w))
        )
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    validate_file_against_worlds(worlds.into_iter(), component, resolution_context).await?;
    Ok(())
}

impl ResolutionContext {
    async fn load_wasm(&self, source: &spin_manifest::schema::v2::ComponentSource) -> anyhow::Result<Vec<u8>> {
        if let spin_manifest::schema::v2::ComponentSource::Local(path) = source {
            let wasm_file = self.base_dir.join(path);
            Ok(std::fs::read(&wasm_file).with_context(|| format!("Can't read Wasm file {}", quoted_path(wasm_file)))?)
        } else {
            anyhow::bail!("can't do non-local component sources yet");
        }
    }
}

fn source_description(source: &spin_manifest::schema::v2::ComponentSource) -> String {
    match source {
        spin_manifest::schema::v2::ComponentSource::Local(path) => format!("file {}", quoted_path(path)),
        spin_manifest::schema::v2::ComponentSource::Remote { url, .. } => format!("URL {url}"),
        spin_manifest::schema::v2::ComponentSource::Registry { package, .. } => format!("package {package}"),
    }
}

async fn validate_file_against_worlds(target_worlds: impl Iterator<Item = (&str, &TargetWorld)>, component: &ComponentToValidate<'_>, resolution_context: &ResolutionContext) -> anyhow::Result<()> {
    let raw_wasm = resolution_context.load_wasm(&component.source)
        .await
        .with_context(|| format!("Couldn't read Wasm {}", source_description(&component.source)))?;
    // FUTURE: take in manifest composition as well
    let cooked_wasm = spin_componentize::componentize_if_necessary(&raw_wasm)
        .with_context(|| format!("Couldn't componentize Wasm {}", source_description(&component.source)))?;
    
    for (env_name, target_world) in target_worlds {
        validate_wasm_against_world(env_name, target_world, component, cooked_wasm.as_ref()).await?;
        tracing::info!("Validated component {} {} against target world {}", component.id, source_description(&component.source), target_world.versioned_name());
    }

    tracing::info!("Validated component {} {} against all target worlds", component.id, source_description(&component.source));
    Ok(())
}

async fn validate_wasm_against_world(env_name: &str, target_world: &TargetWorld, component: &ComponentToValidate<'_>, cooked_wasm: &[u8]) -> anyhow::Result<()> {
    validate_wasm_wac(env_name, target_world, component, "root:component", cooked_wasm).await
}

async fn validate_wasm_wac(env_name: &str, target_world: &TargetWorld, component: &ComponentToValidate<'_>, comp_name: &str, wasm: &[u8]) -> anyhow::Result<()> {
    let target_str = target_world.versioned_name();

    let wac_text = format!(r#"
    package validate:component@1.0.0 targets {target_str};
    let c = new {comp_name} {{ ... }};
    export c...;
    "#);

    let doc = wac_parser::Document::parse(&wac_text)?;

    let compkey = wac_types::BorrowedPackageKey::from_name_and_version(comp_name, None);

    let mut refpkgs = wac_resolver::packages(&doc)?;
    refpkgs.retain(|k, _| k != &compkey);

    let reg_resolver = wac_resolver::RegistryPackageResolver::new(Some("wa.dev"), None).await?;
    let mut packages = reg_resolver.resolve(&refpkgs).await?;

    packages.insert(compkey, wasm.to_owned().to_vec());

    match doc.resolve(packages) {
        Ok(_) => Ok(()),
        Err(wac_parser::resolution::Error::TargetMismatch { kind, name, world, .. }) => {
            // This one doesn't seem to get hit at the moment - we get MissingTargetExport or ImportNotInTarget instead
            Err(anyhow!("Component {} ({}) can't run in environment {env_name} because world {world} expects an {} named {name}", component.id, source_description(&component.source), kind.to_string().to_lowercase()))
        }
        Err(wac_parser::resolution::Error::MissingTargetExport { name, world, .. }) => {
            Err(anyhow!("Component {} ({}) can't run in environment {env_name} because world {world} requires an export named {name}, which the component does not provide", component.id, source_description(&component.source)))
        }
        Err(wac_parser::resolution::Error::PackageMissingExport { export, .. }) => {
            // TODO: The export here seems wrong - it seems to contain the world name rather than the interface name
            Err(anyhow!("Component {} ({}) can't run in environment {env_name} because world {target_str} requires an export named {export}, which the component does not provide", component.id, source_description(&component.source)))
        }
        Err(wac_parser::resolution::Error::ImportNotInTarget { name, world, .. }) => {
            Err(anyhow!("Component {} ({}) can't run in environment {env_name} because world {world} does not provide an import named {name}, which the component requires", component.id, source_description(&component.source)))
        }
        Err(e) => {
            println!("** OTHER OTHER OTHER: {e:?}");  // TODO: remove
            Err(anyhow!(e))
        },
    }
}
