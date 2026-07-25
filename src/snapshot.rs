use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::classify::ClassifiedPackage;
use crate::store;

/// A point-in-time record of installed package names and their versions,
/// so later runs can be compared against it to see what changed.
#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub taken_at: i64,
    pub packages: BTreeMap<String, String>,
}

impl Snapshot {
    /// Captures the given (already scoped/filtered) packages as a snapshot.
    pub fn capture(classified: &[ClassifiedPackage]) -> Self {
        let taken_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let packages = classified
            .iter()
            .map(|p| (p.package.name.clone(), p.package.version.clone()))
            .collect();
        Snapshot { taken_at, packages }
    }

    pub fn save(&self, name: &str) -> Result<()> {
        let path = snapshot_path(name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create snapshot directory {}", parent.display())
            })?;
        }
        let contents = toml::to_string_pretty(self).context("failed to serialize snapshot")?;
        fs::write(&path, contents)
            .with_context(|| format!("failed to write snapshot file at {}", path.display()))
    }

    pub fn load(name: &str) -> Result<Option<Self>> {
        let path = snapshot_path(name)?;
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read snapshot file at {}", path.display()))?;
        let snapshot = toml::from_str(&contents)
            .with_context(|| format!("failed to parse snapshot file at {}", path.display()))?;
        Ok(Some(snapshot))
    }
}

/// Path to a named snapshot file: `<data_dir>/snapshots/<name>.toml`.
pub fn snapshot_path(name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("invalid snapshot name \"{name}\" - use only letters, numbers, '-' and '_'");
    }
    Ok(store::data_dir()?
        .join("snapshots")
        .join(format!("{name}.toml")))
}

/// The difference between a saved snapshot and the current set of installed
/// packages: what's new, what's gone, and what changed version. Sorted by
/// name so output is stable and easy to scan.
pub struct Diff {
    pub added: Vec<(String, String)>,
    pub removed: Vec<(String, String)>,
    pub changed: Vec<(String, String, String)>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

pub fn diff(old: &Snapshot, current: &BTreeMap<String, String>) -> Diff {
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (name, version) in current {
        match old.packages.get(name) {
            None => added.push((name.clone(), version.clone())),
            Some(old_version) if old_version != version => {
                changed.push((name.clone(), old_version.clone(), version.clone()))
            }
            Some(_) => {}
        }
    }
    let mut removed: Vec<(String, String)> = old
        .packages
        .iter()
        .filter(|(name, _)| !current.contains_key(*name))
        .map(|(name, version)| (name.clone(), version.clone()))
        .collect();

    added.sort();
    removed.sort();
    changed.sort();
    Diff {
        added,
        removed,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packages(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn diff_detects_added_removed_and_changed() {
        let old = Snapshot {
            taken_at: 0,
            packages: packages(&[("nmap", "7.94"), ("wireshark", "4.4.0"), ("stable", "1.0")]),
        };
        let current = packages(&[("nmap", "7.99"), ("stable", "1.0"), ("jq", "1.7")]);

        let diff = diff(&old, &current);
        assert_eq!(diff.added, vec![("jq".to_string(), "1.7".to_string())]);
        assert_eq!(
            diff.removed,
            vec![("wireshark".to_string(), "4.4.0".to_string())]
        );
        assert_eq!(
            diff.changed,
            vec![("nmap".to_string(), "7.94".to_string(), "7.99".to_string())]
        );
        assert!(!diff.is_empty());
    }

    #[test]
    fn diff_is_empty_when_nothing_changed() {
        let old = Snapshot {
            taken_at: 0,
            packages: packages(&[("nmap", "7.94")]),
        };
        let current = packages(&[("nmap", "7.94")]);
        assert!(diff(&old, &current).is_empty());
    }

    #[test]
    fn rejects_snapshot_names_with_path_separators() {
        assert!(snapshot_path("../evil").is_err());
        assert!(snapshot_path("a/b").is_err());
        assert!(snapshot_path("").is_err());
        assert!(snapshot_path("before-reinstall_2").is_ok());
    }
}
