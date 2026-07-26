use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "lagerregal",
    version,
    about = "Classify and browse your installed Homebrew packages by category"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Ignore the local cache and re-read everything from `brew`. The cache
    /// is invalidated automatically when packages change, so this is only
    /// needed to force a refresh by hand.
    #[arg(long, global = true)]
    pub refresh: bool,
    /// Render the TUI without Nerd Font glyphs (use plain Unicode instead).
    /// Also settable via the LAGERREGAL_NO_ICONS environment variable.
    #[arg(long, global = true, env = "LAGERREGAL_NO_ICONS")]
    pub no_icons: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Re-scan installed Homebrew packages and print a category summary
    Scan {
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
    },
    /// List installed packages, optionally filtered by category
    #[command(alias = "ls")]
    List {
        /// Only show packages in this category (see `lagerregal categories`)
        #[arg(short, long)]
        category: Option<String>,
        /// Output as JSON instead of a table
        #[arg(long)]
        json: bool,
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
    },
    /// Show details for a single installed package
    Show {
        /// Homebrew formula or cask name
        name: String,
    },
    /// Set or update a personal note for a package
    Note {
        /// Homebrew formula or cask name
        name: String,
        /// Free-text note, e.g. "for debugging DNS on the home lab"
        text: String,
    },
    /// Manually set (override) a package's category, or clear a previous override with --reset
    #[command(alias = "cat")]
    Category {
        /// Homebrew formula or cask name
        name: String,
        /// Category name, e.g. "Security" or "DNS" - see `lagerregal categories`
        category: Option<String>,
        /// Clear a manual override, falling back to the curated/heuristic classification
        #[arg(short, long, conflicts_with = "category")]
        reset: bool,
    },
    /// List all known categories and how many installed packages are in each
    Categories {
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
        /// Also compute and show total on-disk size per category (slower - walks the filesystem)
        #[arg(short, long)]
        sizes: bool,
    },
    /// List installed packages that have an update available (runs `brew outdated`)
    Outdated {
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
        /// Output as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// List installed packages Homebrew has marked deprecated or disabled (no longer maintained)
    Unmaintained {
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
        /// Output as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// List dependency-only packages nothing depends on anymore (what `brew autoremove` would remove)
    Orphans {
        /// Output as JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Upgrade a single installed package via `brew upgrade`
    Update {
        /// Homebrew formula or cask name
        name: String,
    },
    /// Save or compare snapshots of your installed packages and their versions
    Snapshot {
        #[command(subcommand)]
        action: SnapshotCommand,
    },
    /// Launch the interactive TUI dashboard (also the default with no subcommand)
    Tui,
}

#[derive(Subcommand)]
pub enum SnapshotCommand {
    /// Save the current installed packages/versions as a named snapshot (default name: "default")
    Save {
        /// Snapshot name, e.g. "before-reinstall"
        name: Option<String>,
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
    },
    /// Compare the current installed packages against a saved snapshot
    Diff {
        /// Snapshot name to compare against (default: "default")
        name: Option<String>,
        /// Also include packages only pulled in as a dependency of something else
        #[arg(short, long)]
        all: bool,
    },
}
