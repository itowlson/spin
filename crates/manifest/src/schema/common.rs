use serde::{Deserialize, Serialize};

/// Variable definition
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    /// `required = true`
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    /// `default = "default value"`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// `secret = true`
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
}

/// Component source
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum ComponentSource {
    /// `"local.wasm"`
    Local(String),
    /// `{ ... }`
    Remote {
        /// `url = "https://example.test/remote.wasm"`
        url: String,
        /// `digest = `"sha256:abc123..."`
        digest: String,
    },
}

/// Component source
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum WasiFilesMount {
    /// `"images/*.png"`
    Pattern(String),
    /// `{ ... }`
    Placement {
        /// `source = "content/dir"`
        source: String,
        /// `destination = "/"`
        destination: String,
    },
}

/// Component build configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentBuildConfig {
    /// `command = "cargo build"`
    pub command: String,
    /// `workdir = "components/main"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// watch = ["src/**/*.rs"]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<String>,
}

fn is_false(v: &bool) -> bool {
    !*v
}
