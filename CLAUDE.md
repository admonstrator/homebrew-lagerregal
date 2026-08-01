# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`lagerregal` is a Rust CLI/TUI that reads a user's installed Homebrew formulae and casks, classifies them into categories (Homebrew itself has no such concept), and gives a searchable overview with room for personal notes, update/pin/uninstall handling, and dependency insight. It's a single binary crate (no library target), distributed via the shared tap [`admonstrator/homebrew-tap`](https://github.com/admonstrator/homebrew-tap) (`brew tap admonstrator/tap`).

## Commands

```sh
cargo build                                  # debug build
cargo build --release                        # release build (lto + strip enabled)
cargo test                                   # run all tests
cargo test <test_name>                       # run a single test by name (e.g. `cargo test curated_lookup_wins`)
cargo clippy --all-targets -- -D warnings    # lint; CI-equivalent check, must be clean
cargo fmt                                    # format
cargo fmt --check                            # verify formatting without changing files
./target/debug/lagerregal <subcommand>       # run locally
```

There is no library target, so `cargo test --lib` does not work - unit tests live inline in each module and run as part of the `--bin lagerregal` test target (plain `cargo test` handles this correctly).

When developing on a Mac with Homebrew installed, everything can be exercised live - including the TUI, which is best verified inside `tmux` (`tmux send-keys` for input, SGR escape sequences for mouse events, `tmux capture-pane -p` to read frames). In environments *without* `brew`, anything that shells out to it can't run; tests deliberately don't need it: parsing runs against `tests/fixtures/brew_installed.json`, and full TUI frames render into ratatui's `TestBackend`.

## Architecture

Module responsibilities (`src/`):

- `main.rs` - clap dispatch + one `cmd_*` function per subcommand (`scan`, `list`, `show`, `note`, `category`, `categories`, `outdated`, `unmaintained`, `orphans`, `update`, `snapshot`, `tui`). `load_packages()` is the shared loader (parallel `brew` calls + cache), `load_classified()` layers local state and classification on top.
- `cli.rs` - clap `Subcommand` enum defining the CLI surface, plus the global `--refresh` / `--no-icons` flags.
- `homebrew.rs` - runs `brew info --json=v2 --installed` / `brew outdated --json=v2` and parses them into a normalized `Package`; also the mutating shell-outs (`upgrade`, `uninstall`, `set_pinned`). `parse_brew_json` is split out from the `brew`-invoking function specifically so it's testable via a fixture. `Package.dependencies` holds *directly declared* runtime deps (for the dependency tree); `Package.indirect_dependencies` holds the transitive tail from the install receipts - orphan analysis needs both.
- `classify.rs` - the classification pipeline and `ClassifiedPackage`/`ClassificationSource` types.
- `src/data/categories.toml` - the curated data, embedded into the binary at compile time via `include_str!` (parsed once into a `OnceLock`). Two tables: `[curated]` (exact `package-name = "Category"` map) and `[[heuristic]]` (an ordered array of `{category, keywords, exclude}` - order matters, first match wins; `exclude` phrases veto a category even when a keyword matched).
- `details.rs` - on-disk sizes (`package_size`, follows cask symlinks with a dev/ino cycle guard), install-age formatting, `dependency_tree`, `reverse_dependencies`, and `autoremove_candidates` (orphan fixpoint over direct + indirect edges; verified against `brew autoremove --dry-run`).
- `cache.rs` - the fingerprinted package/outdated cache (`cache.json`, FNV-1a over Cellar + Caskroom + `<brew --cache>/api` listings) plus the persistent size map keyed `name|version` (survives fingerprint changes; pruned to installed versions on rewrite). Bump `CACHE_VERSION` whenever `Package` or the file shape changes.
- `store.rs` - persists manual category overrides and notes (never raw package data) to a TOML file via the `directories` crate (e.g. `~/Library/Application Support/lagerregal/state.toml` on macOS).
- `snapshot.rs` - named snapshots of installed packages/versions and their diffing.
- `theme.rs` - Catppuccin Mocha palette (fixed RGB), Nerd-Font glyphs (Font Awesome range only, with plain-Unicode fallbacks) and the per-category style table, drift-guarded against `categories.toml` by a test.
- `tui/` - the `ratatui`/`crossterm` dashboard, split by responsibility: `app.rs` (state, the `ListRow` row model - search results are grouped under category headings, so a table row index is *not* a package index; all selection/click paths go through `visible_rows`), `input.rs` (keyboard/mouse handlers and actions), `draw.rs` (all rendering), `mod.rs` (event loop, terminal setup, the suspend-and-run-`brew` seam), `tests.rs` (logic tests + `TestBackend` frame snapshots).

### Classification precedence

In `classify::classify()`, highest wins:

1. Manual override (from `store::State`, set via `lagerregal category <name> <category>` or the TUI's `c` picker)
2. Curated exact-name lookup (`categories.toml`'s `[curated]` table)
3. Keyword heuristic - matched against **name + desc combined**, not desc alone. This matters: many packages (notably the thousands of `font-*` Homebrew casks) ship with no `desc` at all, only a name, so keywords like `font-` are designed to match on the name. Keywords of ≤4 characters must start at a word boundary; `exclude` phrases veto a category.
4. Falls back to `"Uncategorized"`.

When adding to `categories.toml`, prefer a new/expanded heuristic keyword over a pile of individual curated entries where possible - it generalizes to packages not yet seen. The curated list and heuristics were tuned by running the classifier against real data (a live user's `brew info --json=v2 --installed` output, and the full `homebrew-core`/`homebrew-cask` catalogs) rather than guessed; if you change the taxonomy, it's worth re-checking against `tests/fixtures/brew_installed.json` and, ideally, a larger real-world dump. (A longest-match-wins scheme was tried and rejected on measured results.)

### On-request vs. dependency filtering

`Package.installed_on_request` distinguishes packages the user explicitly installed from ones pulled in only as a dependency of something else (casks are always treated as on-request; there's no such distinction for them in Homebrew). By default, `list`/`scan`/`categories` and the TUI only show on-request packages via `classify::filter_on_request` - dependency-only formulae are noise (in real-world data, roughly 2/3 of installed formulae are dependency-only). `--all` (CLI flag) / `d` (TUI key) opt back into the full set. Exceptions that deliberately ignore the scoping: `show <name>` (one specific package by name), the TUI's search (`/` spans everything), and the Orphaned view / `orphans` subcommand (orphans are dependency-only by definition).

### Distribution

The formula is **not** in this repo. `.github/workflows/release.yml` builds four targets on tag push (or `workflow_dispatch` with a `tag` input) - `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-musl`, `x86_64-unknown-linux-musl` - and publishes a tarball each **twice**: to this repo's GitHub release, and to [`admonstrator/homebrew-tap`](https://github.com/admonstrator/homebrew-tap), the shared tap for everything distributed via `brew` here. `scripts/update-tap.sh` then regenerates `Formula/lagerregal.rb` pointing at the *tap's* copies and pushes it to the tap's `main`. Downloading from the tap rather than from here is what every project in the tap does (mantaray, whose source repo is private, has no other option), so one public repo serves every formula's binaries. Tap release tags are project-prefixed - `lagerregal-v0.5.0` - because the tap holds releases for several projects and bare `v*` tags would collide; the tarballs inside keep the plain `lagerregal-<version>-<target>.tar.gz` name. The cross-repo publish and push both need a token beyond the default `GITHUB_TOKEN` (which only has permissions in this repo): `GH_TAP_PAT`, a fine-grained PAT scoped to `contents: write` on `homebrew-tap` only, stored as a secret on this repo. `brew tap admonstrator/tap` is what users run; don't assume this repo's name maps 1:1 to the tap name. Checksums are computed once in the release job (one `sha256sum`, one runner) and written as `.sha256` sidecars that ship with both releases and feed `update-tap.sh`. CI (`ci.yml`) runs fmt/clippy/tests once per target, each on a runner of that target's own architecture. Releasing = bump `Cargo.toml` version, commit, `git tag vX.Y.Z && git push --tags`.

The Linux targets are musl, not gnu, so the binary links statically and doesn't inherit the glibc version of the runner image that built it. Nothing in the dependency tree compiles C, so `rustup target add <musl triple>` is the only setup a musl build needs - no `musl-tools`, no cross linker. The generated formula selects its URL through Homebrew's `on_macos`/`on_linux` + `on_arm`/`on_intel` blocks; branching on `Hardware::CPU.arm?` alone would hand an ARM Linux host the macOS arm64 tarball. Nothing in `src/` is macOS-specific - the Cellar/Caskroom/cache roots all come from `brew --cellar` / `--caskroom` / `--cache`, and `formulae`/`casks` are both `#[serde(default)]` so Linux Homebrew's cask-less JSON parses fine.
