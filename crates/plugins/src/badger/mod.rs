use std::{path::PathBuf};

use is_terminal::IsTerminal;
use serde::{Serialize, Deserialize};

use crate::manifest::PluginManifest;

const BADGER_TIMEOUT_DAYS: i64 = 14;

pub async fn badger(name: String, current_version: String, spin_version: &'static str) -> anyhow::Result<BadgerUI> {
    // There's no point doing the checks if nobody's around to see the results
    if !std::io::stderr().is_terminal() {
        return Ok(BadgerUI::None);
    }

    let current_version = semver::Version::parse(&current_version)?;

    let record_manager = BadgerRecordManager::default();
    let last_badger = record_manager.last_badger(&name).await;

    let ui = eval_badger(&name, &current_version, last_badger, spin_version).await?;

    match &ui {
        BadgerUI::BadgerEligible(to) | BadgerUI::BadgerQuestionable(to) =>
            record_manager.record_badger(&name, &current_version, to).await,
        _ => (),
    }

    Ok(ui)
}

async fn eval_badger(name: &str, current_version: &semver::Version, last_badger: Option<BadgerRecord>, spin_version: &str) -> anyhow::Result<BadgerUI> {
    let previous_badger = match last_badger {
        Some(b) if &b.badgered_from == current_version => PreviousBadger::FromCurrent { to: b.badgered_to, when: b.when },
        _ => PreviousBadger::Fresh,
    };

    let should_check = match previous_badger {
        PreviousBadger::Fresh => true,
        PreviousBadger::FromCurrent { when, .. } => has_timeout_expired(when),
    };

    if !should_check {
        return Ok(BadgerUI::None);
    }

    let latest_version = get_latest_version(name).await?;

    if &latest_version.version == current_version {
        return Ok(BadgerUI::None);
    }

    // TO CONSIDER: skipping this check and badgering for the same upgrade in case they missed it
    if let PreviousBadger::FromCurrent { to, .. } = previous_badger {
        if latest_version.version == to {
            return Ok(BadgerUI::None);
        }
    }

    let result = match eligible_upgrade(&current_version, &latest_version, spin_version) {
        Eligibility::Eligible => BadgerUI::BadgerEligible(latest_version.version),
        Eligibility::Questionable => BadgerUI::BadgerQuestionable(latest_version.version),
        Eligibility::Ineligible => BadgerUI::None,
    };

    Ok(result)
}

fn has_timeout_expired(from_time: chrono::DateTime<chrono::Utc>) -> bool {
    let timeout = chrono::Duration::days(BADGER_TIMEOUT_DAYS);
    let now = chrono::Utc::now();
    match now.checked_sub_signed(timeout) {
        None => true,
        Some(t) => from_time < t,
    }
}

fn eligible_upgrade(from: &semver::Version, to: &PluginVersion, spin_version: &str) -> Eligibility {
    if !to.version.pre.is_empty() {
        Eligibility::Ineligible
    } else if incompatible_with_current_spin(&to.manifest.spin_compatibility, spin_version) {
        // TODO: this could skip over an intermediate version which _is_ compatible!
        // We can only solve this by doing a full pull
        Eligibility::Ineligible
    } else if to.version.major == 0 {
        Eligibility::Questionable
    } else if to.version.major == from.major {
        Eligibility::Eligible
    } else {
        Eligibility::Questionable
    }
}

fn incompatible_with_current_spin(supported_on: &str, spin_version: &str) -> bool {
    crate::manifest::is_version_compatible_enough(supported_on, spin_version).unwrap_or(true)
}

async fn get_latest_version(name: &str) -> anyhow::Result<PluginVersion> {
    // Example: https://raw.githubusercontent.com/fermyon/spin-plugins/main/manifests/py2wasm/py2wasm.json
    let url = format!("https://raw.githubusercontent.com/fermyon/spin-plugins/main/{}/{name}/{name}.json", crate::lookup::PLUGINS_REPO_MANIFESTS_DIRECTORY);
    let resp = reqwest::get(url).await?;
    if !resp.status().is_success() {
        anyhow::bail!("Error response downloading manifest from GitHub: status {}", resp.status());
    }
    let manifest: PluginManifest = resp.json().await?;
    let version = semver::Version::parse(manifest.version())?;
    Ok(PluginVersion { version, manifest })
}

struct PluginVersion {
    version: semver::Version,
    manifest: PluginManifest,
}

struct BadgerRecordManager {
    db_path: PathBuf,
}

enum PreviousBadger {
    Fresh,
    FromCurrent { to: semver::Version, when: chrono::DateTime<chrono::Utc> },
}

impl BadgerRecordManager {
    fn default() -> Self {
        let base_dir = dirs::cache_dir().unwrap_or(PathBuf::from("./SPINDLYWINDLY"));
        let db_path = base_dir.join("spin").join("badger.json");
        Self { db_path }
    }

    fn load(&self) -> Vec<BadgerRecord> {
        match std::fs::read(&self.db_path) {
            Ok(v) => serde_json::from_slice(&v).unwrap_or_default(),
            Err(_) => vec![],
        }
    }

    fn save(&self, records: Vec<BadgerRecord>) -> anyhow::Result<()> {
        if let Some(dir) = self.db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&records)?;
        std::fs::write(&self.db_path, &json)?;
        Ok(())
    }

    async fn last_badger(&self, name: &str) -> Option<BadgerRecord> {
        self.load()
            .into_iter()
            .find(|r| r.name == name)
    }

    async fn record_badger(&self, name: &str, from: &semver::Version, to: &semver::Version) {
        let new = BadgerRecord {
            name: name.to_owned(),
            badgered_from: from.clone(),
            badgered_to: to.clone(),
            when: chrono::Utc::now(),
        };

        let mut existing = self.load();
        match existing.iter().position(|r| r.name == name) {
            Some(index) => existing[index] = new,
            None => existing.push(new),
        };
        _ = self.save(existing);
    }
}

pub enum BadgerUI {
    None,
    BadgerEligible(semver::Version),
    BadgerQuestionable(semver::Version),
}

#[derive(Serialize, Deserialize)]
struct BadgerRecord {
    name: String,
    badgered_from: semver::Version,
    badgered_to: semver::Version,
    when: chrono::DateTime<chrono::Utc>,
}

enum Eligibility {
    Eligible,
    Questionable,
    Ineligible,
}
