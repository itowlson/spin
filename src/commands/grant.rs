use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context};
use clap::Parser;
use itertools::Itertools;
use spin_common::ui::quoted_path;
use spin_manifest::{
    schema::v2::{AppManifest, KebabId},
    ManifestVersion,
};
use tokio;

use crate::{directory_rels::notify_if_nondefault_rel, opts::APP_MANIFEST_FILE_OPT};

/// Grant a component access to a resource.
#[derive(Parser, Debug)]
pub struct GrantCommand {
    /// Path to spin.toml.
    #[clap(
        name = APP_MANIFEST_FILE_OPT,
        short = 'f',
        long = "file",
    )]
    pub app: Option<PathBuf>,
}

impl GrantCommand {
    pub async fn run(&self) -> anyhow::Result<()> {
        if !std::io::stderr().is_terminal() {
            bail!("this command is interactive only");
        }

        let (manifest_file, distance) =
            spin_common::paths::find_manifest_file_path(self.app.as_ref())?;
        notify_if_nondefault_rel(&manifest_file, distance);

        let manifest = load_manifest(&manifest_file).await?;

        let components = manifest.components.keys().sorted().collect_vec();
        if components.is_empty() {
            bail!("no comps in manifest");
        }
        let Some(component_id) = select_component(&components) else {
            return Ok(());
        };

        let resource_kinds = [
            ResourceKind::Host,
            ResourceKind::KeyValue,
            ResourceKind::Sqlite,
        ];
        let Some(resource_kind) = select_item(
            "What kind of resource do you want the component to have access to?",
            &resource_kinds,
        ) else {
            return Ok(());
        };

        let value: String = dialoguer::Input::new()
            .with_prompt("What thingy?")
            .interact_text()
            .unwrap();

        add_permission_to_manifest(&manifest_file, component_id, resource_kind, value).await?;

        Ok(())
    }
}

async fn load_manifest(manifest_path: &Path) -> anyhow::Result<AppManifest> {
    if !manifest_path.exists() {
        bail!("file {} does not exist", quoted_path(manifest_path));
    }

    let manifest_text = tokio::fs::read_to_string(manifest_path)
        .await
        .with_context(|| format!("Can't read manifest from {}", quoted_path(&manifest_path)))?;

    let ManifestVersion::V2 = ManifestVersion::detect(&manifest_text)? else {
        bail!("only v2 manifest supported");
    };

    toml::from_str(&manifest_text)
        .with_context(|| format!("invalid manifest {}", quoted_path(&manifest_path)))
}

async fn add_permission_to_manifest(
    manifest_file: &Path,
    component_id: &KebabId,
    resource_kind: &ResourceKind,
    value: String,
) -> Result<(), anyhow::Error> {
    let mut edit: toml_edit::DocumentMut =
        tokio::fs::read_to_string(manifest_file).await?.parse()?;

    let Some(component_table) = edit
        .get_mut("component")
        .and_then(|cs| cs.get_mut(component_id.as_ref()))
        .and_then(|item| item.as_table_mut())
    else {
        bail!("wait!  That component is not, after all, bussing");
    };

    let list = edit_resource_list(component_table, resource_kind)?;
    list.push(value);

    tokio::fs::write(manifest_file, edit.to_string())
        .await
        .with_context(|| format!("failed to save changes to {}", quoted_path(manifest_file)))?;

    Ok(())
}

fn edit_resource_list<'a>(
    component_table: &'a mut toml_edit::Table,
    resource_kind: &ResourceKind,
) -> anyhow::Result<&'a mut toml_edit::Array> {
    let resource_key = resource_kind.manifest_key();

    if component_table.get(resource_key).is_none() {
        component_table.insert(resource_key, toml_edit::value(toml_edit::Array::new()));
    }

    component_table
        .get_mut(resource_key)
        .unwrap()
        .as_array_mut()
        .ok_or_else(|| anyhow!("this list is not a list"))
}

fn select_component<'a>(components: &'a [&'a KebabId]) -> Option<&'a &'a KebabId> {
    let index = dialoguer::Select::new()
        .with_prompt("Which component do you want to add a permission to?")
        .items(components)
        .default(0)
        .interact_opt()
        .unwrap();
    index.and_then(|i| components.get(i))
}

fn select_item<'a, T: ToString>(prompt: &str, items: &'a [T]) -> Option<&'a T> {
    let index = dialoguer::Select::new()
        .with_prompt(prompt)
        .items(items)
        .interact_opt()
        .unwrap();
    index.and_then(|i| items.get(i))
}

enum ResourceKind {
    Host,
    KeyValue,
    Sqlite,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Host => "Network host",
            Self::KeyValue => "Key-value store",
            Self::Sqlite => "SQLite database",
        };
        f.write_str(text)
    }
}

impl ResourceKind {
    fn manifest_key(&self) -> &str {
        match self {
            Self::Host => "allowed_outbound_hosts",
            Self::KeyValue => "key_value_stores",
            Self::Sqlite => "sqlite_databases",
        }
    }
}
