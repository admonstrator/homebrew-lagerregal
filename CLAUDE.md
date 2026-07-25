# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`lagerregal` is a Rust CLI/TUI that reads a user's installed Homebrew formulae and casks, classifies them into categories (Homebrew itself has no such concept), and gives a searchable overview with room for personal notes. It's a single binary crate (no library target) meant to eventually be distributed via a Homebrew tap.

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

There is no library target, so `cargo test --lib` does not work - unit tests live inline in each `src/*.rs` module and run as part of the `--bin lagerregal` test target (plain `cargo test` handles this correctly).

This repo/sandbox has no `brew` binary and no macOS. Anything that shells out to `brew` (`homebrew.rs::installed_packages`) cannot be exercised live here - tests instead run `parse_brew_json` against `tests/fixtures/brew_installed.json`. The interactive TUI (`tui.rs`) also can't be visually verified outside a real terminal/macOS. When changing either, reason carefully and rely on the fixture-based tests; final verification needs a real Mac with Homebrew installed.

## Architecture

Module responsibilities (`src/`):

- `main.rs` - clap dispatch + one `cmd_*` function per subcommand (`scan`, `list`, `show`, `note`, `category`, `categories`). `load_classified()` is the shared entry point: shells out to `brew`, loads local state, classifies. Always fetches live from `brew` rather than caching raw package data, so results stay current; only manual overrides/notes are persisted.
- `cli.rs` - clap `Subcommand` enum defining the CLI surface.
- `homebrew.rs` - runs `brew info --json=v2 --installed` and parses it into a normalized `Package` (formula or cask). `parse_brew_json` is split out from the `brew`-invoking function specifically so it's testable via a fixture.
- `classify.rs` - the classification pipeline and `ClassifiedPackage`/`ClassificationSource` types.
- `src/data/categories.toml` - the curated data, embedded into the binary at compile time via `include_str!` (parsed once into a `OnceLock`). Two tables: `[curated]` (exact `package-name = "Category"` map) and `[[heuristic]]` (an ordered array of `{category, keywords}` - order matters, first match wins).
- `store.rs` - persists manual category overrides and notes (never raw package data) to a TOML file via the `directories` crate (e.g. `~/Library/Application Support/lagerregal/state.toml` on macOS).
- `tui.rs` - the `ratatui`/`crossterm` interactive dashboard: category sidebar, package list, detail pane, plus inline note/category/filter editing.

### Classification precedence

In `classify::classify()`, highest wins:

1. Manual override (from `store::State`, set via `lagerregal category <name> <category>` or the TUI's `c` key)
2. Curated exact-name lookup (`categories.toml`'s `[curated]` table)
3. Keyword heuristic - matched against **name + desc combined**, not desc alone. This matters: many packages (notably the thousands of `font-*` Homebrew casks) ship with no `desc` at all, only a name, so keywords like `font-` are designed to match on the name.
4. Falls back to `"Uncategorized"`.

When adding to `categories.toml`, prefer a new/expanded heuristic keyword over a pile of individual curated entries where possible - it generalizes to packages not yet seen. The curated list and heuristics were tuned by running the classifier against real data (a live user's `brew info --json=v2 --installed` output, and the full `homebrew-core`/`homebrew-cask` catalogs) rather than guessed; if you change the taxonomy, it's worth re-checking against `tests/fixtures/brew_installed.json` and, ideally, a larger real-world dump.

### On-request vs. dependency filtering

`Package.installed_on_request` distinguishes packages the user explicitly installed from ones pulled in only as a dependency of something else (casks are always treated as on-request; there's no such distinction for them in Homebrew). By default, `list`/`scan`/`categories` and the TUI only show on-request packages via `classify::filter_on_request` - dependency-only formulae are noise (in real-world data, roughly 2/3 of installed formulae are dependency-only). `--all` (CLI flag) / `d` (TUI key) opt back into the full set. `show <name>` always searches everything, unfiltered, since the user is asking about one specific package by name.

### Distribution

Built with `cargo-dist` (`dist-workspace.toml`) targeting macOS only (`aarch64-apple-darwin` + `x86_64-apple-darwin`), publishing a generated Homebrew formula to a separate `homebrew-lagerregal` tap repo on tagged releases. This config was hand-verified against the actual `cargo-dist` 0.32.0 source (not run end-to-end, since installing `cargo-dist` and Homebrew itself aren't possible in this sandbox) - see the "Publishing to your own Homebrew tap" section in `README.md` for the manual one-time setup steps (creating the tap repo, `HOMEBREW_TAP_TOKEN` secret, running `cargo dist init`/`generate` on a real Mac).
