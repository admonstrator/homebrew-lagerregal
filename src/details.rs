use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::homebrew::{Package, PackageKind};

/// Runs `brew --cellar` / `brew --caskroom` once per process and caches the
/// result, since it never changes for the lifetime of a run and shelling out
/// per package would be wasteful.
fn brew_path(flag: &str) -> Option<PathBuf> {
    let output = Command::new("brew").arg(flag).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

pub fn cellar_root() -> Option<&'static PathBuf> {
    static CELLAR: OnceLock<Option<PathBuf>> = OnceLock::new();
    CELLAR.get_or_init(|| brew_path("--cellar")).as_ref()
}

pub fn caskroom_root() -> Option<&'static PathBuf> {
    static CASKROOM: OnceLock<Option<PathBuf>> = OnceLock::new();
    CASKROOM.get_or_init(|| brew_path("--caskroom")).as_ref()
}

/// Homebrew's download/metadata cache (`brew --cache`). Its contents change
/// when `brew update` refreshes the catalog, which is what makes it a useful
/// staleness signal for our own cache.
pub fn cache_root() -> Option<&'static PathBuf> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(|| brew_path("--cache")).as_ref()
}

/// Sums the size of every regular file under `path`, following symlinks -
/// Homebrew casks typically install their actual `.app` bundle under
/// `/Applications` and leave only a symlink to it in the Caskroom, so
/// without following links every cask would report a near-zero size.
/// Guards against symlink cycles by tracking which physical directories
/// (device + inode) have already been visited.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut visited_dirs: HashSet<(u64, u64)> = HashSet::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(meta) = std::fs::metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() {
            total += meta.len();
            continue;
        }
        if !visited_dirs.insert((meta.dev(), meta.ino())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// Computes the on-disk size of an installed package by summing its install
/// directory (`<Cellar>/<name>/<version>` or `<Caskroom>/<token>/<version>`).
/// Returns `None` if the install location can't be determined (e.g. `brew`
/// isn't on PATH) or doesn't exist on disk.
pub fn package_size(kind: PackageKind, name: &str, version: &str) -> Option<u64> {
    let root = match kind {
        PackageKind::Formula => cellar_root(),
        PackageKind::Cask => caskroom_root(),
    }?;
    let dir = root.join(name).join(version);
    if !dir.is_dir() {
        return None;
    }
    Some(dir_size(&dir))
}

/// Formats a byte count as a human-readable size (e.g. "34.2 MB").
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Converts a Unix timestamp (seconds, UTC) to a proleptic-Gregorian civil
/// date. Pure integer math (Howard Hinnant's `civil_from_days` algorithm),
/// used instead of pulling in a date/time crate for a single "installed on"
/// display.
fn unix_to_ymd(secs: i64) -> (i64, u32, u32) {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

/// Formats an install timestamp as an absolute date plus a relative age,
/// e.g. "2026-03-29 (4 months ago)".
pub fn format_age(installed_at: i64) -> String {
    let (y, m, d) = unix_to_ymd(installed_at);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(installed_at);
    let days = (now - installed_at) / 86_400;

    let relative = if days < 1 {
        "today".to_string()
    } else if days == 1 {
        "1 day ago".to_string()
    } else if days < 60 {
        format!("{days} days ago")
    } else if days < 730 {
        format!("{} months ago", days / 30)
    } else {
        format!("{} years ago", days / 365)
    };

    format!("{y:04}-{m:02}-{d:02} ({relative})")
}

/// One entry in a flattened, pre-order dependency tree.
pub struct DepNode {
    pub depth: usize,
    pub name: String,
    pub version: Option<String>,
}

/// Installed packages that list `name` among their direct (declared)
/// dependencies - the reverse edge of `dependency_tree`, answering "why is
/// this here?" for anything that arrived only as a dependency. Casks count
/// as dependents too, since they can declare formula dependencies.
pub fn reverse_dependencies<'a>(
    packages: impl IntoIterator<Item = &'a Package>,
    name: &str,
) -> Vec<String> {
    let mut out: Vec<String> = packages
        .into_iter()
        .filter(|p| p.dependencies.iter().any(|d| d == name))
        .map(|p| p.name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Names of installed packages that `brew autoremove` would consider
/// removable: installed only as a dependency, and needed by no package that
/// remains. Runs to a fixpoint because removals cascade - dropping the last
/// dependent of `a` can orphan `a`'s own dependency `b` in the next round.
///
/// Uses the *full* runtime-dependency edge set (direct + indirect), unlike
/// the dependency tree, which walks direct declarations only. The indirect
/// edges matter here: after the formula that directly declared a library is
/// uninstalled, the remaining dependents often reference it only through
/// their receipts' transitive entries - counting just direct edges would
/// flag such a library as an orphan while `brew autoremove` keeps it.
pub fn autoremove_candidates<'a>(
    packages: impl IntoIterator<Item = &'a Package>,
) -> HashSet<String> {
    let packages: Vec<&Package> = packages.into_iter().collect();
    let mut removed: HashSet<String> = HashSet::new();
    loop {
        let mut changed = false;
        for p in &packages {
            if p.installed_on_request || removed.contains(&p.name) {
                continue;
            }
            let still_needed = packages.iter().any(|q| {
                q.name != p.name
                    && !removed.contains(&q.name)
                    && (q.dependencies.iter().any(|d| d == &p.name)
                        || q.indirect_dependencies.iter().any(|d| d == &p.name))
            });
            if !still_needed {
                removed.insert(p.name.clone());
                changed = true;
            }
        }
        if !changed {
            return removed;
        }
    }
}

/// Builds a flattened dependency tree for `root`, walking each package's
/// direct dependencies recursively. Diamond dependencies (a lib used by
/// multiple branches) intentionally appear more than once, since that's how
/// a dependency *tree* (as opposed to a graph) reads; only an actual cycle
/// within a single branch is guarded against. `max_depth` and `max_nodes`
/// cap the walk so a package with a very large dependency graph can't
/// produce an unbounded result; the returned `bool` is `true` if the walk
/// was cut short by either limit.
pub fn dependency_tree(
    index: &BTreeMap<String, &Package>,
    root: &str,
    max_depth: usize,
    max_nodes: usize,
) -> (Vec<DepNode>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    if let Some(pkg) = index.get(root) {
        let mut visiting = vec![root.to_string()];
        for dep in &pkg.dependencies {
            walk(
                index,
                dep,
                1,
                max_depth,
                max_nodes,
                &mut visiting,
                &mut out,
                &mut truncated,
            );
        }
    }
    (out, truncated)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    index: &BTreeMap<String, &Package>,
    name: &str,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    visiting: &mut Vec<String>,
    out: &mut Vec<DepNode>,
    truncated: &mut bool,
) {
    if out.len() >= max_nodes {
        *truncated = true;
        return;
    }
    if visiting.contains(&name.to_string()) {
        return;
    }

    let pkg = index.get(name);
    out.push(DepNode {
        depth,
        name: name.to_string(),
        version: pkg.map(|p| p.version.clone()),
    });

    if depth >= max_depth {
        if pkg.is_some_and(|p| !p.dependencies.is_empty()) {
            *truncated = true;
        }
        return;
    }

    if let Some(pkg) = pkg {
        visiting.push(name.to_string());
        for dep in &pkg.dependencies {
            walk(
                index,
                dep,
                depth + 1,
                max_depth,
                max_nodes,
                visiting,
                out,
                truncated,
            );
        }
        visiting.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_to_ymd_matches_known_dates() {
        assert_eq!(unix_to_ymd(0), (1970, 1, 1));
        assert_eq!(unix_to_ymd(1_774_780_748), (2026, 3, 29));
        assert_eq!(unix_to_ymd(1_781_056_151), (2026, 6, 10));
    }

    #[test]
    fn format_size_scales_units() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(32 * 1024 * 1024), "32.0 MB");
    }

    #[test]
    fn dir_size_follows_symlinks_like_a_cask_install() {
        // Mirrors a real Homebrew cask layout: the Caskroom entry is just a
        // symlink to the actual `.app` bundle elsewhere (typically
        // /Applications), so the size has to be read through the link.
        let base = std::env::temp_dir().join(format!("lagerregal-dirsize-{}", std::process::id()));
        let real_app = base.join("Applications").join("Thing.app");
        let caskroom_entry = base.join("Caskroom").join("thing").join("1.0");
        std::fs::create_dir_all(&real_app).unwrap();
        std::fs::write(real_app.join("payload.bin"), vec![0u8; 5000]).unwrap();
        std::fs::create_dir_all(&caskroom_entry).unwrap();
        std::os::unix::fs::symlink(&real_app, caskroom_entry.join("Thing.app")).unwrap();

        assert_eq!(dir_size(&caskroom_entry), 5000);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dir_size_does_not_hang_on_a_symlink_cycle() {
        let base = std::env::temp_dir().join(format!("lagerregal-dircycle-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        // A symlink inside `base` pointing back at `base` itself.
        std::os::unix::fs::symlink(&base, base.join("loop")).unwrap();

        // Must terminate (the (dev, ino) guard stops re-entering `base`).
        let _ = dir_size(&base);

        let _ = std::fs::remove_dir_all(&base);
    }

    fn pkg(name: &str, version: &str, deps: &[&str]) -> Package {
        Package {
            name: name.to_string(),
            kind: PackageKind::Formula,
            desc: String::new(),
            homepage: String::new(),
            tap: "homebrew/core".to_string(),
            version: version.to_string(),
            installed_on_request: true,
            installed_at: None,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            indirect_dependencies: Vec::new(),
            outdated: None,
            unmaintained: false,
            unmaintained_reason: None,
            pinned: false,
        }
    }

    #[test]
    fn autoremove_candidates_cascades_through_orphan_chains() {
        let dep = |name: &str, deps: &[&str]| {
            let mut p = pkg(name, "1.0", deps);
            p.installed_on_request = false;
            p
        };
        // x (on request) -> a -> b : the whole chain is anchored by x.
        // c -> d, with no dependents: c falls, which then cascades to d.
        let packages = vec![
            pkg("x", "1.0", &["a"]),
            dep("a", &["b"]),
            dep("b", &[]),
            dep("c", &["d"]),
            dep("d", &[]),
        ];
        let orphans = autoremove_candidates(&packages);
        assert!(
            !orphans.contains("a") && !orphans.contains("b"),
            "anchored chain survives"
        );
        assert!(
            orphans.contains("c") && orphans.contains("d"),
            "unanchored chain cascades"
        );
        assert!(!orphans.contains("x"), "on-request packages never qualify");
    }

    #[test]
    fn autoremove_candidates_honours_indirect_dependency_edges() {
        // Regression for a real-world case: `gpgmepp` referenced `unbound`
        // only via its receipt's transitive runtime deps (the formula that
        // declared it directly was gone), and an analysis over direct edges
        // alone called `unbound` an orphan while `brew autoremove` did not.
        let mut root = pkg("root", "1.0", &[]);
        root.indirect_dependencies = vec!["lib".to_string()];
        let mut lib = pkg("lib", "1.0", &[]);
        lib.installed_on_request = false;

        let orphans = autoremove_candidates([&root, &lib]);
        assert!(
            !orphans.contains("lib"),
            "an indirect edge anchors the package"
        );
    }

    #[test]
    fn reverse_dependencies_lists_direct_dependents_sorted() {
        let a = pkg("a", "1.0", &["shared", "b"]);
        let b = pkg("b", "2.0", &["shared"]);
        let c = pkg("c", "3.0", &[]);
        let shared = pkg("shared", "0.1", &[]);

        let all = [&a, &b, &c, &shared];
        assert_eq!(
            reverse_dependencies(all.into_iter(), "shared"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(reverse_dependencies(all.into_iter(), "c").is_empty());
        // Only *direct* dependents: `c` doesn't appear just because some
        // chain could reach it.
        assert_eq!(
            reverse_dependencies(all.into_iter(), "b"),
            vec!["a".to_string()]
        );
    }

    #[test]
    fn dependency_tree_walks_transitively_and_flags_missing() {
        let a = pkg("a", "1.0", &["b", "missing"]);
        let b = pkg("b", "2.0", &["c"]);
        let c = pkg("c", "3.0", &[]);
        let index: BTreeMap<String, &Package> = [("a", &a), ("b", &b), ("c", &c)]
            .into_iter()
            .map(|(n, p)| (n.to_string(), p))
            .collect();

        let (nodes, truncated) = dependency_tree(&index, "a", 10, 100);
        assert!(!truncated);
        let names: Vec<_> = nodes.iter().map(|n| (n.depth, n.name.as_str())).collect();
        assert_eq!(names, vec![(1, "b"), (2, "c"), (1, "missing")]);
        assert!(nodes
            .iter()
            .find(|n| n.name == "missing")
            .unwrap()
            .version
            .is_none());
        assert_eq!(
            nodes
                .iter()
                .find(|n| n.name == "c")
                .unwrap()
                .version
                .as_deref(),
            Some("3.0")
        );
    }

    #[test]
    fn dependency_tree_guards_against_cycles() {
        let a = pkg("a", "1.0", &["b"]);
        let b = pkg("b", "2.0", &["a"]);
        let index: BTreeMap<String, &Package> = [("a", &a), ("b", &b)]
            .into_iter()
            .map(|(n, p)| (n.to_string(), p))
            .collect();

        let (nodes, _) = dependency_tree(&index, "a", 10, 100);
        // b is walked once; its dependency back on a is a cycle to a node
        // already on the current branch, so it's dropped rather than
        // recursing forever.
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "b");
    }

    #[test]
    fn dependency_tree_respects_max_nodes() {
        let a = pkg("a", "1.0", &["b", "c", "d"]);
        let b = pkg("b", "1.0", &[]);
        let c = pkg("c", "1.0", &[]);
        let d = pkg("d", "1.0", &[]);
        let index: BTreeMap<String, &Package> = [("a", &a), ("b", &b), ("c", &c), ("d", &d)]
            .into_iter()
            .map(|(n, p)| (n.to_string(), p))
            .collect();

        let (nodes, truncated) = dependency_tree(&index, "a", 10, 2);
        assert_eq!(nodes.len(), 2);
        assert!(truncated);
    }
}
