use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use spin_common::ui::quoted_path;
use wasm_pkg_loader::PackageRef;

mod stolen_from_wasm_tools;

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
    validate_wasm_wac(target_world, "root:component", "1.0.0", cooked_wasm).await
}

// pub async fn validate_wasm_wt(target_world_name: &str, wasm: &[u8]) -> anyhow::Result<()> {
//     let resolve = stolen_from_wasm_tools::WitResolve {
//         wit: stolen_from_wasm_tools::WitSource::Dir(PathBuf::from("/home/ivan/github/spin/wit")),
//         features: vec![],
//         all_features: false,
//     };
//     let (resolve, pkg_ids) = resolve.load().await?;
//     let world = resolve.select_world(&pkg_ids, Some(target_world_name))?;

//     wit_component::targets(&resolve, world, wasm)?;
//     Ok(())
// }

async fn validate_wasm_wac(target_world: &TargetWorld, comp_name: &str, comp_version: &str, wasm: &[u8]) -> anyhow::Result<()> {
    let fake_span = || miette::SourceSpan::new(miette::SourceOffset::from_location("internal", 0, 0), 0);
    
    let comp_version_owned = semver::Version::parse(comp_version).unwrap();
    let comp_version_ref = Some(&comp_version_owned);
    // let comp_qname = format!("{comp_name}@{comp_version}");

    let comp_pkgname = wac_parser::PackageName {
        string: comp_name, // comp_qname.as_str(),
        name: comp_name,
        version: None, // comp_version_ref.cloned(),
        span: fake_span(),
    };

    let target_str = target_world.versioned_name();
    let target_ver = target_world.package_ver.as_str();
    let target_name = target_world.wit_package.to_string();
    let target_seg = target_world.world_name.as_str();
    // let Some((target_fname, target_ver)) = target_world.split_once('@') else {
    //     anyhow::bail!("target world has no version");
    // };
    // let Some((target_name, target_seg)) = target_fname.split_once('/') else {
    //     anyhow::bail!("target world should be `ns:name/world@ver`");
    // };
    let target_ver = Some(semver::Version::parse(target_ver)?);

    // What to call the result of the no-op composition. This has to be
    // distinct from the component being validated, even though the actual
    // content would be identical!
    let output_name = format!("{comp_name}-val");
    let output_qname = format!("{output_name}@{comp_version}");
    let output = wac_parser::PackageName {
        string: output_qname.as_str(),
        name: output_name.as_str(),
        version: comp_version_ref.cloned(),
        span: fake_span(),
    };

    // The target world against which to validate
    let validation_target = wac_parser::PackagePath {
        string: target_str.as_str(),
        name: target_name.as_str(),
        segments: target_seg,
        version: target_ver,
        span: fake_span(),
    };

    // The identifier by which the WAC doc refers to the (no-op) composition
    let comp_id = wac_parser::Ident { string: "c", span: fake_span() };

    let doc = wac_parser::Document {
        docs: vec![],
        // package the:component-val@0.1.2 targets the:target;
        directive: wac_parser::PackageDirective {
            package: output,
            targets: Some(validation_target),
        },
        statements: vec![
            // let c = new the:component { ... };  // NOTE: THIS MUST NOT BE VERSIONED
            wac_parser::Statement::Let(wac_parser::LetStatement {
                docs: vec![],
                id: comp_id.clone(),
                expr: wac_parser::Expr {
                    primary: wac_parser::PrimaryExpr::New(wac_parser::NewExpr {
                        span: fake_span(),
                        package: comp_pkgname,
                        arguments: vec![
                            wac_parser::InstantiationArgument::Fill(fake_span()),
                        ]
                    }),
                    span: fake_span(),
                    postfix: vec![],
                }
            }),
            // export c...;
            wac_parser::Statement::Export(wac_parser::ExportStatement {
                docs: vec![],
                expr: wac_parser::Expr {
                    span: fake_span(),
                    primary: wac_parser::PrimaryExpr::Ident(comp_id.clone()),
                    postfix: vec![],
                },
                options: wac_parser::ExportOptions::Spread(fake_span()),
            })
        ],
    };

    // println!("** DOC: {doc:?}");

    let mut refpkgs = IndexMap::new();
    let fspgkver = semver::Version::parse("2.0.0").unwrap();
    let fspkgkey = wac_types::BorrowedPackageKey::from_name_and_version("fermyon:spin", Some(&fspgkver));
    refpkgs.insert(fspkgkey, fake_span());
    // let whpgkver = semver::Version::parse("0.2.0").unwrap();
    // let whpkgkey = wac_types::BorrowedPackageKey::from_name_and_version("wasi:http", Some(&whpgkver));
    // refpkgs.insert(whpkgkey, fake_span());
    // let whpgkver = semver::Version::parse("0.2.0").unwrap();
    // let whpkgkey = wac_types::BorrowedPackageKey::from_name_and_version("wasi:cli", Some(&whpgkver));
    // refpkgs.insert(whpkgkey, fake_span());

    let reg_resolver = wac_resolver::RegistryPackageResolver::new(Some("wa.dev"), None).await?;
    let mut packages = reg_resolver.resolve(&refpkgs).await?;

    // let compkey = wac_types::BorrowedPackageKey::from_name_and_version(comp_name, comp_version_ref);
    let compkey2 = wac_types::BorrowedPackageKey::from_name_and_version(comp_name, None);
    // println!("** INSERTING {compkey}");
    // packages.insert(compkey, wasm.to_owned().to_vec());
    packages.insert(compkey2, wasm.to_owned().to_vec());

    match doc.resolve(packages) {
        Ok(r) => {
            println!("resolution: {:?}", r.into_graph());
            Ok(())
        },
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
