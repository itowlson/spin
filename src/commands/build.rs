use std::{ffi::OsString, path::PathBuf};

use anyhow::Result;
use clap::Parser;

use crate::opts::{APP_CONFIG_FILE_OPT, BUILD_UP_OPT, DEFAULT_MANIFEST_FILE};

use super::up::UpCommand;

/// Run the build command for each component.
#[derive(Parser, Debug)]
#[clap(about = "Build the Spin application", allow_hyphen_values = true)]
pub struct BuildCommand {
    /// Path to spin.toml.
    #[clap(
            name = APP_CONFIG_FILE_OPT,
            short = 'f',
            long = "file",
        )]
    pub app: Option<PathBuf>,

    /// Check that build prerequisites are installed but do not build.
    #[clap(conflicts_with = BUILD_UP_OPT, long = "check")]
    pub check: bool,

    /// Run the application after building.
    #[clap(name = BUILD_UP_OPT, short = 'u', long = "up")]
    pub up: bool,

    #[clap(requires = BUILD_UP_OPT)]
    pub up_args: Vec<OsString>,
}

impl BuildCommand {
    pub async fn run(self) -> Result<()> {
        let manifest_file = self
            .app
            .as_deref()
            .unwrap_or_else(|| DEFAULT_MANIFEST_FILE.as_ref());

        if self.check {
            spin_build::build_prerequisites(manifest_file).await?;
            return Ok(());
        }

        spin_build::build(manifest_file).await?;

        if self.up {
            let mut cmd = UpCommand::parse_from(
                std::iter::once(OsString::from(format!(
                    "{} up",
                    std::env::args().next().unwrap()
                )))
                .chain(self.up_args),
            );
            cmd.app = Some(manifest_file.into());
            cmd.run().await
        } else {
            Ok(())
        }
    }
}
