# lagerregal

A CLI and TUI for your installed [Homebrew](https://brew.sh) packages: classifies every formula and cask into categories (Networking, Security, DNS, Media & Graphics, …), shows what's outdated or no longer maintained, and lets you keep notes on *why* you installed something.

Homebrew has no concept of categories or tags. `lagerregal` fills that gap.

> **Apple Silicon only.** Builds and releases target `aarch64-apple-darwin`.

## Installation

```sh
brew tap admonstrator/lagerregal https://github.com/admonstrator/lagerregal
brew install lagerregal
```

This repo doubles as its own Homebrew tap, which is why the `tap` command takes an explicit URL - there's no separate `homebrew-lagerregal` repository to maintain.

Or from source:

```sh
cargo build --release
./target/release/lagerregal
```

## Usage

Running `lagerregal` with no arguments launches the TUI. Individual subcommands:

```sh
lagerregal scan                          # category summary of installed packages
lagerregal list                          # table of explicitly-installed packages (alias: ls)
lagerregal list --category DNS           # filter by category
lagerregal list --json                   # machine-readable output
lagerregal list --all                    # include dependency-only packages
lagerregal show <name>                   # full details for one package
lagerregal note <name> "<text>"          # save a personal note
lagerregal category <name> <category>    # manually (re)classify a package
lagerregal category <name> --reset       # clear a manual override
lagerregal categories                    # all categories with package counts
lagerregal categories --sizes            # ...plus total on-disk size per category
lagerregal outdated                      # packages with an update available
lagerregal unmaintained                  # packages Homebrew marked deprecated/disabled
lagerregal update <name>                 # upgrade one package via `brew upgrade`
lagerregal snapshot save [name]          # snapshot current packages + versions
lagerregal snapshot diff [name]          # compare against a saved snapshot
```

Global flags: `--refresh` (bypass the cache), `--no-icons` (plain Unicode instead of Nerd Font glyphs).

By default only packages you explicitly installed are shown — the C libraries Homebrew pulled in as dependencies are hidden. Pass `--all`, or press `d` in the TUI, to include them.

`show` prints on-disk size, install date, publishing tap, update status, deprecation status, and a recursive dependency tree.

Snapshots are useful around a reinstall or cleanup: `snapshot save before-cleanup`, do your thing, then `snapshot diff before-cleanup` to see exactly what was added, removed, or changed version.

## TUI

| Key | Action |
|-----|--------|
| `Tab` | Switch focus between sidebar and package list |
| `↑`/`↓`, `j`/`k` | Move selection |
| `/` | Filter by name, description, or note |
| `s` | Cycle sort order: name / size / install date |
| `Enter` | Open the action menu for the selected package |
| `u` | Update selected package(s) via `brew upgrade` (asks first) |
| `n` | Add/edit a note |
| `c` | Set category (applies to all multi-selected) |
| `R` | Clear a manual category override |
| `o` | Open the package's homepage |
| `y` | Copy the package name to the clipboard |
| `Space` | Toggle multi-select |
| `d` | Toggle dependency-only packages |
| `?` | Full keybinding overlay |
| `Esc` | Clear selection, cancel input, or quit |
| `q` | Quit |

The sidebar has two pseudo-categories pinned above the taxonomy — **Outdated** and **Unmaintained** — that filter to exactly those packages. They're views, not real categories: selecting one doesn't change any package's classification.

`u` is the one action that changes your Homebrew install, so it always confirms first, then hands the terminal over to `brew` so its own progress output renders normally. Press Enter when it's done to return; the list refreshes automatically.

Multi-select (`Space`) drives bulk operations: tick off a run of packages, then `c` to categorize them all at once, `R` to reset their overrides, or `u` to update every outdated one together.

### Mouse

| Action | Effect |
|--------|--------|
| Click a sidebar / package row | Select it and focus that pane |
| Double-click a package row | Select it and open its action menu |
| Right-click a package row | Open its action menu directly |
| Scroll wheel | Move the selection |
| Click a menu item | Activate it |
| Click outside a popup | Dismiss it |

Mouse capture is released while `brew upgrade` owns the terminal, so your terminal's own selection behavior isn't hijacked mid-upgrade.

### Looks

The TUI uses the [Catppuccin Mocha](https://catppuccin.com) palette — fixed RGB values rather than the terminal's ANSI slots, so hues stay balanced regardless of your colorscheme.

Everything carries meaning through both an icon and a color:

- **Each category** has its own glyph and color, used consistently across sidebar, list, and detail pane.
- **Formula vs. cask** is a terminal vs. monitor glyph, tinted by classification source (manual / curated / heuristic / uncategorized).
- **Status markers**: update available, no longer maintained, manually classified, multi-selected.
- **On-disk size** is traffic-lighted green → yellow → orange → red.
- The header carries live counts plus a bar showing what share of your install is current.

Icons are [Nerd Font](https://www.nerdfonts.com) glyphs from the Font Awesome range that every Nerd Font release ships. Without a Nerd Font they'd render as tofu boxes, so there's a fallback:

```sh
lagerregal --no-icons          # or: export LAGERREGAL_NO_ICONS=1
```

That swaps every glyph for widely-supported Unicode (`↑`, `⚠`, `●`, `$`, `▣`) and keeps column alignment identical.

## Startup speed

Reading Homebrew's state is the entire startup cost: `brew info --json=v2 --installed` takes ~1.9s and `brew outdated --json=v2` another ~1.0s on a ~370-package install. Almost none of that is Homebrew starting up (`brew --version` is 0.1s) — it's Homebrew loading every installed formula and cask definition, so no flag makes it cheaper.

Two things bring it down:

1. **The two calls run concurrently** rather than back to back, since they're independent.
2. **Results are cached locally**, so a run where nothing changed skips `brew` entirely.

| | before | after |
|---|---|---|
| first run / after a change | ~2.9s | ~1.9s |
| subsequent runs | ~2.9s | ~0.04s |

The cache lives at `~/Library/Application Support/lagerregal/cache.json`, validated against a fingerprint of:

- the **Cellar and Caskroom listings** — entries appear and disappear on install/uninstall, and a package's directory mtime bumps on upgrade (the new version directory lands next to the old one, updating the parent's mtime);
- **Homebrew's API/metadata cache**, which `brew update` rewrites — that's what can change a description or a deprecated flag without anything in the Cellar moving.

Computing the fingerprint costs ~1ms against the ~2.9s it skips. A cache that's missing, corrupt, or written by an older build is ignored rather than surfaced as an error — a cache should cost you time, never correctness. Force a reload with `--refresh`.

## How classification works

Precedence, highest wins:

1. **Manual override** — `lagerregal category <name> <category>`, or `c` in the TUI
2. **Curated list** (`src/data/categories.toml`) — exact package-name matches
3. **Keyword heuristic** — matched against the package's name *and* description, so the thousands of `font-*` casks that carry no description still classify from their name
4. **Uncategorized** — fallback

Built-in categories: Security, AI & Machine Learning, DNS, Cryptography, Cryptocurrency, Networking, Media & Graphics, Fonts, Documents & PDF, Monitoring, Databases, Cloud & Infra, Dev Tools & Languages, System Utilities, Communication & Browsers, Games & Emulation, Archives & Compression, Peripherals & Input, Productivity. You aren't limited to these — `lagerregal category <name> <anything>` accepts any name.

The curated list and keywords were tuned against the full official catalog (~8,500 [homebrew-core](https://github.com/Homebrew/homebrew-core) formulae and ~7,700 [homebrew-cask](https://github.com/Homebrew/homebrew-cask) casks) as well as a real ~180-package install, where they reach 0 `Uncategorized`. Against the *entire* catalog a large `Uncategorized` tail remains, which is expected: it's dominated by narrow single-purpose libraries nobody installs directly. The goal is covering what people actually install, not inventing a category for every library in existence.

Manual overrides and notes are stored at `~/Library/Application Support/lagerregal/state.toml`. Package data itself is never stored there — it comes from `brew`, via the cache described above.

## Privacy

Everything runs locally against your existing Homebrew install. No network access, no API keys, no telemetry. `brew outdated` runs with `HOMEBREW_NO_AUTO_UPDATE=1` so it never triggers a network fetch on its own.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Homebrew JSON parsing, classification, the cache fingerprint, and the theme's category table all run against fixtures or temp directories rather than a real `brew`, so the test suite runs anywhere Rust does. Exercising the actual `brew` shell-outs and the TUI's rendering needs a Mac with Homebrew installed.

CI (`.github/workflows/ci.yml`) runs the same four commands on every push and pull request.

## Releasing

Releases are cut by pushing a tag:

```sh
# bump version in Cargo.toml first, then:
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` then:

1. checks that the tag matches the version in `Cargo.toml`, failing loudly if it doesn't;
2. builds the `aarch64-apple-darwin` binary and packages it as a tarball;
3. publishes a GitHub Release with that tarball attached;
4. regenerates `Formula/lagerregal.rb` with the release URL and its SHA-256, and commits it to the default branch.

No setup or secrets are required — the formula lives in this repo, so the workflow's built-in `GITHUB_TOKEN` is enough to update it. `Formula/lagerregal.rb` is generated, not hand-maintained; change the workflow rather than the file.

## License

MIT
