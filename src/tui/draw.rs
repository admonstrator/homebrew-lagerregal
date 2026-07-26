//! All rendering: the frame layout, each pane, the overlays, and the row
//! builders for the package table.

use std::collections::BTreeMap;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, LineGauge, List, ListItem, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};
use ratatui::Frame;

use crate::classify::{ClassificationSource, ClassifiedPackage};
use crate::details;
use crate::homebrew::Package;
use crate::theme;

use super::app::*;
use super::input::*;

pub(super) fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(8),
            Constraint::Length(3),
        ])
        .split(area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(20), Constraint::Percentage(80)])
        .split(rows[1]);

    draw_header(f, app, rows[0]);
    draw_sidebar(f, app, columns[0]);
    draw_list(f, app, columns[1]);
    draw_detail(f, app, rows[2]);
    draw_footer(f, app, rows[3]);

    match app.mode {
        InputMode::Help => draw_help_overlay(f),
        InputMode::Menu => draw_menu_overlay(f, app),
        InputMode::Confirm => draw_confirm_overlay(f, app),
        InputMode::CategoryPick => draw_picker_overlay(f, app),
        _ => {}
    }
}

/// A slim, borderless title bar - "lagerregal", live package/update counts,
/// and the dependency-inclusion state - rendered above the main panes.
pub(super) fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let scoped = app.scoped_packages();
    let total = scoped.len();
    let outdated = scoped
        .iter()
        .filter(|p| p.package.outdated.is_some())
        .count();
    let unmaintained = scoped.iter().filter(|p| p.package.unmaintained).count();

    let sep = || Span::styled(" \u{2502} ", Style::default().fg(theme::ACCENT_DIM));

    let mut spans = vec![
        Span::styled(
            format!(" {} lagerregal", theme::brand_icon()),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        sep(),
        Span::styled(
            format!("{total}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" package{}", if total == 1 { "" } else { "s" }),
            Style::default().fg(theme::LABEL),
        ),
    ];
    if app.show_deps {
        spans.push(Span::styled(
            " +deps",
            Style::default().fg(theme::ACCENT_DIM),
        ));
    }
    if outdated > 0 {
        spans.push(sep());
        spans.push(Span::styled(
            format!("{} {outdated} outdated", theme::outdated_icon()),
            Style::default()
                .fg(theme::OUTDATED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if unmaintained > 0 {
        spans.push(sep());
        spans.push(Span::styled(
            format!("{} {unmaintained} unmaintained", theme::unmaintained_icon()),
            Style::default()
                .fg(theme::DANGER)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Paint the whole strip first so the background is continuous even where
    // the two halves don't reach - then lay the text and gauge on top.
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(theme::HEADER_BG)),
        area,
    );

    // A compact "how healthy is this install" bar, pinned right. Only worth
    // the space once there's something to be behind on.
    let gauge_width: u16 = 26;
    let (text_area, gauge_area) = if total > 0 && area.width > gauge_width + 40 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(gauge_width)])
            .split(area);
        (cols[0], Some(cols[1]))
    } else {
        (area, None)
    };

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::HEADER_BG)),
        text_area,
    );

    if let Some(gauge_area) = gauge_area {
        let up_to_date = total.saturating_sub(outdated);
        let ratio = up_to_date as f64 / total as f64;
        let color = if outdated == 0 {
            theme::CURATED
        } else {
            theme::OUTDATED
        };
        f.render_widget(
            LineGauge::default()
                .ratio(ratio)
                .filled_symbol(symbols::line::THICK.horizontal)
                .unfilled_symbol(symbols::line::THICK.horizontal)
                .filled_style(Style::default().fg(color).bg(theme::HEADER_BG))
                .unfilled_style(Style::default().fg(theme::ACCENT_DIM).bg(theme::HEADER_BG))
                .label(Span::styled(
                    format!("{:.0}% current ", ratio * 100.0),
                    Style::default().fg(theme::LABEL),
                ))
                .style(Style::default().bg(theme::HEADER_BG)),
            gauge_area,
        );
    }
}

pub(super) fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    app.hitboxes.sidebar = area;
    app.hitboxes.sidebar_rows_top = area.y + 1;
    let categories = app.sidebar_categories();
    let scoped = app.scoped_packages();
    let counts = category_counts(scoped.iter().copied());
    let total = scoped.len();
    let outdated_total = scoped
        .iter()
        .filter(|p| p.package.outdated.is_some())
        .count();
    let unmaintained_total = scoped.iter().filter(|p| p.package.unmaintained).count();
    let focused = app.focus == Focus::Sidebar;

    // Width available for the "icon name ........ count" line: the pane
    // minus its two border columns, the highlight symbol, and the icon.
    let inner_width = area.width.saturating_sub(2 + 2 + 2) as usize;

    let items: Vec<ListItem> = categories
        .iter()
        .map(|c| {
            let (count, color, icon) = match c.as_str() {
                ALL_CATEGORY => (total, theme::ACCENT, theme::all_icon()),
                OUTDATED_CATEGORY => (outdated_total, theme::OUTDATED, theme::outdated_icon()),
                UNMAINTAINED_CATEGORY => (
                    unmaintained_total,
                    theme::DANGER,
                    theme::unmaintained_icon(),
                ),
                ORPHANED_CATEGORY => (app.orphans.len(), theme::ORPHANED, theme::orphan_icon()),
                _ => (
                    *counts.get(c).unwrap_or(&0),
                    theme::category_color(c),
                    theme::category_icon(c),
                ),
            };
            // Right-align the counts into a column so the numbers line up
            // instead of ragging along behind names of varying length.
            let count_text = count.to_string();
            let gap = inner_width
                .saturating_sub(c.chars().count() + count_text.chars().count())
                .max(1);
            // An empty category is still worth listing (it's a valid filter
            // target) but shouldn't compete for attention with a full one.
            let name_style = if count == 0 {
                Style::default().fg(theme::ACCENT_DIM)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::styled(c.clone(), name_style),
                Span::raw(" ".repeat(gap)),
                Span::styled(count_text, Style::default().fg(theme::ACCENT_DIM)),
            ]))
        })
        .collect();

    let title = if app.show_deps {
        " Categories (incl. deps) "
    } else {
        " Categories "
    };
    let (border_type, border_style) = if focused {
        (
            BorderType::Thick,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (BorderType::Rounded, Style::default().fg(theme::ACCENT_DIM))
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(if focused { theme::ACCENT } else { theme::LABEL })
                        .add_modifier(Modifier::BOLD),
                ))
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style),
        )
        .highlight_symbol("\u{258c}")
        .highlight_style(
            Style::default()
                .bg(theme::HIGHLIGHT_BG)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, area, &mut app.sidebar_state);
}

pub(super) fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    app.hitboxes.list = area;
    // Rows start below the top border and the header row.
    app.hitboxes.list_rows_top = area.y + 2;
    let focused = app.focus == Focus::List;
    // The sidebar already scopes the list to one category once something
    // other than "All" is selected, so repeating it in every row would just
    // be noise - only show the column when it's actually adding information.
    // Grouped results carry their category in the heading above them, so the
    // column would be a third copy of the same word.
    let grouped = app.grouped();
    let show_category = !grouped && app.selected_category() == ALL_CATEGORY;

    // The multi-select gutter only exists once something is actually
    // selected - otherwise every row would carry two columns of indentation
    // for a mode most sessions never enter.
    let selecting = !app.selected_names.is_empty();

    // `rows` owns its content, so the immutable borrow of `app` ends with
    // this block - `render_stateful_widget` needs `app.list_state` mutably
    // further down.
    let (rows, row_count, is_empty, match_summary) = {
        let visible = app.visible_rows();
        let is_empty = matches!(visible.as_slice(), [ListRow::Empty]);
        let hits = visible
            .iter()
            .filter(|r| matches!(r, ListRow::Package(_)))
            .count();
        let groups = visible
            .iter()
            .filter(|r| matches!(r, ListRow::Header { .. }))
            .count();
        let match_summary = grouped.then(|| {
            format!(
                "{hits} match{} in {groups} categor{}",
                if hits == 1 { "" } else { "es" },
                if groups == 1 { "y" } else { "ies" }
            )
        });

        // Striping counts packages, not rendered rows, and restarts under
        // each heading - otherwise a group's first row would be shaded or
        // not depending on how many packages happened to precede it.
        let mut stripe = 0usize;
        let rows: Vec<Row> = visible
            .iter()
            .map(|row| match row {
                // Never actually rendered - `is_empty` below takes over the
                // whole pane, which gives the message the full width.
                ListRow::Empty => Row::default(),
                ListRow::Header { category, count } => {
                    stripe = 0;
                    draw_group_header_row(category, *count)
                }
                ListRow::Package(p) => {
                    let alt = stripe % 2 == 1;
                    stripe += 1;
                    draw_package_row(app, p, grouped, selecting, show_category, alt)
                }
            })
            .collect();
        (rows, visible.len(), is_empty, match_summary)
    };

    // Built as spans rather than one string so the active filter/sort/
    // selection badges can carry their own color - they're state, and state
    // that's on reads better when it's visually distinct from the label.
    let mut title_spans = vec![Span::styled(
        " Packages ",
        Style::default()
            .fg(if focused { theme::ACCENT } else { theme::LABEL })
            .add_modifier(Modifier::BOLD),
    )];
    if !app.filter.is_empty() {
        title_spans.push(Span::styled(
            format!("{} {} ", theme::filter_icon(), app.filter),
            Style::default().fg(theme::HEURISTIC),
        ));
        // Spelling out the spread is what tells the user the sidebar's
        // category was bypassed on purpose - otherwise the two panes
        // disagreeing just looks like a bug.
        if let Some(summary) = &match_summary {
            title_spans.push(Span::styled(
                format!("{summary} "),
                Style::default().fg(theme::LABEL),
            ));
        }
    }
    if app.sort_mode != SortMode::Name {
        title_spans.push(Span::styled(
            format!("{} {} ", theme::sort_icon(), app.sort_mode.label()),
            Style::default().fg(theme::CURATED),
        ));
    }
    if !app.selected_names.is_empty() {
        title_spans.push(Span::styled(
            format!(
                "{} {} selected ",
                theme::checked_icon(),
                app.selected_names.len()
            ),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let title = Line::from(title_spans);
    let (border_type, border_style) = if focused {
        (
            BorderType::Thick,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (BorderType::Rounded, Style::default().fg(theme::ACCENT_DIM))
    };
    let (widths, header): (Vec<Constraint>, Vec<&str>) = if show_category {
        (
            vec![
                Constraint::Percentage(28),
                Constraint::Percentage(20),
                Constraint::Percentage(12),
                Constraint::Percentage(40),
            ],
            vec!["Name", "Category", "Version", "Description"],
        )
    } else if grouped {
        // The first column also has to hold the category headings, which run
        // longer than any package name, so it gets a few points more here
        // than in the plain layout below.
        (
            vec![
                Constraint::Percentage(36),
                Constraint::Percentage(12),
                Constraint::Percentage(52),
            ],
            vec!["Name", "Version", "Description"],
        )
    } else {
        (
            vec![
                Constraint::Percentage(30),
                Constraint::Percentage(14),
                Constraint::Percentage(56),
            ],
            vec!["Name", "Version", "Description"],
        )
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(border_style);

    // An empty result gets the pane to itself, rather than a table whose one
    // row would be clipped to the width of the Name column.
    if is_empty {
        let inner = block.inner(area);
        f.render_widget(block, area);
        let message = if app.filter.is_empty() {
            "Nothing here.".to_string()
        } else {
            format!("No package matches \"{}\".", app.filter)
        };
        f.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme::ACCENT_DIM)),
            inner,
        );
        return;
    }

    let visible_count = row_count;
    let table = Table::new(rows, widths)
        .header(
            Row::new(header).style(
                Style::default()
                    .fg(theme::LABEL)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(block)
        .highlight_symbol("\u{258c}")
        .row_highlight_style(
            Style::default()
                .bg(theme::HIGHLIGHT_BG)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(table, area, &mut app.list_state);

    if visible_count > 0 {
        let mut scrollbar_state =
            ScrollbarState::new(visible_count).position(app.list_state.selected().unwrap_or(0));
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(theme::ACCENT))
            .track_style(Style::default().fg(theme::SURFACE_DIM));
        f.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }
}

/// A category heading inside grouped search results.
///
/// `Row::style` paints the whole row area - including the gaps between
/// columns - so this reads as one unbroken band across the pane even though
/// its text only occupies the first two cells.
pub(super) fn draw_group_header_row<'a>(category: &str, count: usize) -> Row<'a> {
    let color = theme::category_color(category);
    Row::new(vec![Cell::new(Line::from(vec![
        Span::styled(
            format!("{} ", theme::category_icon(category)),
            Style::default().fg(color),
        ),
        Span::styled(
            category.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        // Trailing the name rather than sitting in the next column, where it
        // would line up under "Version" and read as one. It's also the first
        // thing to be clipped in a narrow pane, which is the right thing to
        // lose.
        Span::styled(
            format!("  \u{b7}  {count}"),
            Style::default().fg(theme::ACCENT_DIM),
        ),
    ]))])
    .style(Style::default().bg(theme::GROUP_BG))
}

/// One package row. `grouped` indents it beneath its category heading, `alt`
/// applies the zebra stripe.
/// Byte range of the first case-insensitive occurrence of `needle` in
/// `haystack` at or after byte offset `from` (which must sit on a char
/// boundary). Folding happens one char at a time on both sides, so the
/// returned offsets always land on char boundaries of the original string -
/// unlike the naive `to_lowercase()`-then-`find` approach, where a folding
/// char that changes byte length (ß, İ, ...) would shift every offset
/// after it.
pub(super) fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let folded_needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    for (offset, _) in haystack[from..].char_indices() {
        let start = from + offset;
        let mut want = folded_needle.iter();
        for (i, c) in haystack[start..].char_indices() {
            let mut matched = true;
            for fc in c.to_lowercase() {
                match want.next() {
                    Some(&w) if w == fc => {}
                    _ => {
                        matched = false;
                        break;
                    }
                }
            }
            if !matched {
                break;
            }
            if want.len() == 0 {
                return Some((start, start + i + c.len_utf8()));
            }
        }
    }
    None
}

/// Splits `text` into spans with every case-insensitive occurrence of
/// `needle` restyled in the match color, so search hits show *why* a row is
/// in the result list. With an empty needle this is just one span of `base`.
pub(super) fn highlight_matches<'a>(text: &str, needle: &str, base: Style) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut at = 0;
    while let Some((start, end)) = find_ci(text, needle, at) {
        if start > at {
            spans.push(Span::styled(text[at..start].to_string(), base));
        }
        spans.push(Span::styled(
            text[start..end].to_string(),
            Style::default()
                .fg(theme::MATCH)
                .add_modifier(Modifier::BOLD),
        ));
        at = end;
    }
    if at < text.len() || spans.is_empty() {
        spans.push(Span::styled(text[at..].to_string(), base));
    }
    spans
}

pub(super) fn draw_package_row<'a>(
    app: &App,
    p: &ClassifiedPackage,
    grouped: bool,
    selecting: bool,
    show_category: bool,
    alt: bool,
) -> Row<'a> {
    let mut name_spans = Vec::new();
    // Sit under the heading rather than beside it, so the grouping reads as
    // a hierarchy at a glance.
    if grouped {
        name_spans.push(Span::raw("  "));
    }
    if selecting {
        if app.selected_names.contains(&p.package.name) {
            name_spans.push(Span::styled(
                format!("{} ", theme::checked_icon()),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            name_spans.push(Span::raw("  "));
        }
    }
    // Glyph alone says formula vs cask. It used to be tinted by
    // classification source too, which meant one character carrying
    // two unrelated meanings and reading as neither; the source is
    // spelled out in the detail pane instead.
    name_spans.push(Span::styled(
        format!("{} ", theme::kind_icon(p.package.kind)),
        Style::default().fg(theme::ACCENT_DIM),
    ));
    name_spans.extend(highlight_matches(
        &p.package.name,
        &app.filter,
        Style::default(),
    ));
    if p.source == ClassificationSource::Manual {
        name_spans.push(Span::styled(
            format!(" {}", theme::manual_icon()),
            Style::default().fg(theme::MANUAL),
        ));
    }
    if p.note.is_some() {
        name_spans.push(Span::styled(
            format!(" {}", theme::note_icon()),
            Style::default().fg(theme::LABEL),
        ));
    }
    if p.package.pinned {
        name_spans.push(Span::styled(
            format!(" {}", theme::pin_icon()),
            Style::default().fg(theme::HEURISTIC),
        ));
    }
    if p.package.outdated.is_some() {
        name_spans.push(Span::styled(
            format!(" {}", theme::outdated_icon()),
            Style::default()
                .fg(theme::OUTDATED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if p.package.unmaintained {
        name_spans.push(Span::styled(
            format!(" {}", theme::unmaintained_icon()),
            Style::default()
                .fg(theme::DANGER)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let mut cells = vec![Cell::new(Line::from(name_spans))];
    if show_category {
        // Only the glyph carries the category's color now - enough
        // to tie a row to its sidebar entry, without painting a
        // whole column in nineteen competing hues.
        cells.push(Cell::new(Line::from(vec![
            Span::styled(
                format!("{} ", theme::category_icon(&p.category)),
                Style::default().fg(theme::category_color(&p.category)),
            ),
            Span::styled(p.category.clone(), Style::default().fg(theme::LABEL)),
        ])));
    }
    cells.push(Cell::new(p.package.version.clone()).style(Style::default().fg(theme::LABEL)));
    cells.push(Cell::new(Line::from(highlight_matches(
        &p.package.desc,
        &app.filter,
        Style::default(),
    ))));

    // Zebra striping: a background step small enough to read as
    // texture rather than state. The selected row's highlight is
    // painted after this and wins, so the two never fight.
    let row = Row::new(cells);
    if alt {
        row.style(Style::default().bg(theme::ROW_ALT_BG))
    } else {
        row
    }
}

/// The detail pane runs as a horizontal band under the sidebar/list (rather
/// than a tall column beside them), so its content is laid out in two
/// side-by-side columns to make use of the width: package info/description
/// on the left, the dependency tree on the right.
pub(super) fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    // Refresh the size cache first (a filesystem walk), before borrowing a
    // package reference for line-building below - keeps the two mutable /
    // immutable uses of `app` from overlapping.
    let selected_name = app.selected_package().map(|p| p.package.name.clone());
    if let Some(name) = &selected_name {
        if app.size_cache.as_ref().map(|(n, _)| n) != Some(name) {
            let found = app
                .packages
                .iter()
                .find(|p| &p.package.name == name)
                .map(|p| {
                    (
                        p.package.kind,
                        p.package.name.clone(),
                        p.package.version.clone(),
                    )
                });
            if let Some((kind, name, version)) = found {
                let size = crate::cache::size_or_compute(
                    &mut app.disk_sizes,
                    &mut app.disk_sizes_dirty,
                    kind,
                    &name,
                    &version,
                );
                app.size_cache = Some((name, size));
            }
        }
    }

    let block = Block::default()
        .title(Span::styled(
            " Details ",
            Style::default()
                .fg(theme::LABEL)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::ACCENT_DIM));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(p) = app.selected_package() else {
        f.render_widget(
            Paragraph::new("No package selected.").style(Style::default().fg(theme::ACCENT_DIM)),
            inner,
        );
        return;
    };

    // The detail pane is a fixed six usable lines, so fields are packed
    // several to a row and identified by their glyph rather than a written
    // label - the icon already says "size" or "installed", and the labels
    // cost ten columns each to repeat it.
    let dot = || Span::styled("  \u{b7}  ", Style::default().fg(theme::ACCENT_DIM));
    let tagged = |icon: &str, value: String, style: Style| {
        vec![
            Span::styled(format!("{icon} "), Style::default().fg(theme::LABEL)),
            Span::styled(value, style),
        ]
    };

    // 1: name + kind
    let mut info_lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", theme::kind_icon(p.package.kind)),
            Style::default().fg(theme::ACCENT_DIM),
        ),
        Span::styled(
            p.package.name.clone(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", p.package.kind.as_str()),
            Style::default().fg(theme::ACCENT_DIM),
        ),
    ])];

    // 2: category (+ where it came from) and publisher
    let mut line = vec![
        Span::styled(
            format!("{} ", theme::category_icon(&p.category)),
            Style::default().fg(theme::category_color(&p.category)),
        ),
        Span::styled(
            p.category.clone(),
            Style::default().fg(theme::category_color(&p.category)),
        ),
        Span::styled(
            format!(" [{}]", p.source.as_str()),
            Style::default().fg(theme::source_color(p.source)),
        ),
    ];
    line.push(dot());
    line.extend(tagged(
        theme::publisher_icon(),
        p.package.tap.clone(),
        Style::default().fg(theme::LABEL),
    ));
    info_lines.push(Line::from(line));

    // 3: version, size, install date - the numbers, on one line
    let mut line = tagged(
        theme::version_icon(),
        p.package.version.clone(),
        Style::default(),
    );
    if let Some(newer) = &p.package.outdated {
        line.push(Span::styled(
            format!("  {} {newer}", theme::outdated_icon()),
            Style::default()
                .fg(theme::OUTDATED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if p.package.pinned {
        line.push(Span::styled(
            format!("  {} pinned", theme::pin_icon()),
            Style::default().fg(theme::HEURISTIC),
        ));
    }
    if let Some((_, Some(size))) = &app.size_cache {
        line.push(dot());
        line.extend(tagged(
            theme::size_icon(),
            details::format_size(*size),
            Style::default().fg(theme::size_color(*size)),
        ));
    }
    if let Some(installed_at) = p.package.installed_at {
        line.push(dot());
        line.extend(tagged(
            theme::time_icon(),
            details::format_age(installed_at),
            Style::default().fg(theme::LABEL),
        ));
    }
    info_lines.push(Line::from(line));

    // 4: homepage
    info_lines.push(Line::from(tagged(
        theme::link_icon(),
        p.package.homepage.clone(),
        Style::default().fg(theme::HEURISTIC),
    )));

    // 5: the deprecation warning if there is one, else the description -
    // a package Homebrew has given up on is the more urgent of the two.
    if p.package.unmaintained {
        info_lines.push(Line::from(Span::styled(
            format!(
                "{} No longer maintained ({})",
                theme::unmaintained_icon(),
                p.package
                    .unmaintained_reason
                    .as_deref()
                    .unwrap_or("unspecified reason")
            ),
            Style::default()
                .fg(theme::DANGER)
                .add_modifier(Modifier::BOLD),
        )));
    }
    info_lines.push(Line::from(Span::styled(
        p.package.desc.clone(),
        Style::default().fg(theme::LABEL),
    )));

    // 6: the note, if one was written
    if let Some(note) = &p.note {
        info_lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", theme::note_icon()),
                Style::default().fg(theme::MANUAL),
            ),
            Span::styled(
                note.clone(),
                Style::default()
                    .fg(theme::MANUAL)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
    }

    let index: BTreeMap<String, &Package> = app
        .packages
        .iter()
        .map(|cp| (cp.package.name.clone(), &cp.package))
        .collect();
    let (deps, truncated) = details::dependency_tree(&index, &p.package.name, 4, 30);
    let mut dep_lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", theme::deps_icon()),
            Style::default().fg(theme::LABEL),
        ),
        Span::styled(
            "Dependencies",
            Style::default()
                .fg(theme::LABEL)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if deps.is_empty() {
        dep_lines.push(Line::from(Span::styled(
            "  (none)",
            Style::default().fg(theme::ACCENT_DIM),
        )));
    } else {
        for dep in &deps {
            // Box-drawing branch marks turn the flat, indented list into
            // something that actually reads as a tree.
            let indent = "  ".repeat(dep.depth.saturating_sub(1));
            let (version_text, version_style) = match &dep.version {
                Some(version) => (
                    format!(" {version}"),
                    Style::default().fg(theme::ACCENT_DIM),
                ),
                None => (
                    " (not installed)".to_string(),
                    Style::default().fg(theme::DANGER),
                ),
            };
            dep_lines.push(Line::from(vec![
                Span::styled(
                    format!("{indent}\u{2514}\u{2500} "),
                    Style::default().fg(theme::SURFACE_DIM),
                ),
                Span::raw(dep.name.clone()),
                Span::styled(version_text, version_style),
            ]));
        }
        if truncated {
            dep_lines.push(Line::from(Span::styled(
                "  \u{2026} (truncated)",
                Style::default().fg(theme::ACCENT_DIM),
            )));
        }
    }

    // "Required by" answers the opposite question from the dependency tree:
    // not "what does this need" but "what needs this" - which, for anything
    // that arrived as a dependency, is the whole story of why it's here.
    let required_by =
        details::reverse_dependencies(app.packages.iter().map(|cp| &cp.package), &p.package.name);
    let mut req_lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", theme::deps_icon()),
            Style::default().fg(theme::LABEL),
        ),
        Span::styled(
            "Required by",
            Style::default()
                .fg(theme::LABEL)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    if required_by.is_empty() {
        let hint = if p.package.installed_on_request {
            "  (nothing \u{2013} installed on request)"
        } else {
            "  (nothing)"
        };
        req_lines.push(Line::from(Span::styled(
            hint,
            Style::default().fg(theme::ACCENT_DIM),
        )));
    } else {
        // The pane has six usable lines; the header takes one, so four
        // names fit before the tail has to be summarized.
        for name in required_by.iter().take(4) {
            req_lines.push(Line::from(vec![
                Span::styled(
                    "  \u{2514}\u{2500} ",
                    Style::default().fg(theme::SURFACE_DIM),
                ),
                Span::raw(name.clone()),
            ]));
        }
        if required_by.len() > 4 {
            req_lines.push(Line::from(Span::styled(
                format!("  \u{2026} +{} more", required_by.len() - 4),
                Style::default().fg(theme::ACCENT_DIM),
            )));
        }
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(48),
            Constraint::Percentage(28),
            Constraint::Percentage(24),
        ])
        .split(inner);

    f.render_widget(
        Paragraph::new(info_lines).wrap(Wrap { trim: true }),
        columns[0],
    );
    f.render_widget(
        Paragraph::new(dep_lines).wrap(Wrap { trim: true }),
        columns[1],
    );
    f.render_widget(
        Paragraph::new(req_lines).wrap(Wrap { trim: true }),
        columns[2],
    );
}

/// Key hints shown along the footer in Normal mode - the handful worth
/// keeping permanently visible, with `?` pointing at the full list.
pub(super) const FOOTER_HINTS: &[(&str, &str)] = &[
    ("tab", "pane"),
    ("j/k", "move"),
    ("\u{21b5}", "menu"),
    ("/", "filter"),
    ("s", "sort"),
    ("u", "update"),
    ("?", "help"),
    ("q", "quit"),
];

pub(super) fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    // Text-entry modes show a prompt with a cursor; everything else shows
    // either a transient status message or the standing key hints.
    let prompt = match app.mode {
        InputMode::Filter => Some(("Filter", theme::HEURISTIC)),
        InputMode::Note => Some(("Note", theme::MANUAL)),
        InputMode::Category => Some(("Category", theme::ACCENT)),
        _ => None,
    };

    let line = if let Some((label, color)) = prompt {
        Line::from(vec![
            Span::styled(
                format!(" {label} "),
                Style::default()
                    .fg(theme::HEADER_BG)
                    .bg(color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::raw(app.input_buffer.clone()),
            Span::styled("\u{2588}", Style::default().fg(color)),
        ])
    } else if !app.status.is_empty() {
        Line::from(vec![
            Span::styled(
                format!(" {} ", theme::info_icon()),
                Style::default().fg(theme::ACCENT),
            ),
            Span::styled(app.status.clone(), Style::default().fg(theme::LABEL)),
        ])
    } else {
        let mut spans = Vec::new();
        for (key, action) in FOOTER_HINTS {
            spans.push(Span::styled(
                format!(" {key} "),
                Style::default()
                    .fg(theme::HEADER_BG)
                    .bg(theme::ACCENT_DIM)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {action}  "),
                Style::default().fg(theme::LABEL),
            ));
        }
        Line::from(spans)
    };

    let paragraph = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::ACCENT_DIM)),
    );
    f.render_widget(paragraph, area);
}

/// Computes a centered popup `Rect` covering `percent_x`/`percent_y` of
/// `area` - the standard ratatui recipe for modal-style overlays.
pub(super) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

pub(super) fn draw_help_overlay(f: &mut Frame) {
    let lines = vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Tab              Switch focus between sidebar and list"),
        Line::from("j/k, \u{2191}/\u{2193}         Move selection"),
        Line::from("/                Search all categories, live (name, description, note)"),
        Line::from("d                Toggle showing dependency-only packages"),
        Line::from("s                Cycle sort order (name / size / install date)"),
        Line::from("r                Refresh package data from brew"),
        Line::from(""),
        Line::from(Span::styled(
            "Selected package",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Enter            Open the action menu (incl. uninstall)"),
        Line::from("u                Update via `brew upgrade` (asks for confirmation)"),
        Line::from("U                Update everything outdated (skips pinned)"),
        Line::from("p                Pin/unpin a formula (`brew pin`)"),
        Line::from("n                Edit note"),
        Line::from("c                Pick category (type to filter, Enter to apply)"),
        Line::from("R                Reset manual category override"),
        Line::from("o                Open homepage in browser"),
        Line::from("y                Copy package name to clipboard"),
        Line::from(""),
        Line::from(Span::styled(
            "Sidebar",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Outdated         Packages with an update available"),
        Line::from("Unmaintained     Packages Homebrew has deprecated or disabled"),
        Line::from("Orphaned         Dependency-only packages nothing needs (autoremove)"),
        Line::from(""),
        Line::from(Span::styled(
            "Multi-select",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Space            Toggle selection; c/R then apply to all selected"),
        Line::from("Esc              Clear selection (or quit, if nothing selected)"),
        Line::from(""),
        Line::from("q                Quit"),
        Line::from(""),
        Line::from(Span::styled(
            "Press any key to close",
            Style::default().add_modifier(Modifier::ITALIC),
        )),
    ];

    // Sized to the actual line count (like the action menu), so growing the
    // list can't silently clip the bottom entries at a fixed percentage.
    let screen = f.area();
    let height = (lines.len() as u16 + 2).min(screen.height);
    let width = (screen.width * 60 / 100).clamp(40.min(screen.width), screen.width);
    let area = Rect {
        x: screen.x + (screen.width.saturating_sub(width)) / 2,
        y: screen.y + (screen.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    f.render_widget(Clear, area);

    let block = Block::default()
        .title(Span::styled(
            format!(" {} Keybindings ", theme::info_icon()),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme::ACCENT));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

pub(super) fn draw_confirm_overlay(f: &mut Frame, app: &mut App) {
    let area = centered_rect(46, 40, f.area());
    app.hitboxes.confirm = area;
    f.render_widget(Clear, area);

    // Updates are routine, so they get the warm "heads-up" orange; removal
    // is the destructive one and wears the alarm color throughout.
    let (color, title, question) = match app.confirm_action {
        BrewAction::Upgrade => (
            theme::OUTDATED,
            format!(" {} Confirm Update ", theme::outdated_icon()),
            format!(
                "Update {} package{} via `brew upgrade`?",
                app.confirm_targets.len(),
                if app.confirm_targets.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        ),
        BrewAction::Uninstall => (
            theme::DANGER,
            format!(" {} Confirm Uninstall ", theme::unmaintained_icon()),
            format!(
                "Uninstall {} package{} via `brew uninstall`? This removes {} from disk.",
                app.confirm_targets.len(),
                if app.confirm_targets.len() == 1 {
                    ""
                } else {
                    "s"
                },
                if app.confirm_targets.len() == 1 {
                    "it"
                } else {
                    "them"
                }
            ),
        ),
    };

    let mut lines = vec![
        Line::from(Span::styled(
            question,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (kind, name) in app.confirm_targets.iter().take(10) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", theme::kind_icon(*kind)),
                Style::default().fg(color),
            ),
            Span::raw(name.clone()),
        ]));
    }
    if app.confirm_targets.len() > 10 {
        lines.push(Line::from(format!(
            "  ... and {} more",
            app.confirm_targets.len() - 10
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "  y  ",
            Style::default()
                .fg(theme::HEADER_BG)
                .bg(theme::CURATED)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Confirm     ", Style::default().fg(theme::LABEL)),
        Span::styled(
            " n/Esc ",
            Style::default()
                .fg(theme::HEADER_BG)
                .bg(theme::ACCENT_DIM)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Cancel", Style::default().fg(theme::LABEL)),
    ]));

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(color));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

pub(super) fn draw_menu_overlay(f: &mut Frame, app: &mut App) {
    // Own the name (rather than holding the `&ClassifiedPackage` borrow)
    // before touching `app.hitboxes` below - the two would otherwise
    // overlap an immutable and a mutable borrow of `app`.
    let Some(name) = app.selected_package().map(|p| p.package.name.clone()) else {
        return;
    };
    // Exact height from the item count (plus borders), not a percentage -
    // a percentage silently clips the last entries once the menu grows or
    // the terminal shrinks.
    let screen = f.area();
    let height = (MENU_ITEMS.len() as u16 + 2).min(screen.height);
    let width = (screen.width * 36 / 100).max(30).min(screen.width);
    let area = Rect {
        x: screen.x + (screen.width.saturating_sub(width)) / 2,
        y: screen.y + (screen.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    app.hitboxes.menu = area;
    app.hitboxes.menu_rows_top = area.y + 1;
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = MENU_ITEMS
        .iter()
        .enumerate()
        .map(|(i, (key, label))| {
            let key_label = if *key == ' ' {
                "Space".to_string()
            } else {
                key.to_string()
            };
            let selected = i == app.menu_index;
            let row_style = if selected {
                Style::default()
                    .bg(theme::HIGHLIGHT_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    if selected { " \u{258c} " } else { "   " },
                    Style::default().fg(theme::ACCENT),
                ),
                Span::styled(
                    format!("{key_label:^7}"),
                    Style::default()
                        .fg(theme::HEADER_BG)
                        .bg(if selected {
                            theme::ACCENT
                        } else {
                            theme::ACCENT_DIM
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {label}"), row_style),
            ]))
        })
        .collect();

    let block = Block::default()
        .title(Span::styled(
            format!(" {name} "),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme::ACCENT));
    f.render_widget(List::new(items).block(block), area);
}

/// The category picker: a type-to-filter list of every category, opened by
/// `c`. Line 1 inside the popup is the filter prompt; the rows below it are
/// the candidates, with a final "new category" row as the free-text escape
/// hatch (which is why the row count is `candidates.len() + 1` everywhere).
pub(super) fn draw_picker_overlay(f: &mut Frame, app: &mut App) {
    let candidates = app.picker_candidates();

    let target = if app.selected_names.len() > 1 {
        format!("{} packages", app.selected_names.len())
    } else if let Some(p) = app.selected_package() {
        p.package.name.clone()
    } else {
        return;
    };

    let area = centered_rect(42, 62, f.area());
    app.hitboxes.picker = area;
    // Rows start below the border and the filter prompt line.
    app.hitboxes.picker_rows_top = area.y + 2;
    f.render_widget(Clear, area);

    let selected = app.picker_state.selected().unwrap_or(0);
    let mut items: Vec<ListItem> = candidates
        .iter()
        .map(|c| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", theme::category_icon(c)),
                    Style::default().fg(theme::category_color(c)),
                ),
                Span::raw(c.clone()),
            ]))
        })
        .collect();
    let new_label = if app.input_buffer.trim().is_empty() {
        "  New category\u{2026}".to_string()
    } else {
        format!("  New category \"{}\"\u{2026}", app.input_buffer.trim())
    };
    items.push(ListItem::new(Line::from(Span::styled(
        new_label,
        Style::default()
            .fg(theme::ACCENT_DIM)
            .add_modifier(Modifier::ITALIC),
    ))));

    let block = Block::default()
        .title(Span::styled(
            format!(" Set category \u{2013} {target} "),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme::ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [prompt_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {} ", theme::filter_icon()),
                Style::default().fg(theme::HEURISTIC),
            ),
            Span::raw(app.input_buffer.clone()),
            Span::styled("\u{2588}", Style::default().fg(theme::ACCENT)),
        ]))
        .style(Style::default().bg(theme::HEADER_BG)),
        prompt_area,
    );

    // Keep the shared state in range for however many rows survived the
    // current filter text, then let ratatui handle scroll-into-view.
    app.picker_state
        .select(Some(selected.min(candidates.len())));
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme::HIGHLIGHT_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("\u{258c}");
    f.render_stateful_widget(list, list_area, &mut app.picker_state);
}
