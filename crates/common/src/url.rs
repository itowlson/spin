//! Operations on URLs

use std::path::PathBuf;

use anyhow::{anyhow, Context};

/// Parses a "file:" URL
pub fn parse_file_url(url: &str) -> anyhow::Result<PathBuf> {
    url::Url::parse(url)
        .with_context(|| format!("Invalid URL: {url:?}"))?
        .to_file_path()
        .map_err(|_| anyhow!("Invalid file URL path: {url:?}"))
}
