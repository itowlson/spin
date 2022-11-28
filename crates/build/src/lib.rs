#![deny(missing_docs)]

//! A library for building Spin components.

mod manifest;

use anyhow::{bail, Context, Result};
use spin_loader::local::{
    parent_dir
};
use std::path::{Path, PathBuf};
use subprocess::{Exec, Redirection};
use tracing::log;

use crate::manifest::{BuildAppInfoAnyVersion, RawBuildConfig};

/// If present, run the build command of each component.
pub async fn build(manifest_file: &Path) -> Result<()> {
    let buildable = buildable_components(manifest_file).await?;
    if buildable.is_empty() {
        println!("No build command found!");
        return Ok(());
    }

    let app_dir = parent_dir(manifest_file)?;

    let results = futures::future::join_all(
        buildable
            .into_iter()
            .map(|(id, build)| build_component(id, build, &app_dir))
            .collect::<Vec<_>>(),
    )
    .await;

    for r in results {
        if r.is_err() {
            bail!(r.err().unwrap());
        }
    }

    println!("Successfully ran the build command for the Spin components.");
    Ok(())
}

/// Run the build command of the component.
async fn build_component(id: String, build: RawBuildConfig, app_dir: &Path) -> Result<()> {
    println!(
        "Executing the build command for component {}: {}",
        id, build.command
    );
    let workdir = construct_workdir(app_dir, build.workdir.as_ref())?;
    if build.workdir.is_some() {
        println!("Working directory: {:?}", workdir);
    }

    let res = Exec::shell(&build.command)
        .cwd(workdir)
        .stdout(Redirection::Pipe)
        .capture()
        .with_context(|| {
            format!(
                "Cannot spawn build process '{:?}' for component {}.",
                &build.command, id
            )
        })?;

    if !res.stdout_str().is_empty() {
        log::info!("Standard output for component {}", id);
        print!("{}", res.stdout_str());
    }

    if !res.success() {
        bail!(
            "Build command for component {} failed with status {:?}.",
            id,
            res.exit_status
        );
    }

    Ok(())
}

async fn buildable_components(manifest_file: &Path) -> Result<Vec<(String, RawBuildConfig)>> {
    let manifest_text = tokio::fs::read_to_string(manifest_file)
        .await
        .with_context(|| format!("Cannot read manifest file from {}", manifest_file.display()))?;
    let BuildAppInfoAnyVersion::V1(app) = toml::from_str(&manifest_text)?;
    Ok(app
        .components
        .into_iter()
        .filter_map(|c| c.build.map(|b| (c.id, b)))
        .collect())
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
        build(&bad_trigger_file).await.unwrap();
    }
}
