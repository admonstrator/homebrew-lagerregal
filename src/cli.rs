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
}

#[derive(Subcommand)]
pub enum Command {
    /// Re-scan installed Homebrew packages and print a category summary
    Scan,
    /// List installed packages, optionally filtered by category
    #[command(alias = "ls")]
    List {
        /// Only show packages in this category (see `lagerregal categories`)
        #[arg(short, long)]
        category: Option<String>,
        /// Output as JSON instead of a table
        #[arg(long)]
        json: bool,
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
    /// Manually set (override) a package's category
    #[command(alias = "cat")]
    Category {
        /// Homebrew formula or cask name
        name: String,
        /// Category name, e.g. "Security" or "DNS" - see `lagerregal categories`
        category: String,
    },
    /// List all known categories and how many installed packages are in each
    Categories,
    /// Launch the interactive TUI dashboard (also the default with no subcommand)
    Tui,
}
