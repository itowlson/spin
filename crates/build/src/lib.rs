#![deny(missing_docs)]

//! A library for building Spin components.

mod manifest;

use anyhow::{bail, Context, Result, anyhow};
use crossterm::{QueueableCommand, cursor};
use spin_loader::local::{
    parent_dir
};
use std::{path::{Path, PathBuf}, io::Write};
use subprocess::{Exec, Redirection, PopenError, ExitStatus};
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

/// Checks the prerequisites declared in the `build` section for each component,
/// and prints the messages for any that are unsatisfied.
///
/// Similar to the build function, this prints its output directly, and returns
/// an error only if it can't actually execute the checks.
pub async fn build_prerequisites(manifest_file: &Path) -> Result<()> {
    let buildable = buildable_components(manifest_file).await?;
    if buildable.is_empty() {
        println!("No build command found!");
        return Ok(());
    }

    let results = futures::future::join_all(
        buildable
            .into_iter()
            .map(|(id, build)| component_prerequisites(id, build))
            .collect::<Vec<_>>(),
    )
    .await;

    for r in results {
        if r.is_err() {
            bail!(r.err().unwrap());
        }
    }

    Ok(())
}

async fn component_prerequisites(id: String, build: RawBuildConfig) -> Result<()> {
    print!("Checking component {}...", id);

    let prerequisites = build.prerequisites.unwrap_or_default();
    if prerequisites.is_empty() {
        println!(" no prerequisites listed");
        return Ok(())
    } else {
        println!();
    }

    let mut stdout = std::io::stdout();

    for (key, prerequisite) in prerequisites {
        let _ = stdout.queue(cursor::SavePosition);
        stdout.write_all(format!("- {key}...").as_bytes())?;
        let _ = stdout.queue(cursor::RestorePosition);
        let _ = stdout.flush();
        match check_prerequisite(&prerequisite).await {
            Err(e) => {
                stdout.write_all(format!("X {}... couldn't check!\n  {}\n", key, e.to_string()).as_bytes())?;
                // println!(" couldn't check!\n  - {}", e.to_string());
                break;
            }
            Ok(CheckResult::Passed) => stdout.write_all(format!("/ {key}... okay\n").as_bytes())?, // println!("okay"),
            Ok(CheckResult::Failed) => stdout.write_all(format!("X {}... failed\n  {}\n", key, prerequisite.message).as_bytes())?,  // println!("failed\n  - {}", prerequisite.message),
            Ok(CheckResult::Stop) => {
                stdout.write_all(format!("X {}... failed\n  {}\n", key, prerequisite.message).as_bytes())?;
                // println!("failed\n  - {}", prerequisite.message);
                break;
            }
        }
    }

    Ok(())
}

enum CheckResult {
    Passed,
    Failed,  // failed but can still check others
    Stop,  // failed and this will stop us checking others
}

async fn check_prerequisite(prerequisite: &manifest::RawBuildPrerequisite) -> Result<CheckResult> {
    match run_silently(&prerequisite.command) {
        ExecutionResult::Succeeded => Ok(CheckResult::Passed),
        ExecutionResult::ErrorStatus(_, stdout, stderr) => Ok(CheckResult::Failed),
        _ => Err(anyhow!("Failed to run check")),  // TODO: more info
    }
}

fn run_silently(command: &str) -> ExecutionResult {
    match Exec::shell(command)
        .stdout(Redirection::Pipe)
        .stderr(Redirection::Pipe)
        .capture() {
            Err(PopenError::IoError(e)) => match e.kind() {
                std::io::ErrorKind::NotFound => ExecutionResult::NotFound,
                _ => ExecutionResult::OtherError(e.to_string()),
            },
            Err(e) => ExecutionResult::OtherError(e.to_string()),
            Ok(capture) => {
                match capture.exit_status {
                    ExitStatus::Exited(code) => if capture.success() {
                        ExecutionResult::Succeeded
                    } else {
                        ExecutionResult::ErrorStatus(code, capture.stdout_str(), capture.stderr_str())
                    },
                    ExitStatus::Signaled(_) => ExecutionResult::Cancelled,
                    ExitStatus::Other(e) => ExecutionResult::OtherError(e.to_string()),
                    ExitStatus::Undetermined => ExecutionResult::UnknownStatus,
                }
            },
        }
}

enum ExecutionResult {
    NotFound,
    Succeeded,
    ErrorStatus(u32, String, String),
    UnknownStatus,
    Cancelled,
    OtherError(String),
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
