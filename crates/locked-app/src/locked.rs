//! Spin lock file (spin.lock) serialization models.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use spin_serde::FixedVersionBackwardCompatible;

use crate::{metadata::MetadataExt, values::ValuesMap};

/// A String-keyed map with deterministic serialization order.
pub type LockedMap<T> = std::collections::BTreeMap<String, T>;

/// A LockedApp represents a "fully resolved" Spin application.
#[derive(Clone, Debug, Deserialize)]
pub struct LockedApp {
    /// Locked schema version
    pub spin_lock_version: FixedVersionBackwardCompatible<1>,
    /// Identifies fields in the LockedApp that the host must process if present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub must_understand: Vec<String>,
    /// Application metadata
    #[serde(default, skip_serializing_if = "ValuesMap::is_empty")]
    pub metadata: ValuesMap,
    /// Application metadata
    #[serde(default, skip_serializing_if = "ValuesMap::is_empty")]
    pub host_requirements: ValuesMap,
    /// Custom config variables
    #[serde(default, skip_serializing_if = "LockedMap::is_empty")]
    pub variables: LockedMap<Variable>,
    /// Application triggers
    pub triggers: Vec<LockedTrigger>,
    /// Application components
    pub components: Vec<LockedComponent>,
}

impl Serialize for LockedApp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer {
        use serde::ser::SerializeStruct;

        let version = if self.must_understand.is_empty() && self.host_requirements.is_empty() {
            0
        } else {
            1
        };

        let mut la = serializer.serialize_struct("LockedApp", 7)?;
        la.serialize_field("spin_lock_version", &version)?;
        if !self.must_understand.is_empty() {
            la.serialize_field("must_understand", &self.must_understand)?;
        }
        if !self.metadata.is_empty() {
            la.serialize_field("metadata", &self.metadata)?;
        }
        if !self.host_requirements.is_empty() {
            la.serialize_field("host_requirements", &self.host_requirements)?;
        }
        if !self.variables.is_empty() {
            la.serialize_field("variables", &self.variables)?;
        }
        la.serialize_field("triggers", &self.triggers)?;
        la.serialize_field("components", &self.components)?;
        la.end()
    }
}

impl LockedApp {
    /// Deserializes a [`LockedApp`] from the given JSON data.
    pub fn from_json(contents: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(contents)
    }

    /// Serializes the [`LockedApp`] into JSON data.
    pub fn to_json(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(&self)
    }

    /// Deserializes typed metadata for this app.
    ///
    /// Returns `Ok(None)` if there is no metadata for the given `key` and an
    /// `Err` only if there _is_ a value for the `key` but the typed
    /// deserialization failed.
    pub fn get_metadata<'this, T: Deserialize<'this>>(
        &'this self,
        key: crate::MetadataKey<T>,
    ) -> crate::Result<Option<T>> {
        self.metadata.get_typed(key)
    }

    /// Deserializes typed metadata for this app.
    ///
    /// Like [`LockedApp::get_metadata`], but returns an error if there is
    /// no metadata for the given `key`.
    pub fn require_metadata<'this, T: Deserialize<'this>>(
        &'this self,
        key: crate::MetadataKey<T>,
    ) -> crate::Result<T> {
        self.metadata.require_typed(key)
    }
}

/// A LockedComponent represents a "fully resolved" Spin component.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockedComponent {
    /// Application-unique component identifier
    pub id: String,
    /// Component metadata
    #[serde(default, skip_serializing_if = "ValuesMap::is_empty")]
    pub metadata: ValuesMap,
    /// Wasm source
    pub source: LockedComponentSource,
    /// WASI environment variables
    #[serde(default, skip_serializing_if = "LockedMap::is_empty")]
    pub env: LockedMap<String>,
    /// WASI filesystem contents
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<ContentPath>,
    /// Custom config values
    #[serde(default, skip_serializing_if = "LockedMap::is_empty")]
    pub config: LockedMap<String>,
}

/// A LockedComponentSource specifies a Wasm source.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockedComponentSource {
    /// Wasm source content type (e.g. "application/wasm")
    pub content_type: String,
    /// Wasm source content specification
    #[serde(flatten)]
    pub content: ContentRef,
}

/// A ContentPath specifies content mapped to a WASI path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContentPath {
    /// Content specification
    #[serde(flatten)]
    pub content: ContentRef,
    /// WASI mount path
    pub path: PathBuf,
}

/// A ContentRef represents content used by an application.
///
/// At least one of `source` or `digest` must be specified. Implementations may
/// require one or the other (or both).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContentRef {
    /// A URI where the content can be accessed. Implementations may support
    /// different URI schemes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The content itself, base64-encoded.
    ///
    /// NOTE: This is both an optimization for small content and a workaround
    /// for certain OCI implementations that don't support 0 or 1 byte blobs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "spin_serde::base64"
    )]
    pub inline: Option<Vec<u8>>,
    /// If set, the content must have the given SHA-256 digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// A LockedTrigger specifies configuration for an application trigger.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LockedTrigger {
    /// Application-unique trigger identifier
    pub id: String,
    /// Trigger type (e.g. "http")
    pub trigger_type: String,
    /// Trigger-type-specific configuration
    pub trigger_config: Value,
}

/// A Variable specifies a custom configuration variable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variable {
    /// The variable's default value. If unset, the variable is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// If set, the variable's value may be sensitive and e.g. shouldn't be logged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

#[cfg(test)]
mod test {
    use crate::values::ValuesMapBuilder;

    use super::LockedApp;

    #[test]
    fn locked_app_with_no_host_reqs_serialises_as_v0_and_v0_deserialises_as_v1() {
        let locked_app = LockedApp {
            spin_lock_version: Default::default(),
            must_understand: Default::default(),
            metadata: Default::default(),
            host_requirements: Default::default(),
            variables: Default::default(),
            triggers: Default::default(),
            components: Default::default(),
        };

        let json = locked_app.to_json().unwrap();

        assert!(String::from_utf8_lossy(&json).contains(r#""spin_lock_version": 0"#));

        let reloaded = LockedApp::from_json(&json).unwrap();

        assert_eq!(1, Into::<usize>::into(reloaded.spin_lock_version));
    }

    #[test]
    fn locked_app_with_host_reqs_serialises_as_v1() {
        let mut host_requirements = ValuesMapBuilder::new();
        host_requirements.string("foo", "bar");
        let host_requirements = host_requirements.build();

        let locked_app = LockedApp {
            spin_lock_version: Default::default(),
            must_understand: vec!["host_requirements".to_owned()],
            metadata: Default::default(),
            host_requirements,
            variables: Default::default(),
            triggers: Default::default(),
            components: Default::default(),
        };

        let json = locked_app.to_json().unwrap();

        assert!(String::from_utf8_lossy(&json).contains(r#""spin_lock_version": 1"#));

        let reloaded = LockedApp::from_json(&json).unwrap();

        assert_eq!(1, Into::<usize>::into(reloaded.spin_lock_version));
        assert_eq!(1, reloaded.must_understand.len());
        assert_eq!(1, reloaded.host_requirements.len());
    }
}
