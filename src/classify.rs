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

    let desc_lower = desc.to_lowercase();
    for h in &data.heuristic {
        if h.keywords.iter().any(|kw| desc_lower.contains(kw.as_str())) {
            return (h.category.clone(), ClassificationSource::Heuristic);
        }
    }

    (
        UNCATEGORIZED.to_string(),
        ClassificationSource::Uncategorized,
    )
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
    fn classify_all_applies_overrides_and_notes() {
        let packages = vec![Package {
            name: "nmap".into(),
            kind: PackageKind::Formula,
            desc: "Port scanning utility".into(),
            homepage: "https://nmap.org".into(),
            tap: "homebrew/core".into(),
            version: "7.94".into(),
            installed_on_request: true,
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
