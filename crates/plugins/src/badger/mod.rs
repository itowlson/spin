mod store;

use is_terminal::IsTerminal;
use crate::{manifest::PluginManifest, lookup::PLUGINS_REPO_MANIFESTS_DIRECTORY};
use self::store::{BadgerRecordManager, PreviousBadger};

const BADGER_TIMEOUT_DAYS: i64 = 14;
const PLUGIN_MANIFESTS_BASE_URL: &str = "https://raw.githubusercontent.com/fermyon/spin-plugins";

pub async fn badger(name: String, current_version: String, spin_version: &'static str) -> anyhow::Result<BadgerUI> {
    // There's no point doing the checks if nobody's around to see the results
    if !std::io::stderr().is_terminal() {
        return Ok(BadgerUI::None);
    }

    let current_version = semver::Version::parse(&current_version)?;

    let checker = BadgerChecker::new(&name, &current_version, spin_version)?;

    let record_manager = BadgerRecordManager::default()?;
    let previous_badger = record_manager.previous_badger(&name, &current_version).await;

    let ui = checker.check(previous_badger).await?;

    match &ui {
        BadgerUI::BadgerEligible(to) | BadgerUI::BadgerQuestionable(to) =>
            record_manager.record_badger(&name, &current_version, to).await,
        _ => (),
    }

    Ok(ui)
}

struct BadgerChecker {
    plugin_name: String,
    current_version: semver::Version,
    spin_version: &'static str,
}

impl BadgerChecker {
    fn new(plugin_name: &str, current_version: &semver::Version, spin_version: &'static str) -> anyhow::Result<Self> {
        Ok(Self {
            plugin_name: plugin_name.to_owned(),
            current_version: current_version.clone(),
            spin_version,
        })
    }

    async fn check(&self, previous_badger: PreviousBadger) -> anyhow::Result<BadgerUI> {
        let should_check = match previous_badger {
            PreviousBadger::Fresh => true,
            PreviousBadger::FromCurrent { when, .. } => has_timeout_expired(when),
        };

        if !should_check {
            return Ok(BadgerUI::None);
        }

        let latest_version = self.get_latest_version().await?;

        if latest_version.version == self.current_version {
            return Ok(BadgerUI::None);
        }

        // TO CONSIDER: skipping this check and badgering for the same upgrade in case they missed it
        if let PreviousBadger::FromCurrent { to, .. } = previous_badger {
            if latest_version.version == to {
                return Ok(BadgerUI::None);
            }
        }

        let result = match self.eligible_upgrade(&latest_version) {
            Eligibility::Eligible => BadgerUI::BadgerEligible(latest_version.version),
            Eligibility::Questionable => BadgerUI::BadgerQuestionable(latest_version.version),
            Eligibility::Ineligible => BadgerUI::None,
        };

        Ok(result)
    }

    fn eligible_upgrade(&self, to: &PluginVersion) -> Eligibility {
        if !to.version.pre.is_empty() {
            Eligibility::Ineligible
        } else if !self.compatible_with_current(to) {
            // TODO: this could skip over an intermediate version which _is_ compatible!
            // We can only solve this by doing a full pull
            Eligibility::Ineligible
        } else if to.version.major == 0 {
            Eligibility::Questionable
        } else if to.version.major == self.current_version.major {
            Eligibility::Eligible
        } else {
            Eligibility::Questionable
        }
    }

    async fn get_latest_version(&self) -> anyhow::Result<PluginVersion> {
        let name = &self.plugin_name;

        // Example: https://raw.githubusercontent.com/fermyon/spin-plugins/main/manifests/py2wasm/py2wasm.json
        let url = format!("{PLUGIN_MANIFESTS_BASE_URL}/main/{PLUGINS_REPO_MANIFESTS_DIRECTORY}/{name}/{name}.json");
        let resp = reqwest::get(url).await?;
        if !resp.status().is_success() {
            anyhow::bail!("Error response downloading manifest from GitHub: status {}", resp.status());
        }
        let manifest: PluginManifest = resp.json().await?;
        let version = semver::Version::parse(manifest.version())?;
        Ok(PluginVersion { version, manifest })
    }

    fn compatible_with_current(&self, plugin_version: &PluginVersion) -> bool {
        let supported_on = &plugin_version.manifest.spin_compatibility;
        crate::manifest::is_version_compatible_enough(supported_on, self.spin_version).unwrap_or(true)
    }
}

fn has_timeout_expired(from_time: chrono::DateTime<chrono::Utc>) -> bool {
    let timeout = chrono::Duration::days(BADGER_TIMEOUT_DAYS);
    let now = chrono::Utc::now();
    match now.checked_sub_signed(timeout) {
        None => true,
        Some(t) => from_time < t,
    }
}

struct PluginVersion {
    version: semver::Version,
    manifest: PluginManifest,
}

pub enum BadgerUI {
    None,
    BadgerEligible(semver::Version),
    BadgerQuestionable(semver::Version),
}

enum Eligibility {
    Eligible,
    Questionable,
    Ineligible,
}
