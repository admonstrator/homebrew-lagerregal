mod classify;
mod cli;
mod homebrew;
mod store;
mod tui;

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use clap::Parser;
use comfy_table::{presets::UTF8_FULL, Cell, Table};
use serde::Serialize;

use classify::ClassifiedPackage;
use cli::{Cli, Command};
use store::State;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Scan { all }) => cmd_scan(all),
        Some(Command::List {
            category,
            json,
            all,
        }) => cmd_list(category, json, all),
        Some(Command::Show { name }) => cmd_show(&name),
        Some(Command::Note { name, text }) => cmd_note(&name, &text),
        Some(Command::Category { name, category }) => cmd_category(&name, &category),
        Some(Command::Categories { all }) => cmd_categories(all),
        Some(Command::Tui) | None => tui::run(),
    }
}

/// Fetches the current Homebrew install state and classifies it against
/// local overrides/notes. This always talks to `brew` live rather than
/// caching raw package data, so results reflect what's actually installed.
fn load_classified() -> Result<Vec<ClassifiedPackage>> {
    let packages = homebrew::installed_packages().context(
        "Could not read installed Homebrew packages. Is Homebrew installed and on your PATH?",
    )?;
    let state = State::load()?;
    Ok(classify::classify_all(
        packages,
        &state.categories,
        &state.notes,
    ))
}

fn cmd_scan(all: bool) -> Result<()> {
    let classified = classify::filter_on_request(load_classified()?, all);

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
            note: cp.note.clone(),
        }
    }
}

fn cmd_list(category: Option<String>, json: bool, all: bool) -> Result<()> {
    let mut classified = classify::filter_on_request(load_classified()?, all);
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
    let classified = load_classified()?;
    let pkg = classified
        .iter()
        .find(|p| p.package.name == name)
        .with_context(|| format!("no installed package named \"{name}\" found"))?;

    println!("{}  ({})", pkg.package.name, pkg.package.kind.as_str());
    println!("  Version:    {}", pkg.package.version);
    println!("  Category:   {} [{}]", pkg.category, pkg.source.as_str());
    println!("  Tap:        {}", pkg.package.tap);
    println!("  Homepage:   {}", pkg.package.homepage);
    println!("  Description: {}", pkg.package.desc);
    if let Some(note) = &pkg.note {
        println!("  Note:       {note}");
    }

    Ok(())
}

fn cmd_note(name: &str, text: &str) -> Result<()> {
    let mut state = State::load()?;
    state.set_note(name, text);
    state.save()?;
    println!("Saved note for \"{name}\".");
    Ok(())
}

fn cmd_category(name: &str, category: &str) -> Result<()> {
    let mut state = State::load()?;
    state.set_category(name, category);
    state.save()?;
    println!("\"{name}\" is now manually classified as \"{category}\".");
    Ok(())
}

fn cmd_categories(all: bool) -> Result<()> {
    let classified = classify::filter_on_request(load_classified()?, all);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for pkg in &classified {
        *counts.entry(pkg.category.clone()).or_insert(0) += 1;
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["Category", "Packages"]);
    for category in classify::known_categories() {
        let count = counts.get(&category).copied().unwrap_or(0);
        table.add_row(vec![category, count.to_string()]);
    }
    println!("{table}");

    Ok(())
}
