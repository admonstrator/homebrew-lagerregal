mod cache;
mod classify;
mod cli;
mod details;
mod homebrew;
mod snapshot;
mod store;
mod theme;
mod tui;

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::Parser;
use comfy_table::{Cell, Table, presets::UTF8_FULL};
use serde::Serialize;

use classify::ClassifiedPackage;
use cli::{Cli, Command, SnapshotCommand};
use homebrew::Package;
use store::State;

const DEFAULT_SNAPSHOT: &str = "default";

/// Set once from `--refresh` at startup. A global (like `theme::init_icons`)
/// rather than a parameter threaded through every `cmd_*` function, since
/// it's a process-wide switch that none of them decide for themselves.
static REFRESH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() -> Result<()> {
    let cli = Cli::parse();
    theme::init_icons(!cli.no_icons);
    REFRESH.store(cli.refresh, std::sync::atomic::Ordering::Relaxed);

    match cli.command {
        Some(Command::Scan { all }) => cmd_scan(all),
        Some(Command::List {
            category,
            json,
            all,
        }) => cmd_list(category, json, all),
        Some(Command::Show { name }) => cmd_show(&name),
        Some(Command::Note { name, text }) => cmd_note(&name, &text),
        Some(Command::Category {
            name,
            category,
            reset,
        }) => cmd_category(&name, category.as_deref(), reset),
        Some(Command::Categories { all, sizes }) => cmd_categories(all, sizes),
        Some(Command::Outdated { all, json }) => cmd_outdated(all, json),
        Some(Command::Unmaintained { all, json }) => cmd_unmaintained(all, json),
        Some(Command::Orphans { json }) => cmd_orphans(json),
        Some(Command::Update { name }) => cmd_update(&name),
        Some(Command::Snapshot { action }) => match action {
            SnapshotCommand::Save { name, all } => {
                cmd_snapshot_save(name.as_deref().unwrap_or(DEFAULT_SNAPSHOT), all)
            }
            SnapshotCommand::Diff { name, all } => {
                cmd_snapshot_diff(name.as_deref().unwrap_or(DEFAULT_SNAPSHOT), all)
            }
        },
        Some(Command::Tui) | None => tui::run(),
    }
}

/// Reads the current Homebrew install state, preferring a local cache when
/// nothing relevant has changed since it was written.
///
/// The two `brew` calls this replaces cost ~2.9s combined (mostly Homebrew
/// loading every installed formula/cask definition), which is the entire
/// startup delay. Validating the cache instead costs ~1ms - see
/// [`cache::fingerprint`] for what "nothing relevant has changed" means.
///
/// On a miss the two calls run concurrently rather than back to back, since
/// they're independent and each is dominated by Homebrew's own start-up.
///
/// `with_outdated` controls whether update information is needed at all;
/// commands that don't use it skip `brew outdated` entirely, and the cache
/// records that gap so a later command that *does* need it can fill in just
/// that half.
pub fn load_packages(with_outdated: bool, refresh: bool) -> Result<Vec<homebrew::Package>> {
    let fingerprint = cache::fingerprint();

    if refresh {
        cache::clear();
    } else if let Some(fp) = &fingerprint
        && let Some(cached) = cache::load(fp)
    {
        match cached.outdated {
            Some(outdated) => {
                let mut packages = cached.packages;
                if with_outdated {
                    homebrew::apply_outdated(&mut packages, &outdated);
                }
                return Ok(packages);
            }
            // Cached without update info. Still a win: the expensive
            // half is already in hand, so only `brew outdated` runs.
            None if with_outdated => {
                let mut packages = cached.packages;
                let outdated = homebrew::outdated_versions()
                    .context("failed to check for updates via `brew outdated`")?;
                homebrew::apply_outdated(&mut packages, &outdated);
                cache::store(fp, &packages, Some(&outdated));
                return Ok(packages);
            }
            None => return Ok(cached.packages),
        }
    }

    // Cache miss: fetch both halves at once. `brew outdated` is spawned on a
    // scoped thread so its ~1s overlaps the ~1.9s of `brew info` instead of
    // following it.
    let (packages, outdated) = std::thread::scope(|scope| {
        let outdated_handle = with_outdated.then(|| scope.spawn(homebrew::outdated_versions));
        let packages = homebrew::installed_packages();
        let outdated = outdated_handle.map(|h| h.join().expect("outdated thread panicked"));
        (packages, outdated)
    });

    let mut packages = packages.context(
        "Could not read installed Homebrew packages. Is Homebrew installed and on your PATH?",
    )?;
    let outdated = outdated
        .transpose()
        .context("failed to check for updates via `brew outdated`")?;
    if let Some(outdated) = &outdated {
        homebrew::apply_outdated(&mut packages, outdated);
    }

    if let Some(fp) = &fingerprint {
        cache::store(fp, &packages, outdated.as_ref());
    }
    Ok(packages)
}

/// Whether `--refresh` was passed. Read by the TUI, which does its own
/// loading rather than going through `load_classified`.
pub fn refresh_requested() -> bool {
    REFRESH.load(std::sync::atomic::Ordering::Relaxed)
}

/// Loads packages (see [`load_packages`]) and classifies them against local
/// category overrides and notes.
fn load_classified(with_outdated: bool) -> Result<Vec<ClassifiedPackage>> {
    let packages = load_packages(with_outdated, refresh_requested())?;
    let state = State::load()?;
    Ok(classify::classify_all(
        packages,
        &state.categories,
        &state.notes,
    ))
}

fn cmd_scan(all: bool) -> Result<()> {
    let classified = classify::filter_on_request(load_classified(false)?, all);

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for pkg in &classified {
        *counts.entry(pkg.category.clone()).or_insert(0) += 1;
    }

    let scope = if all {
        "installed"
    } else {
        "explicitly installed"
    };
    println!("Scanned {} {scope} packages.\n", classified.len());

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Category", "Packages"]);
    for category in classify::known_categories() {
        let count = counts.get(&category).copied().unwrap_or(0);
        if count > 0 {
            table.add_row(vec![category, count.to_string()]);
        }
    }
    println!("{table}");

    let uncategorized = counts.get(classify::UNCATEGORIZED).copied().unwrap_or(0);
    if uncategorized > 0 {
        println!(
            "\n{uncategorized} package(s) are Uncategorized. Use `lagerregal list --category Uncategorized` \
             to see them, and `lagerregal category <name> <category>` to classify them yourself."
        );
    }

    Ok(())
}

#[derive(Serialize)]
struct PackageJson {
    name: String,
    kind: String,
    version: String,
    category: String,
    source: String,
    desc: String,
    homepage: String,
    tap: String,
    installed_on_request: bool,
    installed_at: Option<i64>,
    outdated: Option<String>,
    unmaintained: bool,
    unmaintained_reason: Option<String>,
    note: Option<String>,
}

impl From<&ClassifiedPackage> for PackageJson {
    fn from(cp: &ClassifiedPackage) -> Self {
        PackageJson {
            name: cp.package.name.clone(),
            kind: cp.package.kind.as_str().to_string(),
            version: cp.package.version.clone(),
            category: cp.category.clone(),
            source: cp.source.as_str().to_string(),
            desc: cp.package.desc.clone(),
            homepage: cp.package.homepage.clone(),
            tap: cp.package.tap.clone(),
            installed_on_request: cp.package.installed_on_request,
            installed_at: cp.package.installed_at,
            outdated: cp.package.outdated.clone(),
            unmaintained: cp.package.unmaintained,
            unmaintained_reason: cp.package.unmaintained_reason.clone(),
            note: cp.note.clone(),
        }
    }
}

fn cmd_list(category: Option<String>, json: bool, all: bool) -> Result<()> {
    let mut classified = classify::filter_on_request(load_classified(false)?, all);
    if let Some(ref category) = category {
        classified.retain(|p| p.category.eq_ignore_ascii_case(category));
    }
    classified.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.package.name.cmp(&b.package.name))
    });

    if json {
        let views: Vec<PackageJson> = classified.iter().map(PackageJson::from).collect();
        println!("{}", serde_json::to_string_pretty(&views)?);
        return Ok(());
    }

    if classified.is_empty() {
        println!(
            "No packages found{}.",
            category
                .map(|c| format!(" in category \"{c}\""))
                .unwrap_or_default()
        );
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Name", "Kind", "Category", "Version", "Description"]);
    for p in &classified {
        table.add_row(vec![
            Cell::new(&p.package.name),
            Cell::new(p.package.kind.as_str()),
            Cell::new(&p.category),
            Cell::new(&p.package.version),
            Cell::new(&p.package.desc),
        ]);
    }
    println!("{table}");

    Ok(())
}

fn cmd_show(name: &str) -> Result<()> {
    // Asks for update info up front rather than making a separate
    // `brew outdated` call afterwards - that second call would bypass the
    // cache and cost a second even on an otherwise warm run.
    let classified = load_classified(true)?;
    let pkg = classified
        .iter()
        .find(|p| p.package.name == name)
        .with_context(|| format!("no installed package named \"{name}\" found"))?;

    let update = pkg.package.outdated.clone();

    println!("{}  ({})", pkg.package.name, pkg.package.kind.as_str());
    match &update {
        Some(newer) => println!(
            "  Version:     {}  (update available: {newer})",
            pkg.package.version
        ),
        None => println!("  Version:     {}", pkg.package.version),
    }
    println!("  Category:    {} [{}]", pkg.category, pkg.source.as_str());
    println!("  Publisher:   {}", pkg.package.tap);
    println!("  Homepage:    {}", pkg.package.homepage);
    if pkg.package.unmaintained {
        println!(
            "  \u{26A0} No longer maintained ({})",
            pkg.package
                .unmaintained_reason
                .as_deref()
                .unwrap_or("unspecified reason")
        );
    }
    {
        let mut size_map = cache::load_sizes();
        let mut dirty = false;
        if let Some(size) = cache::size_or_compute(
            &mut size_map,
            &mut dirty,
            pkg.package.kind,
            &pkg.package.name,
            &pkg.package.version,
        ) {
            println!("  Size:        {}", details::format_size(size));
        }
        if dirty {
            cache::merge_sizes(&size_map);
        }
    }
    if let Some(installed_at) = pkg.package.installed_at {
        println!("  Installed:   {}", details::format_age(installed_at));
    }
    println!("  Description: {}", pkg.package.desc);
    if let Some(note) = &pkg.note {
        println!("  Note:        {note}");
    }

    let index: BTreeMap<String, &Package> = classified
        .iter()
        .map(|p| (p.package.name.clone(), &p.package))
        .collect();
    let (deps, truncated) = details::dependency_tree(&index, &pkg.package.name, 6, 100);
    if !deps.is_empty() {
        println!("\n  Dependencies:");
        for dep in &deps {
            let indent = "  ".repeat(dep.depth);
            match &dep.version {
                Some(version) => println!("  {indent}- {} ({version})", dep.name),
                None => println!("  {indent}- {} (not installed)", dep.name),
            }
        }
        if truncated {
            println!("    ... (truncated)");
        }
    }

    let required_by =
        details::reverse_dependencies(classified.iter().map(|p| &p.package), &pkg.package.name);
    if !required_by.is_empty() {
        println!("\n  Required by:");
        for name in &required_by {
            println!("    - {name}");
        }
    }

    Ok(())
}

fn cmd_note(name: &str, text: &str) -> Result<()> {
    let mut state = State::load()?;
    // An empty text clears the note instead of storing an empty string.
    if text.trim().is_empty() {
        if state.remove_note(name) {
            state.save()?;
            println!("Cleared the note for \"{name}\".");
        } else {
            println!("\"{name}\" has no note to clear.");
        }
        return Ok(());
    }
    state.set_note(name, text);
    state.save()?;
    println!("Saved note for \"{name}\".");
    Ok(())
}

fn cmd_category(name: &str, category: Option<&str>, reset: bool) -> Result<()> {
    let mut state = State::load()?;
    if reset {
        if state.remove_category(name) {
            state.save()?;
            println!("Cleared the manual category override for \"{name}\".");
        } else {
            println!("\"{name}\" had no manual category override to clear.");
        }
        return Ok(());
    }

    let Some(category) = category else {
        bail!("either a category or --reset is required");
    };
    state.set_category(name, category);
    state.save()?;
    println!("\"{name}\" is now manually classified as \"{category}\".");
    Ok(())
}

fn cmd_categories(all: bool, sizes: bool) -> Result<()> {
    let classified = classify::filter_on_request(load_classified(false)?, all);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for pkg in &classified {
        *counts.entry(pkg.category.clone()).or_insert(0) += 1;
    }
    // Seeded from the persistent size cache, so repeat runs only walk
    // packages whose installed version changed since last time.
    let size_totals = sizes.then(|| {
        let mut size_map = cache::load_sizes();
        let mut dirty = false;
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        for p in &classified {
            let size = cache::size_or_compute(
                &mut size_map,
                &mut dirty,
                p.package.kind,
                &p.package.name,
                &p.package.version,
            )
            .unwrap_or(0);
            *totals.entry(p.category.clone()).or_insert(0) += size;
        }
        if dirty {
            cache::merge_sizes(&size_map);
        }
        totals
    });

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    if size_totals.is_some() {
        table.set_header(vec!["Category", "Packages", "Size"]);
    } else {
        table.set_header(vec!["Category", "Packages"]);
    }
    for category in classify::known_categories() {
        let count = counts.get(&category).copied().unwrap_or(0);
        let mut row = vec![category.clone(), count.to_string()];
        if let Some(totals) = &size_totals {
            let size = totals.get(&category).copied().unwrap_or(0);
            row.push(if count > 0 {
                details::format_size(size)
            } else {
                "-".to_string()
            });
        }
        table.add_row(row);
    }
    println!("{table}");

    Ok(())
}

fn cmd_outdated(all: bool, json: bool) -> Result<()> {
    let mut classified = classify::filter_on_request(load_classified(true)?, all);
    classified.retain(|p| p.package.outdated.is_some());
    classified.sort_by(|a, b| a.package.name.cmp(&b.package.name));

    if json {
        let views: Vec<PackageJson> = classified.iter().map(PackageJson::from).collect();
        println!("{}", serde_json::to_string_pretty(&views)?);
        return Ok(());
    }

    if classified.is_empty() {
        println!("Everything is up to date.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Name", "Kind", "Installed", "Available"]);
    for p in &classified {
        table.add_row(vec![
            Cell::new(&p.package.name),
            Cell::new(p.package.kind.as_str()),
            Cell::new(&p.package.version),
            Cell::new(p.package.outdated.as_deref().unwrap_or("")),
        ]);
    }
    println!("{table}");
    println!(
        "\n{} package(s) have updates available. Run `brew upgrade` (or `brew upgrade --cask`) to update them.",
        classified.len()
    );

    Ok(())
}

fn cmd_unmaintained(all: bool, json: bool) -> Result<()> {
    let mut classified = classify::filter_on_request(load_classified(false)?, all);
    classified.retain(|p| p.package.unmaintained);
    classified.sort_by(|a, b| a.package.name.cmp(&b.package.name));

    if json {
        let views: Vec<PackageJson> = classified.iter().map(PackageJson::from).collect();
        println!("{}", serde_json::to_string_pretty(&views)?);
        return Ok(());
    }

    if classified.is_empty() {
        println!("No installed packages are marked deprecated or disabled.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Name", "Kind", "Version", "Reason"]);
    for p in &classified {
        table.add_row(vec![
            Cell::new(&p.package.name),
            Cell::new(p.package.kind.as_str()),
            Cell::new(&p.package.version),
            Cell::new(p.package.unmaintained_reason.as_deref().unwrap_or("")),
        ]);
    }
    println!("{table}");
    println!(
        "\n{} package(s) are no longer maintained upstream in Homebrew. Consider a replacement or removing them.",
        classified.len()
    );

    Ok(())
}

/// Dependency-only packages that nothing installed still depends on - what
/// `brew autoremove` would remove. No `--all` flag: orphans are by
/// definition dependency-only, so the on-request scoping never applies.
fn cmd_orphans(json: bool) -> Result<()> {
    let classified = load_classified(false)?;
    let orphans = details::autoremove_candidates(classified.iter().map(|p| &p.package));
    let mut classified: Vec<_> = classified
        .into_iter()
        .filter(|p| orphans.contains(&p.package.name))
        .collect();
    classified.sort_by(|a, b| a.package.name.cmp(&b.package.name));

    if json {
        let views: Vec<PackageJson> = classified.iter().map(PackageJson::from).collect();
        println!("{}", serde_json::to_string_pretty(&views)?);
        return Ok(());
    }

    if classified.is_empty() {
        println!("No orphaned packages - everything installed as a dependency is still needed.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Name", "Version", "Description"]);
    for p in &classified {
        table.add_row(vec![
            Cell::new(&p.package.name),
            Cell::new(&p.package.version),
            Cell::new(&p.package.desc),
        ]);
    }
    println!("{table}");
    println!(
        "\n{} orphaned package(s). `brew autoremove` removes them all at once.",
        classified.len()
    );

    Ok(())
}

fn cmd_update(name: &str) -> Result<()> {
    let classified = load_classified(false)?;
    let pkg = classified
        .iter()
        .find(|p| p.package.name == name)
        .with_context(|| format!("no installed package named \"{name}\" found"))?;

    println!("==> brew upgrade {name}");
    let status = homebrew::upgrade(pkg.package.kind, name)?;
    if !status.success() {
        bail!("`brew upgrade` for \"{name}\" did not complete successfully");
    }
    Ok(())
}

fn cmd_snapshot_save(name: &str, all: bool) -> Result<()> {
    let classified = classify::filter_on_request(load_classified(false)?, all);
    let snap = snapshot::Snapshot::capture(&classified);
    let count = snap.packages.len();
    snap.save(name)?;
    println!("Saved snapshot \"{name}\" with {count} package(s).");
    Ok(())
}

fn cmd_snapshot_diff(name: &str, all: bool) -> Result<()> {
    let Some(old) = snapshot::Snapshot::load(name)? else {
        bail!("no snapshot named \"{name}\" found - run `lagerregal snapshot save {name}` first");
    };

    let classified = classify::filter_on_request(load_classified(false)?, all);
    let current: BTreeMap<String, String> = classified
        .iter()
        .map(|p| (p.package.name.clone(), p.package.version.clone()))
        .collect();
    let diff = snapshot::diff(&old, &current);

    let taken = details::format_age(old.taken_at);
    println!("Comparing against snapshot \"{name}\" (saved {taken}):\n");

    if diff.is_empty() {
        println!("No changes since this snapshot.");
        return Ok(());
    }

    if !diff.added.is_empty() {
        println!("Added ({}):", diff.added.len());
        for (name, version) in &diff.added {
            println!("  + {name} ({version})");
        }
    }
    if !diff.removed.is_empty() {
        println!("Removed ({}):", diff.removed.len());
        for (name, version) in &diff.removed {
            println!("  - {name} ({version})");
        }
    }
    if !diff.changed.is_empty() {
        println!("Changed version ({}):", diff.changed.len());
        for (name, old_version, new_version) in &diff.changed {
            println!("  ~ {name} ({old_version} -> {new_version})");
        }
    }

    Ok(())
}
