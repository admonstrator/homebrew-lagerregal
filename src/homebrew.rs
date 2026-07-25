use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    Formula,
    Cask,
}

impl PackageKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PackageKind::Formula => "formula",
            PackageKind::Cask => "cask",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub kind: PackageKind,
    pub desc: String,
    pub homepage: String,
    pub tap: String,
    pub version: String,
    pub installed_on_request: bool,
}

#[derive(Debug, Deserialize)]
struct BrewInfo {
    #[serde(default)]
    formulae: Vec<RawFormula>,
    #[serde(default)]
    casks: Vec<RawCask>,
}

#[derive(Debug, Deserialize)]
struct RawFormula {
    name: String,
    #[serde(default)]
    tap: String,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    installed: Vec<RawFormulaInstall>,
}

#[derive(Debug, Deserialize)]
struct RawFormulaInstall {
    #[serde(default)]
    version: String,
    #[serde(default)]
    installed_on_request: bool,
}

#[derive(Debug, Deserialize)]
struct RawCask {
    token: String,
    #[serde(default)]
    tap: Option<String>,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    installed: Option<String>,
}

/// Runs `brew info --json=v2 --installed` and parses the result.
pub fn installed_packages() -> Result<Vec<Package>> {
    let output = Command::new("brew")
        .args(["info", "--json=v2", "--installed"])
        .output()
        .context("failed to run `brew` - is Homebrew installed and on PATH?")?;

    if !output.status.success() {
        bail!(
            "`brew info --json=v2 --installed` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    parse_brew_json(&output.stdout)
}

/// Parses the JSON produced by `brew info --json=v2 --installed` into normalized packages.
/// Kept separate from `installed_packages` so it can be unit-tested against fixture files
/// without requiring a real `brew` binary.
pub fn parse_brew_json(bytes: &[u8]) -> Result<Vec<Package>> {
    let info: BrewInfo =
        serde_json::from_slice(bytes).context("failed to parse brew JSON output")?;

    let mut packages = Vec::with_capacity(info.formulae.len() + info.casks.len());

    for f in info.formulae {
        let install = f.installed.first();
        packages.push(Package {
            name: f.name,
            kind: PackageKind::Formula,
            desc: f.desc.unwrap_or_default(),
            homepage: f.homepage.unwrap_or_default(),
            tap: f.tap,
            version: install.map(|i| i.version.clone()).unwrap_or_default(),
            installed_on_request: install.map(|i| i.installed_on_request).unwrap_or(false),
        });
    }

    for c in info.casks {
        packages.push(Package {
            name: c.token,
            kind: PackageKind::Cask,
            desc: c.desc.unwrap_or_default(),
            homepage: c.homepage.unwrap_or_default(),
            tap: c.tap.unwrap_or_default(),
            version: c.installed.unwrap_or_default(),
            // Casks have no "dependency vs. on-request" distinction - they are always explicit.
            installed_on_request: true,
        });
    }

    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_formulae_and_casks() {
        let fixture = include_bytes!("../tests/fixtures/brew_installed.json");
        let packages = parse_brew_json(fixture).expect("fixture should parse");

        assert!(packages
            .iter()
            .any(|p| p.name == "nmap" && p.kind == PackageKind::Formula));
        assert!(packages
            .iter()
            .any(|p| p.name == "wireshark" && p.kind == PackageKind::Cask));

        let nmap = packages.iter().find(|p| p.name == "nmap").unwrap();
        assert_eq!(nmap.tap, "homebrew/core");
        assert!(nmap.desc.to_lowercase().contains("network"));
    }
}
