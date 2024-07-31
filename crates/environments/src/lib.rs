use std::path::{Path, PathBuf};

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

pub struct TargetEnvironment {
    name: String,
    environments: std::collections::HashMap<TriggerType, TargetWorld>,
}

fn component_source<'a>(app: &'a spin_manifest::schema::v2::AppManifest, trigger: &'a spin_manifest::schema::v2::Trigger) -> anyhow::Result<&'a spin_manifest::schema::v2::ComponentSource> {
    let component_id = trigger.component.as_ref().ok_or_else(|| anyhow!("No component specified for trigger {}", trigger.id))?;
    let csrc = match component_id {
        spin_manifest::schema::v2::ComponentSpec::Inline(c) => &c.source,
        spin_manifest::schema::v2::ComponentSpec::Reference(r) => &app.components.get(r).ok_or_else(|| anyhow!("Component {r} specified for trigger {} does not exist", trigger.id))?.source,
    };
    Ok(csrc)
}

pub async fn validate_application_against_environment_ids(env_ids: impl Iterator<Item = &str>, app: &spin_manifest::schema::v2::AppManifest) -> anyhow::Result<()> {
    let envs = futures::future::join_all(env_ids.map(|id| resolve_environment_id(id))).await;
    let envs: Vec<_> = envs.into_iter().collect::<Result<_, _>>()?;
    validate_application_against_environments(&envs, app).await
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

pub async fn validate_application_against_environments(envs: &[TargetEnvironment], app: &spin_manifest::schema::v2::AppManifest) -> anyhow::Result<()> {
    for trigger_type in app.triggers.keys() {
        if let Some(env) = envs.iter().find(|e| !e.environments.contains_key(trigger_type)) {
            anyhow::bail!("Environment {} does not support trigger type {trigger_type}", env.name);
        }
    }

    let tt2components = app.triggers.iter()
        .map(|(ty, ts)|
            ts.iter()
                .map(|t| component_source(app, t))
                .collect::<Result<Vec<_>, _>>()
                .map(|css| (ty, css))
        )
        .collect::<Result<Vec<_>, _>>()?;

    for (trigger_type, css) in tt2components {
        for cs in &css {
            if let spin_manifest::schema::v2::ComponentSource::Local(path) = cs {
                let wasm_file = PathBuf::from(path);
                validate_component_against_environments(envs, trigger_type, &wasm_file).await?;
            } else {
                anyhow::bail!("can't do non-local component sources yet");
            }
        }
    }
    // let tt2components = app.triggers.iter()
    //     .into_group_map_by(|t| &t.trigger_type)
    //     .into_iter()
    //     .map(|(ty, ts)| ts.iter().map(|t| component_source(app, t)).collect::<Result<Vec<_>, _>>().map(|css| (ty, css)))
    //     .collect::<Result<std::collections::HashMap<_, _>, _>>()?;

    // for (trigger_type, component_sources) in &tt2components {
    //     for cs in component_sources {
    //         spin_locked_app::locked::LockedComponentSource
    //     }
    // }

    Ok(())
}

pub async fn validate_component_against_environments(envs: &[TargetEnvironment], trigger_type: &TriggerType, wasm_file: &Path) -> anyhow::Result<()> {
    let worlds = envs.iter()
        .map(|e| e.environments.get(trigger_type).ok_or(anyhow!("env {} doesn't support trigger type {trigger_type}", e.name)))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    validate_file_against_worlds(worlds.into_iter(), wasm_file).await?;
    Ok(())
}

async fn validate_file_against_worlds(target_worlds: impl Iterator<Item = &TargetWorld>, wasm_file: &Path) -> anyhow::Result<()> {
    let raw_wasm = std::fs::read(wasm_file)
        .with_context(|| format!("Couldn't read Wasm file {}", quoted_path(wasm_file)))?;
    // FUTURE: take in manifest composition as well
    let cooked_wasm = spin_componentize::componentize_if_necessary(&raw_wasm)
        .with_context(|| format!("Couldn't componentize Wasm file {}", quoted_path(wasm_file)))?;
    
    for target_world in target_worlds {
        validate_wasm_against_world(target_world, cooked_wasm.as_ref()).await?;
        tracing::info!("Validated component {wasm_file:?} against target world {}", target_world.versioned_name());
    }

    tracing::info!("Validated component {wasm_file:?} against all target worlds");
    Ok(())
}

// async fn validate_file_against_world(target_world: &TargetWorld, wasm_file: &Path) -> anyhow::Result<()> {
//     let raw_wasm = std::fs::read(wasm_file)
//         .with_context(|| format!("Couldn't read Wasm file {}", quoted_path(wasm_file)))?;
//     // FUTURE: take in manifest composition as well
//     let cooked_wasm = spin_componentize::componentize_if_necessary(&raw_wasm)
//         .with_context(|| format!("Couldn't componentize Wasm file {}", quoted_path(wasm_file)))?;

//     validate_wasm_against_world(target_world, cooked_wasm.as_ref()).await?;
//     tracing::info!("Validated component {wasm_file:?} against target world {}", target_world.versioned_name());
//     Ok(())
// }

async fn validate_wasm_against_world(target_world: &TargetWorld, cooked_wasm: &[u8]) -> anyhow::Result<()> {
    validate_wasm_wac(target_world, "root:component", cooked_wasm).await
}

async fn validate_wasm_wac(target_world: &TargetWorld, comp_name: &str, wasm: &[u8]) -> anyhow::Result<()> {
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
            // This one doesn't seem to get hit at the moment - we get MissingTargetExport instead.  We would expect TargetMismatch for other cases including imports though.
            Err(anyhow!("World {world} expects an {} named {name}", kind.to_string().to_lowercase()))
        }
        Err(wac_parser::resolution::Error::MissingTargetExport { name, world, .. }) => {
            Err(anyhow!("World {world} expects an export named {name}"))
        }
        Err(e) => {
            Err(anyhow!(e))
        },
    }
}
