# lagerregal

A small CLI/TUI tool that reads your installed [Homebrew](https://brew.sh) formulae and casks, classifies them into categories (Networking, Security, DNS, Media & Graphics, AI & Machine Learning, ...), and gives you a searchable overview - with room for your own notes on why you installed something.

Homebrew itself has no concept of categories or tags. `lagerregal` fills that gap with:

1. A curated name → category lookup for well-known packages (validated against a real-world `brew info --json=v2 --installed` dump so it actually covers what people have installed, not just a hand-picked wishlist)
2. A keyword heuristic that scans each package's description for anything not in the curated list
3. Manual overrides and personal notes you set yourself, which always win

By default, only packages you explicitly installed are shown - the dozens of C libraries Homebrew pulls in as *dependencies* of those packages are hidden (pass `--all`, or press `d` in the TUI, to include them). Everything runs locally - no network access, no API keys required.

## Installation

### From source (today)

```sh
cargo build --release
./target/release/lagerregal
```

### Via Homebrew (once the tap is published, see below)

```sh
brew tap admonstrator/lagerregal
brew install lagerregal
```

## Usage

Running `lagerregal` with no arguments launches the interactive TUI dashboard. Individual subcommands are also available:

```sh
lagerregal scan                          # re-read installed packages, print a category summary
lagerregal list                          # table of explicitly-installed packages (alias: ls)
lagerregal list --category DNS           # filter by category
lagerregal list --json                   # machine-readable output
lagerregal list --all                    # also include dependency-only packages
lagerregal show <name>                   # details for a single package (always searches everything)
lagerregal note <name> "<text>"          # save a personal note ("why did I install this?")
lagerregal category <name> <category>    # manually (re)classify a package
lagerregal categories                    # list all categories with package counts
lagerregal tui                           # launch the dashboard explicitly
```

### TUI keybindings

| Key       | Action                                   |
|-----------|-------------------------------------------|
| `Tab`     | Switch focus between category sidebar and package list |
| `↑`/`↓`, `j`/`k` | Move selection |
| `/`       | Filter/search packages by name or description |
| `n`       | Add/edit a note for the selected package |
| `c`       | Manually set the category of the selected package |
| `d`       | Toggle showing dependency-only packages |
| `Enter`   | Confirm filter/note/category input |
| `Esc`     | Cancel input, or quit from the normal view |
| `q`       | Quit |

## How classification works

Precedence, highest wins:

1. **Manual override** - set via `lagerregal category <name> <category>` or the `c` key in the TUI
2. **Curated list** (`src/data/categories.toml`) - exact package name matches
3. **Keyword heuristic** - matches keywords against the package's `desc` field
4. **Uncategorized** - fallback if nothing matched

Built-in categories: Security, AI & Machine Learning, DNS, Cryptography, Networking, Media & Graphics, Documents & PDF, Monitoring, Databases, Cloud & Infra, Dev Tools & Languages, System Utilities, Communication & Browsers, Productivity. You're not limited to these - `lagerregal category <name> <anything>` accepts any category name you want.

Manual overrides and notes are stored locally (not the raw package data, which is always re-read live from `brew` so it stays current) at:

- macOS: `~/Library/Application Support/lagerregal/state.toml`

Run `lagerregal scan` any time (e.g. after `brew install`/`brew upgrade`) to see an updated category breakdown and spot newly `Uncategorized` packages worth classifying yourself. The curated list and heuristics were tuned against a real ~180-package `brew info --json=v2 --installed` dump (explicitly-installed formulae + casks), not just guessed - that run came back with 0 packages left `Uncategorized`, though your own mix of packages will vary.

## Development

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt
```

Tests for Homebrew JSON parsing and the classification pipeline run against a fixture file (`tests/fixtures/brew_installed.json`) rather than a real `brew` binary, so they run anywhere Rust does. Actually exercising `brew` shell-outs and the TUI's rendering/interaction needs a real macOS machine with Homebrew installed.

## Publishing to your own Homebrew tap

`lagerregal` is built with [`cargo-dist`](https://opensource.axo.dev/cargo-dist/) so tagged releases produce precompiled macOS binaries (`aarch64-apple-darwin` and `x86_64-apple-darwin`) and push a generated Homebrew formula to a tap repository. `dist-workspace.toml` in this repo has a starting configuration, but a few one-time manual steps happen outside this repo:

1. **Install cargo-dist** on your Mac: `brew install cargo-dist`
2. **Create the tap repository**: a new GitHub repo named `admonstrator/homebrew-lagerregal` (the `homebrew-` prefix is required by Homebrew's tap naming convention)
3. **Add a `HOMEBREW_TAP_TOKEN` secret** to *this* repo (`admonstrator/lagerregal`): a GitHub personal access token with push access to the tap repo
4. **Run `cargo dist init`** in this repo to confirm/regenerate `dist-workspace.toml` against your installed cargo-dist version, then **`cargo dist generate`** to (re)generate `.github/workflows/release.yml`
5. **Tag a release**, e.g. `git tag v0.1.0 && git push --tags` - CI builds the binaries, creates a GitHub Release, and pushes the formula to the tap

After that, anyone can run:

```sh
brew tap admonstrator/lagerregal
brew install lagerregal
```
