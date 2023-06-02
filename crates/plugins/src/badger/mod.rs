mod store;

use is_terminal::IsTerminal;
use crate::{manifest::PluginManifest};
use self::store::{BadgerRecordManager, PreviousBadger};

const BADGER_TIMEOUT_DAYS: i64 = 14;

pub async fn badger(name: String, current_version: String, spin_version: &'static str) -> anyhow::Result<BadgerUI2> {
    // There's no point doing the checks if nobody's around to see the results
    if !std::io::stderr().is_terminal() {
        return Ok(BadgerUI2::None);
    }

    let current_version = semver::Version::parse(&current_version)?;

    let checker = BadgerChecker::new(&name, &current_version, spin_version)?;

    let record_manager = BadgerRecordManager::default()?;
    let previous_badger = record_manager.previous_badger(&name, &current_version).await;

    let available_upgrades = checker.check(previous_badger).await?;

    if !available_upgrades.is_none() {
        record_manager.record_badger(&name, &current_version, &available_upgrades.list()).await
    };

    Ok(available_upgrades.classify())
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

    async fn check(&self, previous_badger: PreviousBadger) -> anyhow::Result<AvailableUpgrades> {
        let should_check = match previous_badger {
            PreviousBadger::Fresh => true,
            PreviousBadger::FromCurrent { when, .. } => has_timeout_expired(when),
        };

        if !should_check {
            return Ok(AvailableUpgrades::none());
        }

        let available_upgrades = self.get_available_upgrades().await?;

        if available_upgrades.is_none() {
            return Ok(AvailableUpgrades::none());
        }

        // TO CONSIDER: skipping this check and badgering for the same upgrade in case they missed it
        if previous_badger.includes_any(&available_upgrades.list()) {
            return Ok(AvailableUpgrades::none());
        }

        Ok(available_upgrades)
    }

    async fn get_available_upgrades(&self) -> anyhow::Result<AvailableUpgrades> {
        update().await?;

        // TODO: okay the manager probably needs to get plumbed in
        let manager = crate::manager::PluginManager::try_default()?;
        let store = manager.store();

        let latest_version = {
            let latest_lookup = crate::lookup::PluginLookup::new(&self.plugin_name, None);
            let latest_manifest = latest_lookup.get_manifest_from_repository(store.get_plugins_directory()).await.ok();
            latest_manifest.and_then(|m| semver::Version::parse(m.version()).ok())
        };

        let manifests = store.catalogue_manifests()?;
        let relevant_manifests = manifests.into_iter().filter(|m| m.name() == self.plugin_name);
        let compatible_manifests = relevant_manifests.filter(|m| m.has_compatible_package() && m.is_compatible_spin_version(self.spin_version));
        let compatible_plugin_versions = compatible_manifests.filter_map(|m| {
            PluginVersion::try_from(m, &latest_version)
        });
        let considerable_manifests = compatible_plugin_versions.filter(|pv| !pv.is_prerelease() && pv.is_higher_than(&self.current_version)).collect::<Vec<_>>();

        let (eligible_manifests, questionable_manifests) = if self.current_version.major == 0 {
            (vec![], considerable_manifests)
        } else {
            considerable_manifests.into_iter().partition(|pv| pv.version.major == self.current_version.major)
        };

        let highest_eligible_manifest = eligible_manifests.into_iter().max_by_key(|pv| pv.version.clone());
        let highest_questionable_manifest = questionable_manifests.into_iter().max_by_key(|pv| pv.version.clone());

        Ok(AvailableUpgrades {
            eligible: highest_eligible_manifest,
            questionable: highest_questionable_manifest,
        })
    }

    // fn compatible_with_current(&self, plugin_version: &PluginVersion) -> bool {
    //     let supported_on = &plugin_version.manifest.spin_compatibility;
    //     crate::manifest::is_version_compatible_enough(supported_on, self.spin_version).unwrap_or(true)
    // }
}

// TODO: this is the same as in the CLI command (except without output) - deduplicate
async fn update() -> anyhow::Result<()> {
    let manager = crate::manager::PluginManager::try_default()?;
    let plugins_dir = manager.store().get_plugins_directory();
    let url = crate::lookup::plugins_repo_url()?;
    crate::lookup::fetch_plugins_repo(&url, plugins_dir, true).await?;
    Ok(())
}

fn has_timeout_expired(from_time: chrono::DateTime<chrono::Utc>) -> bool {
    let timeout = chrono::Duration::days(BADGER_TIMEOUT_DAYS);
    let now = chrono::Utc::now();
    match now.checked_sub_signed(timeout) {
        None => true,
        Some(t) => from_time < t,
    }
}

pub struct AvailableUpgrades {
    eligible: Option<PluginVersion>,
    questionable: Option<PluginVersion>,
}

impl AvailableUpgrades {
    fn none() -> Self {
        Self { eligible: None, questionable: None }
    }

    fn is_none(&self) -> bool {
        self.eligible.is_none() && self.questionable.is_none()
    }

    fn classify(&self) -> BadgerUI2 {
        match (&self.eligible, &self.questionable) {
            (None, None) => BadgerUI2::None,
            (Some(e), None) => BadgerUI2::Eligible(e.clone()),
            (None, Some(q)) => BadgerUI2::Questionable(q.clone()),
            (Some(e), Some(q)) => BadgerUI2::Both { eligible: e.clone(), questionable: q.clone() },
        }
    }

    fn list(&self) -> Vec<&semver::Version> {
        [self.eligible.as_ref(), self.questionable.as_ref()].iter().filter_map(|pv| pv.as_ref()).map(|pv| &pv.version).collect()
    }
}

#[derive(Clone, Debug)]
pub struct PluginVersion {
    version: semver::Version,
    name: String,
    is_latest: bool,
}

impl PluginVersion {
    fn try_from(manifest: PluginManifest, latest: &Option<semver::Version>) -> Option<Self> {
        match semver::Version::parse(manifest.version()) {
            Ok(version) => {
                let name = manifest.name();
                let is_latest = match latest {
                    None => false,
                    Some(latest) => &version == latest,
                };
                Some(Self { version, name, is_latest })
            }
            Err(_) => None,
        }
    }

    fn is_prerelease(&self) -> bool {
        !self.version.pre.is_empty()
    }

    fn is_higher_than(&self, other: &semver::Version) -> bool {
        &self.version > other
    }

    pub fn upgrade_command(&self) -> String {
        if self.is_latest {
            format!("spin plugins upgrade {}", self.name)
        } else {
            format!("spin plugins upgrade {} -v {}", self.name, self.version)
        }
    }
}

impl std::fmt::Display for PluginVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}", self.version))
    }
}

// pub enum BadgerUI {
//     None,
//     BadgerEligible(semver::Version),
//     BadgerQuestionable(semver::Version),
// }

pub enum BadgerUI2 {
    None,
    Eligible(PluginVersion),
    Questionable(PluginVersion),
    Both { eligible: PluginVersion, questionable: PluginVersion },
}

// enum Eligibility {
//     Eligible,
//     Questionable,
//     Ineligible,
// }
