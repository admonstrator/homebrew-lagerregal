use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::homebrew::Package;

pub const UNCATEGORIZED: &str = "Uncategorized";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassificationSource {
    Manual,
    Curated,
    Heuristic,
    Uncategorized,
}

impl ClassificationSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClassificationSource::Manual => "manual",
            ClassificationSource::Curated => "curated",
            ClassificationSource::Heuristic => "heuristic",
            ClassificationSource::Uncategorized => "uncategorized",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClassifiedPackage {
    pub package: Package,
    pub category: String,
    pub source: ClassificationSource,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Heuristic {
    category: String,
    keywords: Vec<String>,
    /// Phrases that veto this category even when a keyword matched. Exists
    /// for words that mean something else in a specific context - "terminal
    /// emulator" contains "emulat" without being about games.
    #[serde(default)]
    exclude: Vec<String>,
}

/// Length at or below which a keyword must start at a word boundary.
///
/// Short keywords collide by accident: "ssl" inside "lossless", "ocr" inside
/// "gocryptfs", "dns" inside "cjdns". Long ones essentially don't, and
/// several rely on matching mid-word ("compiler" in "decompiler", "game" in
/// "videogame"), so the rule is deliberately limited to the short ones.
const BOUNDARY_REQUIRED_UP_TO: usize = 4;

/// Whether `keyword` occurs in `haystack`, honouring the word-boundary rule
/// for short keywords. Growth to the *right* is always allowed, so stems
/// like "emulat" still match "emulator" and "emulation".
fn keyword_matches(keyword: &str, haystack: &str) -> bool {
    let needs_boundary = keyword.trim().len() <= BOUNDARY_REQUIRED_UP_TO
        && keyword.starts_with(|c: char| c.is_ascii_alphanumeric());
    if !needs_boundary {
        return haystack.contains(keyword);
    }
    haystack.match_indices(keyword).any(|(at, _)| {
        at == 0
            || !haystack[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

#[derive(Debug, Deserialize)]
struct CategoryData {
    curated: BTreeMap<String, String>,
    heuristic: Vec<Heuristic>,
}

static CATEGORY_DATA: OnceLock<CategoryData> = OnceLock::new();

fn category_data() -> &'static CategoryData {
    CATEGORY_DATA.get_or_init(|| {
        toml::from_str(include_str!("data/categories.toml"))
            .expect("bundled data/categories.toml must be valid TOML")
    })
}

/// All known category names in a stable display order: heuristic categories
/// (highest priority first), then any curated-only categories, then
/// "Uncategorized" last.
pub fn known_categories() -> Vec<String> {
    let data = category_data();
    let mut cats: Vec<String> = data.heuristic.iter().map(|h| h.category.clone()).collect();
    for cat in data.curated.values() {
        if !cats.contains(cat) {
            cats.push(cat.clone());
        }
    }
    cats.push(UNCATEGORIZED.to_string());
    cats
}

/// Classifies a single package by name/description, given an optional manual
/// override from local state. Precedence: manual override > curated name
/// lookup > keyword heuristic on `desc` > Uncategorized.
pub fn classify(
    name: &str,
    desc: &str,
    manual_override: Option<&str>,
) -> (String, ClassificationSource) {
    if let Some(cat) = manual_override {
        return (cat.to_string(), ClassificationSource::Manual);
    }

    let data = category_data();

    if let Some(cat) = data.curated.get(name) {
        return (cat.clone(), ClassificationSource::Curated);
    }

    // Matched against name + desc together (not desc alone) so name-pattern
    // packages with no useful description - e.g. the thousands of `font-*`
    // casks, which carry no `desc` at all - can still be caught by keywords
    // like "font-" without needing one curated entry per package.
    let haystack = format!("{name} {desc}").to_lowercase();
    for h in &data.heuristic {
        if h.exclude.iter().any(|ex| haystack.contains(ex.as_str())) {
            continue;
        }
        if h.keywords.iter().any(|kw| keyword_matches(kw, &haystack)) {
            return (h.category.clone(), ClassificationSource::Heuristic);
        }
    }

    (
        UNCATEGORIZED.to_string(),
        ClassificationSource::Uncategorized,
    )
}

/// Filters out packages that were only pulled in as a dependency of
/// something else (unless `include_deps` is set), so the default view
/// reflects what the user actually chose to install rather than every
/// transitive C library brew happened to build along the way.
pub fn filter_on_request(
    packages: Vec<ClassifiedPackage>,
    include_deps: bool,
) -> Vec<ClassifiedPackage> {
    if include_deps {
        packages
    } else {
        packages
            .into_iter()
            .filter(|p| p.package.installed_on_request)
            .collect()
    }
}

/// Classifies a full list of installed packages, applying manual overrides
/// and notes from the local state store where present.
pub fn classify_all(
    packages: Vec<Package>,
    overrides: &BTreeMap<String, String>,
    notes: &BTreeMap<String, String>,
) -> Vec<ClassifiedPackage> {
    packages
        .into_iter()
        .map(|package| {
            let (category, source) = classify(
                &package.name,
                &package.desc,
                overrides.get(&package.name).map(|s| s.as_str()),
            );
            let note = notes.get(&package.name).cloned();
            ClassifiedPackage {
                package,
                category,
                source,
                note,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::homebrew::PackageKind;

    #[test]
    fn curated_lookup_wins_over_heuristic() {
        let (cat, source) = classify("nmap", "some unrelated description", None);
        assert_eq!(cat, "Networking");
        assert_eq!(source, ClassificationSource::Curated);
    }

    #[test]
    fn heuristic_matches_on_name_when_description_is_empty() {
        // Homebrew's `font-*` casks (there are thousands) ship with no
        // `desc` at all - only a name - so the heuristic must be able to
        // match against the package name too.
        let (cat, source) = classify("font-fira-code-nerd-font", "", None);
        assert_eq!(cat, "Fonts");
        assert_eq!(source, ClassificationSource::Heuristic);
    }

    #[test]
    fn heuristic_matches_on_description() {
        let (cat, source) = classify(
            "totally-unknown-tool",
            "A blazing fast DNS resolver and cache",
            None,
        );
        assert_eq!(cat, "DNS");
        assert_eq!(source, ClassificationSource::Heuristic);
    }

    #[test]
    fn terminal_emulators_are_not_games() {
        // The bug this guards: "emulat" is the right stem for game
        // emulators, and a terminal emulator is not one.
        let (cat, _) = classify(
            "ghostty",
            "Terminal emulator that uses platform-native UI and GPU acceleration",
            None,
        );
        assert_eq!(cat, "Productivity");

        let (cat, _) = classify("foot", "Fast, lightweight Wayland terminal emulator", None);
        assert_eq!(cat, "Productivity");
    }

    #[test]
    fn actual_game_emulators_still_classify_as_games() {
        // The other half of the same rule: don't fix terminals by breaking
        // the case the keyword exists for.
        for (name, desc) in [
            ("atari800", "Atari 8-bit machine emulator"),
            ("fceux", "All-in-one NES/Famicom Emulator"),
            (
                "dosbox-x",
                "DOSBox with accurate emulation and wide testing",
            ),
        ] {
            let (cat, _) = classify(name, desc, None);
            assert_eq!(
                cat, "Games & Emulation",
                "{name} should stay a game emulator"
            );
        }
    }

    #[test]
    fn short_keywords_do_not_match_inside_other_words() {
        // "ssl" inside "lossless" used to file compression tools under
        // Cryptography; "dns" inside "cjdns" filed a mesh router under DNS.
        let (cat, _) = classify(
            "brotli",
            "Generic-purpose lossless compression algorithm",
            None,
        );
        assert_eq!(cat, "Archives & Compression");

        let (cat, _) = classify("flac", "Free lossless audio codec", None);
        assert_eq!(cat, "Media & Graphics");
    }

    #[test]
    fn short_keywords_still_match_as_whole_words_and_in_known_compounds() {
        // The boundary rule must not cost us the legitimate hits: a bare
        // occurrence, and the compounds spelled out in categories.toml.
        let (cat, _) = classify("some-tool", "Talks TLS over SSL sockets", None);
        assert_eq!(cat, "Cryptography");

        let (cat, _) = classify("hopenpgp-tools", "Command-line tools for OpenPGP", None);
        assert_eq!(cat, "Cryptography");

        let (cat, _) = classify("ddns-go", "Simple and easy-to-use DDNS", None);
        assert_eq!(cat, "DNS");
    }

    #[test]
    fn long_keywords_may_still_match_mid_word() {
        // Several stems depend on this: "compiler" inside "decompiler",
        // "game" inside "videogame".
        let (cat, _) = classify("jadx", "Dex to Java decompiler", None);
        assert_eq!(cat, "Dev Tools & Languages");

        let (cat, _) = classify("myman", "Text-mode videogame inspired by Pac-Man", None);
        assert_eq!(cat, "Games & Emulation");
    }

    #[test]
    fn manual_override_wins_over_curated() {
        let (cat, source) = classify("nmap", "Port scanning utility", Some("Security"));
        assert_eq!(cat, "Security");
        assert_eq!(source, ClassificationSource::Manual);
    }

    #[test]
    fn unknown_package_falls_back_to_uncategorized() {
        let (cat, source) = classify("mystery-tool", "Does something mysterious", None);
        assert_eq!(cat, UNCATEGORIZED);
        assert_eq!(source, ClassificationSource::Uncategorized);
    }

    #[test]
    fn filter_on_request_hides_dependency_only_packages_by_default() {
        let on_request = Package {
            name: "nmap".into(),
            kind: PackageKind::Formula,
            desc: "Port scanning utility".into(),
            homepage: "https://nmap.org".into(),
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
        };
        let dependency = Package {
            name: "libpng".into(),
            kind: PackageKind::Formula,
            desc: "Library for manipulating PNG images".into(),
            homepage: "http://www.libpng.org/pub/png/libpng.html".into(),
            tap: "homebrew/core".into(),
            version: "1.6.43".into(),
            installed_on_request: false,
            installed_at: None,
            dependencies: Vec::new(),
            indirect_dependencies: Vec::new(),
            outdated: None,
            unmaintained: false,
            unmaintained_reason: None,
            pinned: false,
        };
        let classified = classify_all(
            vec![on_request, dependency],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );

        let default_view = filter_on_request(classified.clone(), false);
        assert_eq!(default_view.len(), 1);
        assert_eq!(default_view[0].package.name, "nmap");

        let full_view = filter_on_request(classified, true);
        assert_eq!(full_view.len(), 2);
    }

    #[test]
    fn classify_all_applies_overrides_and_notes() {
        let packages = vec![Package {
            name: "nmap".into(),
            kind: PackageKind::Formula,
            desc: "Port scanning utility".into(),
            homepage: "https://nmap.org".into(),
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
        }];
        let mut overrides = BTreeMap::new();
        overrides.insert("nmap".to_string(), "Security".to_string());
        let mut notes = BTreeMap::new();
        notes.insert("nmap".to_string(), "for CTF recon".to_string());

        let classified = classify_all(packages, &overrides, &notes);
        assert_eq!(classified[0].category, "Security");
        assert_eq!(classified[0].source, ClassificationSource::Manual);
        assert_eq!(classified[0].note.as_deref(), Some("for CTF recon"));
    }
}
