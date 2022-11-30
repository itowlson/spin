use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "spin_version")]
pub(crate) enum BuildAppInfoAnyVersion {
    #[serde(rename = "1")]
    V1(BuildAppInfoV1),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct BuildAppInfoV1 {
    #[serde(rename = "component")]
    pub components: Vec<RawComponentManifest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct RawComponentManifest {
    pub id: String,
    pub build: Option<RawBuildConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct RawBuildConfig {
    pub command: String,
    pub workdir: Option<PathBuf>,
    pub prerequisites: Option<IndexMap<String, RawBuildPrerequisite>>
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub(crate) struct RawBuildPrerequisite {
    pub command: String,
    // TODO: moar, much moar
    pub must_contain: Option<String>,
    pub message: String,
}

impl RawBuildPrerequisite {
    pub(crate) fn duplication_key(&self) -> String {
        format!("{}**{:?}", self.command, self.must_contain)
    }
}
