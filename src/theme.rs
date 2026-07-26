use std::sync::OnceLock;

use ratatui::style::Color;

use crate::classify::ClassificationSource;
use crate::homebrew::PackageKind;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------
//
// Catppuccin Mocha (https://catppuccin.com) - a coordinated dark palette
// rather than raw ANSI colors, so hues stay balanced against each other and
// don't shift with whatever the user's terminal has mapped color 1..15 to.
// Every color below is a fixed RGB value for exactly that reason.

const RED: Color = Color::Rgb(243, 139, 168);
const MAROON: Color = Color::Rgb(235, 160, 172);
const PEACH: Color = Color::Rgb(250, 179, 135);
const YELLOW: Color = Color::Rgb(249, 226, 175);
const GREEN: Color = Color::Rgb(166, 227, 161);
const TEAL: Color = Color::Rgb(148, 226, 213);
const SKY: Color = Color::Rgb(137, 220, 235);
const SAPPHIRE: Color = Color::Rgb(116, 199, 236);
const BLUE: Color = Color::Rgb(137, 180, 250);
const LAVENDER: Color = Color::Rgb(180, 190, 254);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const PINK: Color = Color::Rgb(245, 194, 231);
const FLAMINGO: Color = Color::Rgb(242, 205, 205);
const ROSEWATER: Color = Color::Rgb(245, 224, 220);
const SUBTEXT: Color = Color::Rgb(166, 173, 200);
const OVERLAY: Color = Color::Rgb(108, 112, 134);
const SURFACE: Color = Color::Rgb(49, 50, 68);
const MANTLE: Color = Color::Rgb(24, 24, 37);
const CRUST: Color = Color::Rgb(17, 17, 27);

/// Accent color for focused-pane borders, the header brand, and highlights.
pub const ACCENT: Color = MAUVE;
/// Secondary accent, for less prominent structural bits (scrollbar, rules).
pub const ACCENT_DIM: Color = OVERLAY;
/// Muted color for labels ("Size:", "Publisher:") so values stand out.
pub const LABEL: Color = SUBTEXT;
/// Barely-there tone for inert chrome like the scrollbar track.
pub const SURFACE_DIM: Color = SURFACE;

/// Colors for a package's classification source, used on the manual-override
/// marker and the "[source]" label so its provenance reads at a glance.
pub const MANUAL: Color = YELLOW;
pub const CURATED: Color = GREEN;
pub const HEURISTIC: Color = SAPPHIRE;
pub const UNCATEGORIZED: Color = OVERLAY;

/// Warm, attention-grabbing color for "an update is available" - distinct
/// from red (which reads as an error/failure, not a heads-up).
pub const OUTDATED: Color = PEACH;

/// Orphaned packages (autoremove candidates) - informational rather than
/// alarming: they cost disk space, but nothing is broken.
pub const ORPHANED: Color = SKY;

/// The matched substring inside live search results. Yellow like `MANUAL`,
/// but always paired with bold and only ever mid-text, so the two never
/// appear in a context where they could be mistaken for each other.
pub const MATCH: Color = YELLOW;

/// Alarm color for packages Homebrew itself has marked deprecated/disabled -
/// deliberately red (unlike `OUTDATED`'s warm orange), since this is closer
/// to "this may stop working" than "a newer version exists".
pub const DANGER: Color = RED;

/// Background tint for the selected row/item - a raised surface tone instead
/// of a hard `REVERSED` flip, so per-cell colors (category, source, ...)
/// stay legible on the highlighted row instead of being inverted into noise.
pub const HIGHLIGHT_BG: Color = SURFACE;

/// Background tint for the slim header/footer bars.
pub const HEADER_BG: Color = MANTLE;

/// Background for every other row in the package list. Deliberately a very
/// small step away from the terminal's own background - enough to guide the
/// eye along a wide row, not enough to read as "these rows are special".
/// Selection uses `HIGHLIGHT_BG`, which is lighter, so the two never
/// compete: stripes recede, the selected row comes forward.
pub const ROW_ALT_BG: Color = MANTLE;

/// Background for the category headings that break up grouped search
/// results. Deliberately the darkest tone in the palette: the two other row
/// backgrounds already sit above it (stripes at `ROW_ALT_BG`, the selected
/// row at the lighter `HIGHLIGHT_BG`), so a heading reads as a groove
/// between groups and can't be confused with either.
pub const GROUP_BG: Color = CRUST;

pub fn source_color(source: ClassificationSource) -> Color {
    match source {
        ClassificationSource::Manual => MANUAL,
        ClassificationSource::Curated => CURATED,
        ClassificationSource::Heuristic => HEURISTIC,
        ClassificationSource::Uncategorized => UNCATEGORIZED,
    }
}

/// Traffic-light coloring for on-disk size, so a glance at the detail pane
/// tells you whether a package is a rounding error or a serious tenant.
pub fn size_color(bytes: u64) -> Color {
    const MB: u64 = 1024 * 1024;
    match bytes {
        b if b >= 2048 * MB => RED,
        b if b >= 512 * MB => PEACH,
        b if b >= 64 * MB => YELLOW,
        _ => GREEN,
    }
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------
//
// All glyphs below are Font Awesome 4 codepoints (U+F000..U+F2FF), the range
// that has been present in every Nerd Font release - so any patched font
// renders them, rather than only the newest ones. Users without a Nerd Font
// get the plain-Unicode fallbacks via `--no-icons` / `LAGERREGAL_NO_ICONS`.

static ICONS_ENABLED: OnceLock<bool> = OnceLock::new();

/// Enables or disables Nerd Font glyphs for this process. Called once at
/// startup; later calls are ignored (`OnceLock` semantics), so the setting
/// can't change mid-render and leave a half-iconified frame.
pub fn init_icons(enabled: bool) {
    let _ = ICONS_ENABLED.set(enabled);
}

fn icons_enabled() -> bool {
    *ICONS_ENABLED.get().unwrap_or(&true)
}

/// Picks between a Nerd Font glyph and a plain-Unicode fallback.
fn icon(nerd: &'static str, fallback: &'static str) -> &'static str {
    if icons_enabled() {
        nerd
    } else {
        fallback
    }
}

pub fn brand_icon() -> &'static str {
    icon("\u{f0fc}", "\u{25c6}") //  / ◆
}

/// Icon for the "All" sidebar entry. Deliberately not part of
/// `CATEGORY_STYLES` - that table is checked against the real taxonomy, and
/// "All" is a pseudo-category that never appears as a package's category.
pub fn all_icon() -> &'static str {
    icon("\u{f00b}", "\u{25cf}") //  / ●
}

pub fn outdated_icon() -> &'static str {
    icon("\u{f0aa}", "\u{2191}") //  / ↑
}

pub fn unmaintained_icon() -> &'static str {
    icon("\u{f071}", "\u{26a0}") //  / ⚠
}

pub fn manual_icon() -> &'static str {
    icon("\u{f005}", "*") //  / *
}

pub fn checked_icon() -> &'static str {
    icon("\u{f046}", "\u{2713}") //  / ✓
}

pub fn info_icon() -> &'static str {
    icon("\u{f05a}", "\u{2139}") //  / ℹ
}

pub fn filter_icon() -> &'static str {
    icon("\u{f002}", "/") //  / /
}

pub fn sort_icon() -> &'static str {
    icon("\u{f0dc}", "\u{2195}") //  / ↕
}

pub fn deps_icon() -> &'static str {
    icon("\u{f0e8}", "\u{2325}") //  / ⌥
}

pub fn pin_icon() -> &'static str {
    icon("\u{f08d}", "\u{29bf}") //  / ⦿
}

pub fn orphan_icon() -> &'static str {
    icon("\u{f1b8}", "\u{267b}") //  / ♻
}

pub fn note_icon() -> &'static str {
    icon("\u{f040}", "\u{270e}") //  / ✎
}

pub fn version_icon() -> &'static str {
    icon("\u{f02b}", "\u{2022}") //  / •
}

pub fn size_icon() -> &'static str {
    icon("\u{f0a0}", "\u{2022}") //  / •
}

pub fn time_icon() -> &'static str {
    icon("\u{f017}", "\u{2022}") //  / •
}

pub fn link_icon() -> &'static str {
    icon("\u{f0c1}", "\u{2022}") //  / •
}

pub fn publisher_icon() -> &'static str {
    icon("\u{f1b3}", "\u{2022}") //  / •
}

/// Formulae are command-line tools, casks are GUI apps - worth telling apart
/// at a glance in a list that mixes both.
pub fn kind_icon(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Formula => icon("\u{f120}", "$"), //  / $
        PackageKind::Cask => icon("\u{f108}", "\u{25a3}"), //  / ▣
    }
}

/// Per-category glyph and color. Hand-assigned for the built-in taxonomy
/// (so each one is *about* something recognizable rather than arbitrary),
/// with a hashed fallback for user-invented category names.
///
/// The two entries are kept in one table so a category's icon and color are
/// impossible to get out of sync.
const CATEGORY_STYLES: &[(&str, &str, Color)] = &[
    ("Security", "\u{f132}", RED),                      // shield
    ("AI & Machine Learning", "\u{f0e7}", PINK),        // bolt
    ("DNS", "\u{f0ac}", SKY),                           // globe
    ("Cryptography", "\u{f023}", MAUVE),                // lock
    ("Cryptocurrency", "\u{f15a}", YELLOW),             // bitcoin
    ("Networking", "\u{f0e8}", BLUE),                   // sitemap
    ("Media & Graphics", "\u{f03e}", PEACH),            // image
    ("Fonts", "\u{f031}", ROSEWATER),                   // font
    ("Documents & PDF", "\u{f0f6}", LAVENDER),          // file-text
    ("Monitoring", "\u{f0e4}", TEAL),                   // dashboard
    ("Databases", "\u{f1c0}", SAPPHIRE),                // database
    ("Cloud & Infra", "\u{f0c2}", MAROON),              // cloud
    ("Dev Tools & Languages", "\u{f121}", GREEN),       // code
    ("System Utilities", "\u{f0ad}", SUBTEXT),          // wrench
    ("Communication & Browsers", "\u{f086}", FLAMINGO), // comments
    ("Games & Emulation", "\u{f11b}", MAUVE),           // gamepad
    ("Archives & Compression", "\u{f187}", PEACH),      // archive
    ("Peripherals & Input", "\u{f11c}", TEAL),          // keyboard
    ("Productivity", "\u{f046}", GREEN),                // check-square
    ("Uncategorized", "\u{f128}", OVERLAY),             // question mark
];

/// A small, curated, terminal-friendly palette for color-coding category
/// names that aren't part of the built-in taxonomy.
const FALLBACK_PALETTE: [Color; 8] = [TEAL, MAUVE, YELLOW, GREEN, BLUE, PEACH, PINK, SAPPHIRE];

fn category_entry(category: &str) -> Option<&'static (&'static str, &'static str, Color)> {
    CATEGORY_STYLES
        .iter()
        .find(|(name, _, _)| *name == category)
}

/// Deterministically maps a category name to a color: hand-picked for the
/// built-in taxonomy, otherwise hashed over the name text (not an index, so
/// it's stable across runs and independent of category list ordering).
pub fn category_color(category: &str) -> Color {
    if let Some((_, _, color)) = category_entry(category) {
        return *color;
    }
    let hash = category
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    FALLBACK_PALETTE[(hash as usize) % FALLBACK_PALETTE.len()]
}

/// The glyph for a category. Custom (user-invented) categories share one
/// generic "tag" glyph, since there's nothing to base a specific one on.
pub fn category_icon(category: &str) -> &'static str {
    if !icons_enabled() {
        return "\u{25cf}"; // ●
    }
    match category_entry(category) {
        Some((_, glyph, _)) => glyph,
        None => "\u{f02b}", // tag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in taxonomy, as `classify::known_categories()` reports it.
    /// Kept as a literal here so a drift between the two shows up as a test
    /// failure rather than as silently un-iconified categories in the UI.
    const BUILT_IN: [&str; 20] = [
        "Security",
        "AI & Machine Learning",
        "DNS",
        "Cryptography",
        "Cryptocurrency",
        "Networking",
        "Media & Graphics",
        "Fonts",
        "Documents & PDF",
        "Monitoring",
        "Databases",
        "Cloud & Infra",
        "Dev Tools & Languages",
        "System Utilities",
        "Communication & Browsers",
        "Games & Emulation",
        "Archives & Compression",
        "Peripherals & Input",
        "Productivity",
        "Uncategorized",
    ];

    #[test]
    fn every_built_in_category_has_a_hand_picked_style() {
        for name in BUILT_IN {
            assert!(
                category_entry(name).is_some(),
                "category \"{name}\" has no entry in CATEGORY_STYLES"
            );
        }
    }

    #[test]
    fn category_styles_matches_the_real_taxonomy() {
        // Guards against adding a category to categories.toml (or renaming
        // one) without giving it an icon/color here.
        let known = crate::classify::known_categories();
        for name in &known {
            assert!(
                category_entry(name).is_some(),
                "category \"{name}\" from categories.toml has no style entry"
            );
        }
        for (name, _, _) in CATEGORY_STYLES {
            assert!(
                known.iter().any(|k| k == name),
                "CATEGORY_STYLES has stale entry \"{name}\" that no longer exists"
            );
        }
    }

    #[test]
    fn category_color_is_deterministic_for_custom_names() {
        assert_eq!(category_color("My Stuff"), category_color("My Stuff"));
    }

    #[test]
    fn custom_categories_get_a_generic_icon_and_a_palette_color() {
        // Not in the built-in taxonomy, so it falls through to the hash.
        assert_eq!(category_icon("Totally Custom"), "\u{f02b}");
        assert!(FALLBACK_PALETTE.contains(&category_color("Totally Custom")));
    }

    #[test]
    fn size_color_escalates_with_magnitude() {
        const MB: u64 = 1024 * 1024;
        assert_eq!(size_color(MB), GREEN);
        assert_eq!(size_color(100 * MB), YELLOW);
        assert_eq!(size_color(800 * MB), PEACH);
        assert_eq!(size_color(4096 * MB), RED);
    }
}
