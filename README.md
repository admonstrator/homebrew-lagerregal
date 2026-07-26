# lagerregal

A CLI and TUI for your installed [Homebrew](https://brew.sh) packages: classifies every formula and cask into categories (Networking, Security, DNS, Media & Graphics, …), shows what's outdated or no longer maintained, and lets you keep notes on *why* you installed something.

Homebrew has no concept of categories or tags. `lagerregal` fills that gap.

> **macOS and Linux.** Releases target `aarch64-apple-darwin` and `x86_64-apple-darwin` on macOS, plus `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl` on Linux — statically linked, so they don't depend on the host's glibc version.
>
> On Linux, Homebrew has no casks, so `lagerregal` sees formulae only. Everything else works the same; the category taxonomy just leans heavily on casks, so expect a thinner picture than on a Mac.

## Installation

```sh
brew tap admonstrator/tap
brew install lagerregal
```

The formula is generated and maintained in [`admonstrator/homebrew-tap`](https://github.com/admonstrator/homebrew-tap), the shared tap for everything I distribute via Homebrew. This repo's release workflow pushes the regenerated formula there on every tagged release.

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
lagerregal orphans                       # dependency-only packages nothing needs anymore
lagerregal update <name>                 # upgrade one package via `brew upgrade`
lagerregal snapshot save [name]          # snapshot current packages + versions
lagerregal snapshot diff [name]          # compare against a saved snapshot
```

Global flags: `--refresh` (bypass the cache), `--no-icons` (plain Unicode instead of Nerd Font glyphs).

By default only packages you explicitly installed are shown — the C libraries Homebrew pulled in as dependencies are hidden. Pass `--all`, or press `d` in the TUI, to include them.

`show` prints on-disk size, install date, publishing tap, update status, deprecation status, a recursive dependency tree, and the reverse view — which installed packages require this one.

Snapshots are useful around a reinstall or cleanup: `snapshot save before-cleanup`, do your thing, then `snapshot diff before-cleanup` to see exactly what was added, removed, or changed version.

## TUI

| Key | Action |
|-----|--------|
| `Tab` | Switch focus between sidebar and package list |
| `↑`/`↓`, `j`/`k` | Move selection |
| `/` | Search all categories, live while you type |
| `s` | Cycle sort order: name / size / install date |
| `r` | Refresh package data from `brew` without restarting |
| `Enter` | Open the action menu for the selected package (incl. uninstall) |
| `u` | Update selected package(s) via `brew upgrade` (asks first) |
| `U` | Update everything outdated at once (pinned packages are skipped) |
| `p` | Pin/unpin a formula (`brew pin`) — pinned packages are left out of updates |
| `n` | Add/edit a note |
| `c` | Pick a category from a filterable list (applies to all multi-selected) |
| `R` | Clear a manual category override |
| `o` | Open the package's homepage |
| `y` | Copy the package name to the clipboard |
| `Space` | Toggle multi-select |
| `d` | Toggle dependency-only packages |
| `?` | Full keybinding overlay |
| `Esc` | Clear selection, cancel input, or quit |
| `q` | Quit |

The sidebar has three pseudo-categories pinned above the taxonomy — **Outdated**, **Unmaintained**, and **Orphaned** — that filter to exactly those packages. They're views, not real categories: selecting one doesn't change any package's classification. **Orphaned** shows dependency-only packages that nothing installed still needs (what `brew autoremove` would remove); it's the one view that ignores the dependency toggle, since orphans are by definition dependency-only. The analysis walks the full runtime-dependency edge set from Homebrew's install receipts — including transitive entries whose direct declarer is long uninstalled — and was checked against `brew autoremove --dry-run` on a real install.

Search (`/`) deliberately ignores the sidebar: you reach for it precisely when you *don't* know which category a package sits in, so scoping the search to one would hide the answer. Instead every category is searched and the hits come back grouped under category headings — biggest group first, so the category that best answers the search is at the top. The list narrows with every keystroke, the matched text is highlighted in each row, and Esc restores whatever was there before you started typing:

```
 Packages  ⌕ file  17 matches in 8 categories
   Archives & Compression · 4
     cabextract     1.11      Extract files from Microsoft cabinet files
     keka           1.6.7     File archiver
     …
   Documents & PDF · 3
     archivewebpage 0.16.2    Archive webpages manually to WARC or WACZ files
     …
```

Headings are skipped by `j`/`k` and clicking one drops the cursor on the first package below it. Clearing the search puts you back in whatever category the sidebar was on.

The detail pane shows each package's dependencies as a tree and, next to it, **Required by** — the reverse edge, which for anything that arrived as a dependency is the whole answer to "why is this installed?".

Updating (`u`/`U`) and uninstalling (via the action menu) are the actions that change your Homebrew install, so both always confirm first — uninstall in unmissable red — then hand the terminal over to `brew` so its own progress output renders normally. Press Enter when it's done to return; the list refreshes automatically.

Multi-select (`Space`) drives bulk operations: tick off a run of packages, then `c` to categorize them all at once, `R` to reset their overrides, `u` to update every outdated one together, or uninstall the lot from the action menu.

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

Colour is spent where it earns its keep, rather than everywhere at once:

- **Status is what colour is for.** Update available, no longer maintained, manually classified, multi-selected — these are the things that light up. Package names and category labels stay neutral so the markers actually register.
- **Each category** has its own glyph and colour, but in the list only the glyph is tinted. A whole column in nineteen competing hues reads as noise; one character is enough to tie a row to its sidebar entry.
- **Formula vs. cask** is a terminal vs. monitor glyph. Just the one meaning — it used to be tinted by classification source as well, and a character carrying two unrelated meanings read as neither.
- **On-disk size** is traffic-lighted green → yellow → orange → red.
- **Alternating row backgrounds**, one step off the terminal's own, guide the eye across a wide row. The selected row uses a lighter tone and is painted afterwards, so the two never compete.
- The header carries live counts plus a bar showing what share of your install is current.

The sidebar hides categories with nothing in them (All, Outdated and Unmaintained always stay, since "0" is a useful answer there), and the multi-select gutter only appears once something is selected — so the default view spends its width on packages.

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

**On-disk sizes are cached too.** Walking a package's install directory is the expensive part of size-sorting and `categories --sizes` (seconds across a whole install), but a size only changes when the installed version does — so every computed size is persisted keyed by `name|version`, which makes each entry self-invalidating on upgrade. First `categories --sizes` run: ~4.7s; every one after: ~0.05s. The TUI seeds its size lookups from the same store and writes anything newly computed back on exit.

The TUI can also re-read everything mid-session with `r`, which bypasses the cache the same way `--refresh` does.

## How classification works

Precedence, highest wins:

1. **Manual override** — `lagerregal category <name> <category>`, or `c` in the TUI
2. **Curated list** (`src/data/categories.toml`) — exact package-name matches
3. **Keyword heuristic** — matched against the package's name *and* description, so the thousands of `font-*` casks that carry no description still classify from their name
4. **Uncategorized** — fallback

Two rules keep the keyword matching from firing on coincidences:

- **Short keywords must start at a word boundary.** Anything four characters or fewer collides by accident otherwise — `ssl` inside "lossless" filed compression tools under Cryptography, `dns` inside "cjdns" filed a mesh router under DNS. Growth to the right is still allowed, so `emulat` keeps matching "emulator", and longer keywords may still match mid-word, which several rely on (`compiler` inside "decompiler").
- **Heuristics can veto themselves** with an `exclude` list, for stems that mean something else in a specific context. `emulat` is right for game emulators and wrong for a *terminal* emulator; `video` is right for media tools and wrong for a *videogame*.

Both were derived by measuring against the full catalogue rather than guessing. A tempting alternative — letting the longest matching keyword win — was tried and rejected on the numbers: on a real 184-package install it improved three classifications and broke five, because length isn't specificity ("terminal" is eight characters and a weak signal).

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

Homebrew JSON parsing, classification, the cache fingerprint, and the theme's category table all run against fixtures or temp directories rather than a real `brew`, so the test suite runs anywhere Rust does. The TUI is covered two ways: its selection/search/grouping logic by plain unit tests, and whole rendered frames via ratatui's `TestBackend` — full screens drawn into an in-memory buffer and asserted on, so layout regressions fail in `cargo test` instead of waiting to be noticed in a terminal. Exercising the actual `brew` shell-outs still needs a Mac with Homebrew installed.

The TUI lives in `src/tui/` split by responsibility — `app.rs` (state + row model), `input.rs` (keyboard/mouse), `draw.rs` (rendering) — with the event loop in `mod.rs`. Built on ratatui 0.30 / crossterm 0.29.

CI (`.github/workflows/ci.yml`) runs the same four commands on every push and pull request, once per shipped target and always on a runner of that target's own architecture, so no platform is just assumed to work: `aarch64-apple-darwin` (`macos-latest`), `x86_64-apple-darwin` (`macos-15-intel` — GitHub's last Intel macOS image, retiring alongside macOS 15 support around fall 2027), `aarch64-unknown-linux-musl` (`ubuntu-24.04-arm`) and `x86_64-unknown-linux-musl` (`ubuntu-latest`).

## Releasing

Releases are cut by pushing a tag:

```sh
# bump version in Cargo.toml first, then:
git tag v0.1.0
git push origin v0.1.0
```

`.github/workflows/release.yml` then:

1. checks that the tag matches the version in `Cargo.toml`, failing loudly if it doesn't;
2. builds all four target binaries — two macOS, two Linux — each on a runner of its own architecture, and packages each as a tarball;
3. publishes a GitHub Release with all four tarballs attached;
4. regenerates `Formula/lagerregal.rb` with the per-platform URLs and their SHA-256 sums, and pushes it to the `main` branch of [`admonstrator/homebrew-tap`](https://github.com/admonstrator/homebrew-tap).

Checksums are taken in the release job rather than per build, so they all come from one `sha256sum` on one runner — `shasum` and `sha256sum` differ across the macOS and Linux images, and a formula is only as good as its hashes.

Since the formula lives in a different repo, the built-in `GITHUB_TOKEN` (scoped to this repo only) isn't enough to push it. The workflow uses `GH_TAP_PAT`, a fine-grained Personal Access Token scoped to `contents: write` on `homebrew-tap` only, stored as a secret on this repo. The formula itself is generated, not hand-maintained; change the workflow rather than editing it in `homebrew-tap` directly.

## License

MIT
