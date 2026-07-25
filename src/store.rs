use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Local, user-editable state: manual category overrides and personal notes.
/// Raw package data (description, homepage, version, ...) is never persisted
/// here - it is always re-fetched from `brew` on `scan` so it stays current.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub categories: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}

impl State {
    pub fn load() -> Result<Self> {
        Self::load_from(&state_path()?)
    }

    fn load_from(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(State::default());
        }
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read state file at {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("failed to parse state file at {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&state_path()?)
    }

    fn save_to(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create state directory {}", parent.display())
            })?;
        }
        let contents = toml::to_string_pretty(self).context("failed to serialize state")?;
        fs::write(path, contents)
            .with_context(|| format!("failed to write state file at {}", path.display()))
    }

    pub fn set_category(&mut self, name: &str, category: &str) {
        self.categories
            .insert(name.to_string(), category.to_string());
    }

    pub fn set_note(&mut self, name: &str, note: &str) {
        self.notes.insert(name.to_string(), note.to_string());
    }
}

/// Path to the local state file, e.g. `~/Library/Application Support/lagerregal/state.toml`
/// on macOS (via the `directories` crate, which follows each platform's conventions).
pub fn state_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "lagerregal")
        .context("could not determine a home directory for storing local state")?;
    Ok(dirs.data_dir().join("state.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_toml() {
        let mut state = State::default();
        state.set_category("nmap", "Security");
        state.set_note("nmap", "for CTF recon");

        let serialized = toml::to_string_pretty(&state).unwrap();
        let deserialized: State = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.categories.get("nmap").unwrap(), "Security");
        assert_eq!(deserialized.notes.get("nmap").unwrap(), "for CTF recon");
    }

    #[test]
    fn load_from_missing_path_returns_default() {
        let path = PathBuf::from("/nonexistent/path/that/should/not/exist/state.toml");
        let state = State::load_from(&path).unwrap();
        assert!(state.categories.is_empty());
        assert!(state.notes.is_empty());
    }

    #[test]
    fn save_then_load_from_temp_dir_roundtrips() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("lagerregal-test-{}", std::process::id()));
        let path = dir.join("state.toml");

        let mut state = State::default();
        state.set_category("bind", "DNS");
        state.save_to(&path).unwrap();

        let loaded = State::load_from(&path).unwrap();
        assert_eq!(loaded.categories.get("bind").unwrap(), "DNS");

        let _ = fs::remove_dir_all(&dir);
    }
}
