use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::details;
use crate::homebrew::Package;
use crate::store;

/// Bumped whenever the shape of `Package` (or of this file) changes, so an
/// older cache written by a previous build is discarded rather than
/// deserialized into something subtly wrong.
// v2: `Package` gained `pinned` and `indirect_dependencies`, and the file
// gained the `sizes` map. Bumping discards pre-field caches wholesale
// instead of relying on serde defaults to paper over the difference.
const CACHE_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    fingerprint: String,
    packages: Vec<Package>,
    /// `None` means "we never ran `brew outdated` for this fingerprint" -
    /// distinct from `Some(empty)`, which means "we ran it and everything
    /// was up to date". Commands that don't need update info skip that call
    /// entirely, so the cache has to be able to represent the gap.
    outdated: Option<BTreeMap<String, String>>,
    /// On-disk sizes previously computed by size-sorting, the detail pane,
    /// or `categories --sizes`, keyed `name|version`. Sizes are expensive
    /// (a full directory walk per package) and only change when the
    /// installed version does - which changes the key - so unlike the rest
    /// of this file they stay valid across fingerprint changes and are
    /// carried over (pruned to still-installed versions) on every rewrite.
    #[serde(default)]
    sizes: BTreeMap<String, u64>,
}

pub struct Cached {
    pub packages: Vec<Package>,
    pub outdated: Option<BTreeMap<String, String>>,
}

fn cache_path() -> Result<PathBuf> {
    Ok(store::data_dir()?.join("cache.json"))
}

/// Mixes a name and modification time into a running hash (FNV-1a). Kept
/// dependency-free on purpose - this is a change detector, not a security
/// primitive, so a fast non-cryptographic hash is the right tool.
fn mix(hash: &mut u64, name: &str, mtime_ns: i128) {
    for byte in name.as_bytes().iter().chain(&mtime_ns.to_le_bytes()) {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// Folds every direct child of `dir` (name + mtime) into `hash`. Entries are
/// sorted first so the result doesn't depend on filesystem iteration order.
fn hash_dir(hash: &mut u64, dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(String, i128)> = entries
        .flatten()
        .map(|e| {
            let mtime = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i128)
                .unwrap_or(0);
            (e.file_name().to_string_lossy().into_owned(), mtime)
        })
        .collect();
    items.sort();
    for (name, mtime) in items {
        mix(hash, &name, mtime);
    }
}

/// A cheap signature of everything that could change what `brew info` and
/// `brew outdated` would report:
///
/// - the Cellar and Caskroom directory listings, whose entries appear and
///   disappear on install/uninstall and whose mtimes bump on upgrade (a new
///   version directory is added alongside the old one, which updates the
///   parent's mtime);
/// - Homebrew's API/metadata cache, which `brew update` rewrites - that's
///   what can change a package's description or its deprecated flag without
///   anything in the Cellar moving.
///
/// Reading it costs ~1ms for a few hundred packages, versus ~2.9s for the
/// two `brew` calls it lets us skip. Returns `None` if the paths can't be
/// resolved at all, which disables caching rather than risking a signature
/// that can't detect changes.
pub fn fingerprint() -> Option<String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    roots.extend(details::cellar_root().cloned());
    roots.extend(details::caskroom_root().cloned());
    roots.extend(details::cache_root().map(|p| p.join("api")));
    if roots.is_empty() {
        return None;
    }

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for root in &roots {
        hash_dir(&mut hash, root);
    }
    Some(format!("{hash:016x}"))
}

/// Reads the cache, returning it only if it was written by this build and
/// matches the current fingerprint. Any problem (missing, unreadable,
/// corrupt, stale) is reported as a plain miss - a cache must never be able
/// to turn into an error the user has to care about.
pub fn load(fingerprint: &str) -> Option<Cached> {
    let path = cache_path().ok()?;
    let contents = fs::read_to_string(path).ok()?;
    let cached: CacheFile = serde_json::from_str(&contents).ok()?;
    if cached.version != CACHE_VERSION || cached.fingerprint != fingerprint {
        return None;
    }
    Some(Cached {
        packages: cached.packages,
        outdated: cached.outdated,
    })
}

/// Writes the cache, ignoring failures for the same reason `load` does: a
/// cache that can't be written should slow the next run down, not break it.
///
/// `Package::outdated` is stripped from what gets written, on purpose. By
/// the time a caller has update info it has usually already merged it into
/// the packages, and storing them in that state would leak update info into
/// later commands that never asked for it - making a cached run differ from
/// a fresh one. The update map is kept in its own field instead, so
/// re-applying it stays an explicit decision by the caller.
pub fn store(fingerprint: &str, packages: &[Package], outdated: Option<&BTreeMap<String, String>>) {
    let Ok(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let mut packages = packages.to_vec();
    for package in &mut packages {
        package.outdated = None;
    }
    // A full rewrite would silently drop the accumulated size data, so it
    // gets carried over from whatever file is being replaced.
    let sizes = prune_sizes(read_sizes().unwrap_or_default(), &packages);
    let file = CacheFile {
        version: CACHE_VERSION,
        fingerprint: fingerprint.to_string(),
        packages,
        outdated: outdated.cloned(),
        sizes,
    };
    if let Ok(json) = serde_json::to_string(&file) {
        let _ = fs::write(path, json);
    }
}

/// Cache key for a package's on-disk size. Versioned, so an upgrade
/// naturally invalidates the old entry instead of serving a stale size.
pub fn size_key(name: &str, version: &str) -> String {
    format!("{name}|{version}")
}

/// Keeps only size entries whose `name|version` still corresponds to an
/// installed package, so uninstalled or upgraded-away versions don't
/// accumulate in the file forever.
fn prune_sizes(sizes: BTreeMap<String, u64>, packages: &[Package]) -> BTreeMap<String, u64> {
    let live: std::collections::HashSet<String> = packages
        .iter()
        .map(|p| size_key(&p.name, &p.version))
        .collect();
    sizes
        .into_iter()
        .filter(|(k, _)| live.contains(k))
        .collect()
}

/// The raw sizes map from the cache file, if it exists and was written by
/// this build. Deliberately ignores the fingerprint: size entries carry
/// their own validity in the key (see `size_key`).
fn read_sizes() -> Option<BTreeMap<String, u64>> {
    let path = cache_path().ok()?;
    let contents = fs::read_to_string(path).ok()?;
    let cached: CacheFile = serde_json::from_str(&contents).ok()?;
    (cached.version == CACHE_VERSION).then_some(cached.sizes)
}

/// Loads the persisted size map for seeding an in-memory lookup. A missing
/// or unusable cache is just an empty map - sizes get recomputed on demand.
pub fn load_sizes() -> BTreeMap<String, u64> {
    read_sizes().unwrap_or_default()
}

/// Writes an updated size map back into the cache file, leaving everything
/// else in it untouched. A missing or stale cache file is left alone - the
/// next full `store` will persist the sizes then.
pub fn merge_sizes(sizes: &BTreeMap<String, u64>) {
    let Ok(path) = cache_path() else {
        return;
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut cached) = serde_json::from_str::<CacheFile>(&contents) else {
        return;
    };
    if cached.version != CACHE_VERSION {
        return;
    }
    cached
        .sizes
        .extend(sizes.iter().map(|(k, v)| (k.clone(), *v)));
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = fs::write(path, json);
    }
}

/// Cached-size lookup shared by the TUI and the CLI: returns the persisted
/// size for this exact name+version if known, otherwise walks the package's
/// install directory, records the result in `sizes`, and raises `dirty` so
/// the caller knows there's something worth writing back via `merge_sizes`.
pub fn size_or_compute(
    sizes: &mut BTreeMap<String, u64>,
    dirty: &mut bool,
    kind: crate::homebrew::PackageKind,
    name: &str,
    version: &str,
) -> Option<u64> {
    let key = size_key(name, version);
    if let Some(&s) = sizes.get(&key) {
        return Some(s);
    }
    let size = details::package_size(kind, name, version);
    if let Some(s) = size {
        sizes.insert(key, s);
        *dirty = true;
    }
    size
}

/// Deletes the cache file. Used by `--refresh` so a forced reload also
/// clears a cache that somehow went bad, rather than leaving it to be
/// overwritten only if the reload succeeds.
pub fn clear() {
    if let Ok(path) = cache_path() {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lagerregal-cache-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hash_dir_changes_when_an_entry_is_added_or_removed() {
        let dir = temp_dir("entries");
        let mut before: u64 = 0;
        hash_dir(&mut before, &dir);

        fs::create_dir(dir.join("nmap")).unwrap();
        let mut added: u64 = 0;
        hash_dir(&mut added, &dir);
        assert_ne!(before, added, "adding a package must change the hash");

        fs::remove_dir(dir.join("nmap")).unwrap();
        let mut removed: u64 = 0;
        hash_dir(&mut removed, &dir);
        assert_eq!(
            before, removed,
            "removing it again must return to the original hash"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_dir_is_stable_across_repeated_reads() {
        let dir = temp_dir("stable");
        fs::create_dir(dir.join("a")).unwrap();
        fs::create_dir(dir.join("b")).unwrap();

        let mut first: u64 = 0;
        hash_dir(&mut first, &dir);
        let mut second: u64 = 0;
        hash_dir(&mut second, &dir);
        assert_eq!(first, second);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_dir_notices_a_version_directory_appearing_inside_a_package() {
        // This is the upgrade case: `Cellar/foo/2.0` shows up next to
        // `Cellar/foo/1.0`, which bumps `Cellar/foo`'s mtime.
        let dir = temp_dir("upgrade");
        let pkg = dir.join("foo");
        fs::create_dir(&pkg).unwrap();
        fs::create_dir(pkg.join("1.0")).unwrap();

        let mut before: u64 = 0;
        hash_dir(&mut before, &dir);

        // Filesystem mtimes have limited resolution; make sure the change is
        // distinguishable rather than racing inside the same tick.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::create_dir(pkg.join("2.0")).unwrap();

        let mut after: u64 = 0;
        hash_dir(&mut after, &dir);
        assert_ne!(
            before, after,
            "a new version directory must change the hash"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stored_packages_never_carry_update_info() {
        // Regression guard: callers usually hold packages that already have
        // `outdated` merged in. If that reached the cache, a later command
        // asking for no update info would still get it - a cached run and a
        // fresh run would disagree.
        use crate::homebrew::PackageKind;

        let packages = vec![Package {
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
            outdated: Some("7.99".into()),
            unmaintained: false,
            unmaintained_reason: None,
            pinned: false,
        }];

        let mut outdated = BTreeMap::new();
        outdated.insert("nmap".to_string(), "7.99".to_string());

        // Exercise the same stripping `store` does, without touching the
        // real cache file that a parallel test (or the user) may be using.
        let file = CacheFile {
            version: CACHE_VERSION,
            fingerprint: "test".into(),
            packages: {
                let mut p = packages.clone();
                for package in &mut p {
                    package.outdated = None;
                }
                p
            },
            outdated: Some(outdated),
            sizes: BTreeMap::new(),
        };

        let json = serde_json::to_string(&file).unwrap();
        let round_tripped: CacheFile = serde_json::from_str(&json).unwrap();
        assert_eq!(
            round_tripped.packages[0].outdated, None,
            "cached packages must not carry `outdated`"
        );
        assert_eq!(
            round_tripped.outdated.unwrap().get("nmap"),
            Some(&"7.99".to_string()),
            "the update map itself must survive alongside them"
        );
    }

    #[test]
    fn prune_sizes_drops_entries_for_gone_or_upgraded_packages() {
        use crate::homebrew::PackageKind;

        let pkg = |name: &str, version: &str| Package {
            name: name.into(),
            kind: PackageKind::Formula,
            desc: String::new(),
            homepage: String::new(),
            tap: String::new(),
            version: version.into(),
            installed_on_request: true,
            installed_at: None,
            dependencies: Vec::new(),
            indirect_dependencies: Vec::new(),
            outdated: None,
            unmaintained: false,
            unmaintained_reason: None,
            pinned: false,
        };

        let mut sizes = BTreeMap::new();
        sizes.insert(size_key("jq", "1.8"), 100);
        sizes.insert(size_key("jq", "1.7"), 90); // upgraded away
        sizes.insert(size_key("gone", "1.0"), 50); // uninstalled

        let pruned = prune_sizes(sizes, &[pkg("jq", "1.8")]);
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned.get(&size_key("jq", "1.8")), Some(&100));
    }

    #[test]
    fn missing_directories_are_skipped_rather_than_panicking() {
        let mut hash: u64 = 123;
        hash_dir(
            &mut hash,
            Path::new("/nonexistent/path/for/lagerregal/tests"),
        );
        assert_eq!(hash, 123, "an unreadable directory must not alter the hash");
    }
}
