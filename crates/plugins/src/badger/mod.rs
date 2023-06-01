use std::{path::PathBuf};

use is_terminal::IsTerminal;
use serde::{Serialize, Deserialize};

use crate::manifest::PluginManifest;

pub async fn badger(name: String, current_version: String) -> anyhow::Result<BadgerUI> {
    // There's no point doing the checks if nobody's around to see the results
    if !std::io::stderr().is_terminal() {
        return Ok(BadgerUI::None);
    }

    let current_version = semver::Version::parse(&current_version)?;

    let record_manager = BadgerRecordManager::default();
    let last_badger = record_manager.last_badger(&name).await;

    let ui = eval_badger(&name, &current_version, last_badger).await?;

    match &ui {
        BadgerUI::BadgerEligible(to) | BadgerUI::BadgerQuestionable(to) =>
            record_manager.record_badger(&name, &current_version, to).await,
        _ => (),
    }

    Ok(ui)
}

async fn eval_badger(name: &str, current_version: &semver::Version, last_badger: Option<BadgerRecord>) -> anyhow::Result<BadgerUI> {
    let badgeriness = match last_badger {
        Some(b) if &b.badgered_from == current_version => BadgerEval::FromCurrent { to: b.badgered_to, when: b.when },
        _ => BadgerEval::Fresh,
    };

    let should_check = match badgeriness {
        BadgerEval::Fresh => true,
        BadgerEval::FromCurrent { when, .. } => has_timeout_expired(when),
    };

    if !should_check {
        return Ok(BadgerUI::None);
    }

    let latest_version = get_latest_version(name).await?;

    if &latest_version == current_version {
        return Ok(BadgerUI::None);
    }

    // TO CONSIDER: skipping this check and badgering for the same upgrade in case they missed it
    if let BadgerEval::FromCurrent { to, .. } = badgeriness {
        if latest_version == to {
            return Ok(BadgerUI::None);
        }
    }

    let result = match eligible_upgrade(&current_version, &latest_version) {
        Eligibility::Eligible => BadgerUI::BadgerEligible(latest_version),
        Eligibility::Questionable => BadgerUI::BadgerQuestionable(latest_version),
        Eligibility::Ineligible => BadgerUI::None,
    };

    Ok(result)

    // What was the last badgering incident for this plugin?
    // If the incident involved badgering FROM the CURRENT INSTALLED version:
    //   If we are within the badger timeout, do nothing.
    //   Otherwise:
    //     Check for the most recent eligible and questionable versions.
    //     If the user has already been badgered about these, do nothing.  [Alternate proposal is to badger anyway.]
    //     If either of these is different from the current installed version, COMMENCE BADGERING.
    //     Otherwise, do nothing.
    // If the incident involved badgering from a DIFFERENT version:
    //   Check for the most recent eligible and questionable versions.
    //   If either of these is different from the current installed version, COMMENCE BADGERING.
    //   Otherwise, do nothing.
    // If the user has NEVER been badgered for this plugin:
    //   Continue as per "if the incident involved badgering from a different version."
}

const BADGER_TIMEOUT_DAYS: i64 = 14;

fn has_timeout_expired(from_time: chrono::DateTime<chrono::Utc>) -> bool {
    let timeout = chrono::Duration::days(BADGER_TIMEOUT_DAYS);
    let now = chrono::Utc::now();
    match now.checked_sub_signed(timeout) {
        None => true,
        Some(t) => from_time < t,
    }
}

fn eligible_upgrade(from: &semver::Version, to: &semver::Version) -> Eligibility {
    // TODO: check that the Spin version is compatible!
    if !to.pre.is_empty() {
        Eligibility::Ineligible
    } else if to.major == 0 {
        Eligibility::Questionable
    } else if to.major == from.major {
        Eligibility::Eligible
    } else {
        Eligibility::Questionable
    }
}

async fn get_latest_version(name: &str) -> anyhow::Result<semver::Version> {
    // Example: https://raw.githubusercontent.com/fermyon/spin-plugins/main/manifests/py2wasm/py2wasm.json
    let url = format!("https://raw.githubusercontent.com/fermyon/spin-plugins/main/{}/{name}/{name}.json", crate::lookup::PLUGINS_REPO_MANIFESTS_DIRECTORY);
    let resp = reqwest::get(url).await?;
    if !resp.status().is_success() {
        anyhow::bail!("Error response downloading manifest from GitHub: status {}", resp.status());
    }
    let body: PluginManifest = resp.json().await?;
    let version = semver::Version::parse(body.version())?;
    Ok(version)
}

struct BadgerRecordManager {
    db_path: PathBuf,
}

enum BadgerEval {
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
