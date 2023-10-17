#![deny(missing_docs)]

//! A library for building Spin components.

mod manifest;

use anyhow::{anyhow, bail, Context, Result};
use spin_common::paths::parent_dir;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use subprocess::{Exec, Redirection};

use crate::manifest::{BuildAppInfoAnyVersion, RawComponentManifest};

/// If present, run the build command of each component.
pub async fn build(manifest_file: &Path, component_ids: &[String]) -> Result<()> {
    let manifest_text = tokio::fs::read_to_string(manifest_file)
        .await
        .with_context(|| format!("Cannot read manifest file from {}", manifest_file.display()))?;
    let app = toml::from_str(&manifest_text).map(BuildAppInfoAnyVersion::into_v1)?;
    let app_dir = parent_dir(manifest_file)?;

    let components_to_build = if component_ids.is_empty() {
        app.components
    } else {
        let all_ids: HashSet<_> = app.components.iter().map(|c| &c.id).collect();
        let unknown_component_ids: Vec<_> = component_ids
            .iter()
            .filter(|id| !all_ids.contains(id))
            .map(|s| s.as_str())
            .collect();

        if !unknown_component_ids.is_empty() {
            bail!("Unknown component(s) {}", unknown_component_ids.join(", "));
        }

        app.components
            .into_iter()
            .filter(|c| component_ids.contains(&c.id))
            .collect()
    };

    if components_to_build.iter().all(|c| c.build.is_none()) {
        println!("None of the components have a build command.");
        println!("For information on specifying a build command, see https://developer.fermyon.com/spin/build#setting-up-for-spin-build.");
        return Ok(());
    }

    components_to_build
        .into_iter()
        .map(|c| build_component(c, &app_dir))
        .collect::<Result<Vec<_>, _>>()?;

    terminal::step!("Finished", "building all Spin components");
    Ok(())
}

/// Run the build command of the component.
fn build_component(raw: RawComponentManifest, app_dir: &Path) -> Result<()> {
    match raw.build {
        Some(b) => {
            terminal::step!("Building", "component {} with `{}`", raw.id, b.command);
            let workdir = construct_workdir(app_dir, b.workdir.as_ref())?;
            if b.workdir.is_some() {
                println!("Working directory: {:?}", workdir);
            }

            let exit_status = Exec::shell(&b.command)
                .cwd(workdir)
                .stdout(Redirection::None)
                .stderr(Redirection::None)
                .stdin(Redirection::None)
                .popen()
                .map_err(|err| {
                    anyhow!(
                        "Cannot spawn build process '{:?}' for component {}: {}",
                        &b.command,
                        raw.id,
                        err
                    )
                })?
                .wait()?;

            if !exit_status.success() {
                bail!(
                    "Build command for component {} failed with status {:?}",
                    raw.id,
                    exit_status,
                );
            }

            Ok(())
        }
        _ => Ok(()),
    }
}

/// Constructs the absolute working directory in which to run the build command.
fn construct_workdir(app_dir: &Path, workdir: Option<impl AsRef<Path>>) -> Result<PathBuf> {
    let mut cwd = app_dir.to_owned();

    if let Some(workdir) = workdir {
        // Using `Path::has_root` as `is_relative` and `is_absolute` have
        // surprising behavior on Windows, see:
        // https://doc.rust-lang.org/std/path/struct.Path.html#method.is_absolute
        if workdir.as_ref().has_root() {
            bail!("The workdir specified in the application file must be relative.");
        }
        cwd.push(workdir);
    }

    Ok(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_data_root() -> PathBuf {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(crate_dir).join("tests")
    }

    #[tokio::test]
    async fn can_load_even_if_trigger_invalid() {
        let bad_trigger_file = test_data_root().join("bad_trigger.toml");
        build(&bad_trigger_file, &[]).await.unwrap();
    }
}
