use super::app::*;
use super::draw::*;
use super::input::*;

use crate::classify::{ClassificationSource, ClassifiedPackage};
use crate::homebrew::{Package, PackageKind};

#[test]
fn row_to_index_accounts_for_the_header_offset_and_scroll_position() {
    // rows_top=5 (border + header), no scrolling: row 5 is item 0.
    assert_eq!(row_to_index(5, 5, 0), Some(0));
    assert_eq!(row_to_index(7, 5, 0), Some(2));
    // Scrolled down 3 rows: the row at the top of the viewport is item 3.
    assert_eq!(row_to_index(5, 5, 3), Some(3));
}

#[test]
fn row_to_index_is_none_above_the_first_row() {
    // A click on the border/header itself (row < rows_top) hits nothing.
    assert_eq!(row_to_index(2, 5, 0), None);
}

fn pkg(name: &str, category: &str) -> ClassifiedPackage {
    ClassifiedPackage {
        package: Package {
            name: name.to_string(),
            kind: PackageKind::Formula,
            desc: String::new(),
            homepage: String::new(),
            tap: String::new(),
            version: "1.0".to_string(),
            installed_on_request: true,
            installed_at: None,
            dependencies: Vec::new(),
            indirect_dependencies: Vec::new(),
            outdated: None,
            unmaintained: false,
            unmaintained_reason: None,
            pinned: false,
        },
        category: category.to_string(),
        source: ClassificationSource::Curated,
        note: None,
    }
}

/// Sidebar on some category, plus a search term - the setup where the
/// old behaviour would have hidden everything outside that category.
fn searching_app(filter: &str) -> App {
    let mut app = App::new(vec![
        pkg("1password", "Security"),
        pkg("1password-cli", "Security"),
        pkg("find-the-password", "Games & Emulation"),
        pkg("passwordler", "Productivity"),
        pkg("ripgrep", "Dev Tools & Languages"),
    ]);
    app.filter = filter.to_string();
    app
}

#[test]
fn a_search_reaches_past_the_selected_category() {
    let mut app = searching_app("password");
    // Park the sidebar on a category that holds only one of the hits.
    let productivity = app
        .sidebar_categories()
        .iter()
        .position(|c| c == "Productivity")
        .expect("Productivity is populated");
    app.sidebar_state.select(Some(productivity));

    let names: Vec<&str> = app
        .visible_packages()
        .iter()
        .map(|p| p.package.name.as_str())
        .collect();
    assert_eq!(names.len(), 4, "all four matches, not just Productivity's");
    assert!(names.contains(&"1password"));
    assert!(names.contains(&"find-the-password"));
    assert!(!names.contains(&"ripgrep"), "non-matches stay out");
}

#[test]
fn search_results_are_grouped_by_category_biggest_group_first() {
    let app = searching_app("password");
    let rows = app.visible_rows();

    let layout: Vec<String> = rows
        .iter()
        .map(|r| match r {
            ListRow::Header { category, count } => format!("# {category} ({count})"),
            ListRow::Package(p) => p.package.name.clone(),
            ListRow::Empty => "<empty>".to_string(),
        })
        .collect();

    assert_eq!(
        layout,
        vec![
            "# Security (2)",
            "1password",
            "1password-cli",
            "# Games & Emulation (1)",
            "find-the-password",
            "# Productivity (1)",
            "passwordler",
        ]
    );
}

#[test]
fn without_a_search_the_rows_stay_flat_and_scoped() {
    let mut app = App::new(vec![pkg("1password", "Security"), pkg("ripgrep", "Dev")]);
    app.filter.clear();
    assert!(!app.grouped());
    assert!(
        app.visible_rows()
            .iter()
            .all(|r| matches!(r, ListRow::Package(_))),
        "no headings without a search"
    );
}

#[test]
fn navigation_steps_over_group_headings() {
    let mut app = searching_app("password");
    app.focus = Focus::List;
    app.reset_list_selection();

    // Row 0 is a heading, so the cursor starts on row 1.
    assert_eq!(app.list_state.selected(), Some(1));
    assert_eq!(app.selected_package().unwrap().package.name, "1password");

    // 1password-cli, then across the "Games & Emulation" heading.
    move_selection(&mut app, 1);
    assert_eq!(
        app.selected_package().unwrap().package.name,
        "1password-cli"
    );
    move_selection(&mut app, 1);
    assert_eq!(
        app.selected_package().unwrap().package.name,
        "find-the-password"
    );

    // Wrapping past the last package lands back on the first one, not
    // on the heading that physically precedes it.
    move_selection(&mut app, 1);
    move_selection(&mut app, 1);
    assert_eq!(app.selected_package().unwrap().package.name, "1password");

    // And backwards over a heading too.
    move_selection(&mut app, -1);
    assert_eq!(app.selected_package().unwrap().package.name, "passwordler");
}

#[test]
fn a_cursor_left_on_a_heading_snaps_to_the_next_package() {
    let mut app = searching_app("password");
    // Row 3 is the "Games & Emulation" heading.
    app.list_state.select(Some(3));
    app.clamp_list_selection();
    assert_eq!(app.list_state.selected(), Some(4));
    assert_eq!(
        app.selected_package().unwrap().package.name,
        "find-the-password"
    );
}

#[test]
fn a_search_with_no_hits_yields_one_unselectable_empty_row() {
    let mut app = searching_app("nothing-matches-this");
    assert!(matches!(app.visible_rows().as_slice(), [ListRow::Empty]));
    app.clamp_list_selection();
    assert_eq!(app.list_state.selected(), None);
    assert!(app.selected_package().is_none());
}

#[test]
fn find_ci_is_case_insensitive_and_boundary_safe() {
    assert_eq!(find_ci("Ripgrep", "rip", 0), Some((0, 3)));
    assert_eq!(find_ci("abcdef", "CDE", 0), Some((2, 5)));
    assert_eq!(find_ci("abc", "x", 0), None);
    assert_eq!(find_ci("abc", "", 0), None);
    // Umlauts fold within the same byte length...
    assert_eq!(find_ci("K\u{d6}LN tools", "k\u{f6}ln", 0), Some((0, 5)));
    // ...and a `from` offset skips earlier hits.
    assert_eq!(find_ci("aa", "a", 1), Some((1, 2)));
}

#[test]
fn highlight_matches_marks_every_occurrence_and_keeps_the_rest() {
    let spans = highlight_matches("a match, a match", "match", Default::default());
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "a match, a match", "no characters lost or duplicated");
    let highlighted = spans
        .iter()
        .filter(|s| s.style.fg == Some(crate::theme::MATCH))
        .count();
    assert_eq!(highlighted, 2);
}

#[test]
fn highlight_matches_with_empty_needle_is_one_plain_span() {
    let spans = highlight_matches("plain", "", Default::default());
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content.as_ref(), "plain");
}

// ---------------------------------------------------------------------------
// Frame snapshots: render full frames into ratatui's TestBackend and assert
// on their text content, so layout regressions fail in `cargo test` instead
// of waiting for someone to notice them in a terminal. Assertions stick to
// text (not styling), which is what survives refactors worth catching.
// ---------------------------------------------------------------------------

/// Renders one frame at the given size and flattens the buffer to a string
/// (rows separated by newlines) for `contains` assertions.
fn render_frame(app: &mut App, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn frame_normal_mode_shows_sidebar_list_and_category_column() {
    let mut app = searching_app("");
    let frame = render_frame(&mut app, 130, 34);
    assert!(frame.contains("Categories"), "sidebar pane is titled");
    assert!(frame.contains(" Packages "), "list pane is titled");
    assert!(
        frame.contains("Category"),
        "All view keeps the category column"
    );
    assert!(frame.contains("1password"));
    assert!(frame.contains("ripgrep"));
    assert!(
        frame.contains("Orphaned"),
        "orphan pseudo-category is listed"
    );
}

#[test]
fn frame_grouped_search_shows_headings_and_match_summary() {
    let mut app = searching_app("password");
    app.reset_list_selection();
    let frame = render_frame(&mut app, 130, 34);
    assert!(frame.contains("4 matches in 3 categories"));
    assert!(frame.contains("Security"), "group heading present");
    assert!(
        frame.contains("Productivity"),
        "search reaches other categories"
    );
    assert!(
        !frame.contains("ripgrep"),
        "non-matching packages stay out of the frame"
    );
}

#[test]
fn frame_search_without_hits_states_it_plainly() {
    let mut app = searching_app("zzz-no-such-package");
    app.clamp_list_selection();
    let frame = render_frame(&mut app, 130, 34);
    assert!(frame.contains("No package matches \"zzz-no-such-package\""));
}

#[test]
fn frame_category_picker_lists_candidates_and_the_new_row() {
    let mut app = searching_app("");
    start_category_edit(&mut app);
    app.input_buffer = "Sec".to_string();
    let frame = render_frame(&mut app, 130, 34);
    assert!(frame.contains("Set category"), "picker popup is titled");
    assert!(frame.contains("Security"), "matching candidate listed");
    assert!(
        !frame.contains(" DNS"),
        "non-matching candidates are filtered out"
    );
    assert!(
        frame.contains("New category"),
        "free-text escape hatch listed"
    );
}

#[test]
fn frame_uninstall_confirm_is_named_and_warns_about_disk() {
    let mut app = searching_app("");
    start_uninstall(&mut app);
    let frame = render_frame(&mut app, 130, 34);
    assert!(frame.contains("Confirm Uninstall"));
    assert!(frame.contains("brew uninstall"));
    assert!(frame.contains("1password"), "the target package is listed");
}

#[test]
fn frame_orphaned_view_shows_the_empty_state() {
    let mut app = searching_app("");
    let orphaned = app
        .sidebar_categories()
        .iter()
        .position(|c| c == "Orphaned")
        .expect("Orphaned is always listed");
    app.sidebar_state.select(Some(orphaned));
    app.clamp_list_selection();
    let frame = render_frame(&mut app, 130, 34);
    assert!(
        frame.contains("Nothing here."),
        "an empty pseudo-category states its emptiness"
    );
}
