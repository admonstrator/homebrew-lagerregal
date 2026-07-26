use std::collections::BTreeMap;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub kind: PackageKind,
    pub desc: String,
    pub homepage: String,
    pub tap: String,
    pub version: String,
    pub installed_on_request: bool,
    /// Unix timestamp of when this package was installed, if known.
    pub installed_at: Option<i64>,
    /// Names of this package's direct (declared) dependencies, resolved
    /// against other installed packages to build a dependency tree.
    pub dependencies: Vec<String>,
    /// Runtime dependencies recorded in the install receipt that were *not*
    /// declared directly - the transitive tail. Not shown in the dependency
    /// tree (which walks direct edges), but essential for orphan analysis:
    /// a package can be needed through an edge that exists only here, e.g.
    /// when the formula that declared it directly was since uninstalled.
    #[serde(default)]
    pub indirect_dependencies: Vec<String>,
    /// The newer version available, if `brew outdated` reports one. `None`
    /// either means the package is up to date, or `apply_outdated` was never
    /// called (outdated-ness is opt-in - it needs its own `brew` call).
    pub outdated: Option<String>,
    /// Whether Homebrew itself has marked this package deprecated (slated
    /// for removal) or disabled (already uninstallable fresh). Both read as
    /// "no longer maintained" to an end user, so they're folded into one
    /// flag; `unmaintained_reason` prefers the disable reason when both are
    /// set, since disabled is the more severe/final state.
    pub unmaintained: bool,
    pub unmaintained_reason: Option<String>,
    /// Whether the user has run `brew pin` on this formula, freezing it at
    /// its current version. Casks can't be pinned, so this is always
    /// `false` for them. Defaults on deserialize so caches written before
    /// the field existed still parse.
    #[serde(default)]
    pub pinned: bool,
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
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    deprecation_reason: Option<String>,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    disable_reason: Option<String>,
    #[serde(default)]
    pinned: bool,
}

#[derive(Debug, Deserialize)]
struct RawFormulaInstall {
    #[serde(default)]
    version: String,
    #[serde(default)]
    installed_on_request: bool,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    runtime_dependencies: Vec<RawRuntimeDependency>,
}

#[derive(Debug, Deserialize)]
struct RawRuntimeDependency {
    full_name: String,
    #[serde(default)]
    declared_directly: bool,
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
    #[serde(default)]
    installed_time: Option<i64>,
    #[serde(default)]
    depends_on: Option<RawCaskDependsOn>,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    deprecation_reason: Option<String>,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    disable_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawCaskDependsOn {
    #[serde(default)]
    formula: Vec<String>,
    #[serde(default)]
    cask: Vec<String>,
}

/// Homebrew's `full_name` for a runtime dependency is tap-qualified for
/// third-party taps (e.g. `user/tap/formula`) but bare for homebrew/core
/// (e.g. `liblinear`). Our own `Package::name` is always bare, so strip any
/// tap prefix to make dependency names resolvable against it.
fn short_name(full_name: &str) -> String {
    full_name
        .rsplit('/')
        .next()
        .unwrap_or(full_name)
        .to_string()
}

/// Combines Homebrew's `deprecated`/`disabled` flags (and their reasons)
/// into the single "unmaintained" concept end users think in terms of.
/// Disabled wins over deprecated when both are set, since a formula/cask is
/// often marked disabled some time after being deprecated - it's the more
/// current, more severe reason.
fn unmaintained_status(
    deprecated: bool,
    deprecation_reason: Option<String>,
    disabled: bool,
    disable_reason: Option<String>,
) -> (bool, Option<String>) {
    if disabled {
        (true, disable_reason.or(Some("disabled".to_string())))
    } else if deprecated {
        (true, deprecation_reason.or(Some("deprecated".to_string())))
    } else {
        (false, None)
    }
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
        let (dependencies, indirect_dependencies) = install
            .map(|i| {
                let (direct, indirect): (Vec<_>, Vec<_>) = i
                    .runtime_dependencies
                    .iter()
                    .partition(|d| d.declared_directly);
                (
                    direct.iter().map(|d| short_name(&d.full_name)).collect(),
                    indirect.iter().map(|d| short_name(&d.full_name)).collect(),
                )
            })
            .unwrap_or_default();
        let (unmaintained, unmaintained_reason) = unmaintained_status(
            f.deprecated,
            f.deprecation_reason,
            f.disabled,
            f.disable_reason,
        );
        packages.push(Package {
            name: f.name,
            kind: PackageKind::Formula,
            desc: f.desc.unwrap_or_default(),
            homepage: f.homepage.unwrap_or_default(),
            tap: f.tap,
            version: install.map(|i| i.version.clone()).unwrap_or_default(),
            installed_on_request: install.map(|i| i.installed_on_request).unwrap_or(false),
            installed_at: install.and_then(|i| i.time),
            dependencies,
            indirect_dependencies,
            outdated: None,
            unmaintained,
            unmaintained_reason,
            pinned: f.pinned,
        });
    }

    for c in info.casks {
        let dependencies = c
            .depends_on
            .map(|d| d.formula.into_iter().chain(d.cask).collect())
            .unwrap_or_default();
        let (unmaintained, unmaintained_reason) = unmaintained_status(
            c.deprecated,
            c.deprecation_reason,
            c.disabled,
            c.disable_reason,
        );
        packages.push(Package {
            name: c.token,
            kind: PackageKind::Cask,
            desc: c.desc.unwrap_or_default(),
            homepage: c.homepage.unwrap_or_default(),
            tap: c.tap.unwrap_or_default(),
            version: c.installed.unwrap_or_default(),
            // Casks have no "dependency vs. on-request" distinction - they are always explicit.
            installed_on_request: true,
            installed_at: c.installed_time,
            dependencies,
            indirect_dependencies: Vec::new(),
            outdated: None,
            unmaintained,
            unmaintained_reason,
            // Casks can't be pinned.
            pinned: false,
        });
    }

    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(packages)
}

#[derive(Debug, Deserialize)]
struct OutdatedInfo {
    #[serde(default)]
    formulae: Vec<RawOutdatedEntry>,
    #[serde(default)]
    casks: Vec<RawOutdatedEntry>,
}

#[derive(Debug, Deserialize)]
struct RawOutdatedEntry {
    name: String,
    current_version: String,
}

/// Runs `brew outdated --json=v2` and returns a map of installed package
/// name to the newer version available. `HOMEBREW_NO_AUTO_UPDATE=1` is set
/// because plain `brew outdated` otherwise triggers an implicit `brew
/// update` (a network fetch of the tap repositories) - this tool is meant to
/// work entirely off the locally installed state, so that has to be opt-in,
/// not a side effect of asking what's outdated.
pub fn outdated_versions() -> Result<BTreeMap<String, String>> {
    let output = Command::new("brew")
        .args(["outdated", "--json=v2"])
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .output()
        .context("failed to run `brew` - is Homebrew installed and on PATH?")?;

    if !output.status.success() {
        bail!(
            "`brew outdated --json=v2` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    parse_outdated_json(&output.stdout)
}

/// Parses the JSON produced by `brew outdated --json=v2`. Kept separate from
/// `outdated_versions` so it can be unit-tested without a real `brew` binary.
pub fn parse_outdated_json(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let info: OutdatedInfo =
        serde_json::from_slice(bytes).context("failed to parse `brew outdated` JSON output")?;
    Ok(info
        .formulae
        .into_iter()
        .chain(info.casks)
        .map(|e| (short_name(&e.name), e.current_version))
        .collect())
}

/// Applies the result of `outdated_versions` onto a package list in place.
pub fn apply_outdated(packages: &mut [Package], outdated: &BTreeMap<String, String>) {
    for pkg in packages.iter_mut() {
        pkg.outdated = outdated.get(&pkg.name).cloned();
    }
}

/// Runs `brew upgrade <name>` (or `brew upgrade --cask <name>`) with stdio
/// inherited from the calling process, so the caller sees `brew`'s own
/// progress/download/build output live rather than it being captured and
/// needing to be re-rendered. Returns once the upgrade finishes; callers
/// should treat a non-success exit status as "the upgrade failed" rather
/// than an error to propagate, since `brew`'s own output already explains
/// why.
/// Runs `brew pin` or `brew unpin` for a formula. Unlike `upgrade`, output
/// is captured rather than inherited - pinning is an instant metadata
/// operation with nothing worth watching, and the TUI stays on screen.
pub fn set_pinned(name: &str, pinned: bool) -> Result<()> {
    let sub = if pinned { "pin" } else { "unpin" };
    let output = Command::new("brew")
        .arg(sub)
        .arg(name)
        .output()
        .context("failed to run `brew` - is Homebrew installed and on PATH?")?;
    if !output.status.success() {
        anyhow::bail!(
            "brew {sub} {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn upgrade(kind: PackageKind, name: &str) -> Result<std::process::ExitStatus> {
    run_inherited(kind, "upgrade", name)
}

/// Runs `brew uninstall` for a single package, with stdio inherited like
/// `upgrade` - removal prints its own confirmation of what was deleted, and
/// its own refusal when other installed packages still depend on the target.
pub fn uninstall(kind: PackageKind, name: &str) -> Result<std::process::ExitStatus> {
    run_inherited(kind, "uninstall", name)
}

fn run_inherited(kind: PackageKind, sub: &str, name: &str) -> Result<std::process::ExitStatus> {
    let mut cmd = Command::new("brew");
    cmd.arg(sub);
    if kind == PackageKind::Cask {
        cmd.arg("--cask");
    }
    cmd.arg(name);
    cmd.status()
        .context("failed to run `brew` - is Homebrew installed and on PATH?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_formulae_and_casks() {
        let fixture = include_bytes!("../tests/fixtures/brew_installed.json");
        let packages = parse_brew_json(fixture).expect("fixture should parse");

        assert!(
            packages
                .iter()
                .any(|p| p.name == "nmap" && p.kind == PackageKind::Formula)
        );
        assert!(
            packages
                .iter()
                .any(|p| p.name == "wireshark" && p.kind == PackageKind::Cask)
        );

        let nmap = packages.iter().find(|p| p.name == "nmap").unwrap();
        assert_eq!(nmap.tap, "homebrew/core");
        assert!(nmap.desc.to_lowercase().contains("network"));
        assert!(!nmap.unmaintained);
        assert!(!nmap.pinned, "pinned defaults to false when absent");

        let openssl = packages.iter().find(|p| p.name == "openssl@3").unwrap();
        assert!(openssl.pinned, "pinned is read from the brew JSON");
        let wireshark = packages.iter().find(|p| p.name == "wireshark").unwrap();
        assert!(!wireshark.pinned, "casks are never pinned");
    }

    #[test]
    fn unmaintained_status_prefers_disable_reason_over_deprecation_reason() {
        let (unmaintained, reason) = unmaintained_status(
            true,
            Some("unmaintained".to_string()),
            true,
            Some("repo_archived".to_string()),
        );
        assert!(unmaintained);
        assert_eq!(reason.as_deref(), Some("repo_archived"));
    }

    #[test]
    fn unmaintained_status_falls_back_to_generic_label_without_a_reason() {
        let (unmaintained, reason) = unmaintained_status(true, None, false, None);
        assert!(unmaintained);
        assert_eq!(reason.as_deref(), Some("deprecated"));
    }

    #[test]
    fn unmaintained_status_is_false_for_a_healthy_package() {
        let (unmaintained, reason) = unmaintained_status(false, None, false, None);
        assert!(!unmaintained);
        assert_eq!(reason, None);
    }

    #[test]
    fn parses_deprecated_and_disabled_flags() {
        let json = br#"{
            "formulae": [
                {"name": "old-tool", "tap": "homebrew/core", "desc": "An old tool", "homepage": "https://example.com", "installed": [{"version": "1.0", "installed_on_request": true}], "deprecated": true, "deprecation_reason": "unmaintained"},
                {"name": "gone-tool", "tap": "homebrew/core", "desc": "A removed tool", "homepage": "https://example.com", "installed": [{"version": "1.0", "installed_on_request": true}], "deprecated": true, "deprecation_reason": "unmaintained", "disabled": true, "disable_reason": "repo_archived"}
            ],
            "casks": []
        }"#;
        let packages = parse_brew_json(json).expect("fixture should parse");

        let old_tool = packages.iter().find(|p| p.name == "old-tool").unwrap();
        assert!(old_tool.unmaintained);
        assert_eq!(
            old_tool.unmaintained_reason.as_deref(),
            Some("unmaintained")
        );

        let gone_tool = packages.iter().find(|p| p.name == "gone-tool").unwrap();
        assert!(gone_tool.unmaintained);
        assert_eq!(
            gone_tool.unmaintained_reason.as_deref(),
            Some("repo_archived")
        );
    }

    #[test]
    fn parses_outdated_json_and_strips_tap_prefixes() {
        let json = br#"{
            "formulae": [
                {"name": "tesseract", "installed_versions": ["5.5.2"], "current_version": "5.5.3", "pinned": false, "pinned_version": null},
                {"name": "anomalyco/tap/opencode", "installed_versions": ["1.18.4"], "current_version": "1.18.5", "pinned": false, "pinned_version": null}
            ],
            "casks": [
                {"name": "obs", "installed_versions": ["32.1.2"], "current_version": "32.2.1", "pinned": false, "pinned_version": null}
            ]
        }"#;

        let outdated = parse_outdated_json(json).expect("valid outdated JSON should parse");
        assert_eq!(outdated.get("tesseract"), Some(&"5.5.3".to_string()));
        assert_eq!(outdated.get("opencode"), Some(&"1.18.5".to_string()));
        assert_eq!(outdated.get("obs"), Some(&"32.2.1".to_string()));
        assert_eq!(outdated.len(), 3);
    }

    #[test]
    fn apply_outdated_only_touches_matching_packages() {
        let mut packages = vec![
            Package {
                name: "nmap".into(),
                kind: PackageKind::Formula,
                desc: String::new(),
                homepage: String::new(),
                tap: "homebrew/core".into(),
                version: "7.94".into(),
                installed_on_request: true,
                installed_at: None,
                dependencies: Vec::new(),
                indirect_dependencies: Vec::new(),
                outdated: None,
                unmaintained: false,
                unmaintained_reason: None,
                pinned: false,
            },
            Package {
                name: "wireshark".into(),
                kind: PackageKind::Cask,
                desc: String::new(),
                homepage: String::new(),
                tap: "homebrew/cask".into(),
                version: "4.4.0".into(),
                installed_on_request: true,
                installed_at: None,
                dependencies: Vec::new(),
                indirect_dependencies: Vec::new(),
                outdated: None,
                unmaintained: false,
                unmaintained_reason: None,
                pinned: false,
            },
        ];
        let mut outdated = BTreeMap::new();
        outdated.insert("nmap".to_string(), "7.99".to_string());

        apply_outdated(&mut packages, &outdated);

        assert_eq!(packages[0].outdated.as_deref(), Some("7.99"));
        assert_eq!(packages[1].outdated, None);
    }
}
