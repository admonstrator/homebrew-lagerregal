//! Keyboard and mouse handling, plus the actions they trigger (note/category
//! editing, sorting, selection, updates, clipboard).

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::classify::{self, ClassificationSource};
use crate::details;
use crate::homebrew::{self, PackageKind};
use crate::store::State;

use super::app::*;

/// Actions in the `m`-triggered action menu. All but the last are also
/// bound to the same letter in Normal mode; uninstall is menu-only on
/// purpose, so removal is never a single stray keypress away.
pub(super) const MENU_ITEMS: &[(char, &str)] = &[
    ('u', "Update (brew upgrade)"),
    ('p', "Pin/Unpin (formulae)"),
    ('n', "Edit note"),
    ('c', "Set category"),
    ('R', "Reset category"),
    ('o', "Open homepage"),
    ('y', "Copy name"),
    (' ', "Toggle multi-select"),
    ('x', "Uninstall (brew uninstall)"),
];

pub(super) fn handle_normal_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => {
            if !app.selected_names.is_empty() {
                app.selected_names.clear();
                app.status = "Selection cleared".to_string();
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Sidebar => Focus::List,
                Focus::List => Focus::Sidebar,
            };
        }
        KeyCode::Down | KeyCode::Char('j') => move_selection(app, 1),
        KeyCode::Up | KeyCode::Char('k') => move_selection(app, -1),
        KeyCode::Char('/') => {
            app.input_buffer = app.filter.clone();
            app.filter_backup = app.filter.clone();
            app.mode = InputMode::Filter;
        }
        KeyCode::Char('?') => app.mode = InputMode::Help,
        KeyCode::Enter => launch_selected(app),
        KeyCode::Char('m') => open_menu(app),
        KeyCode::Char('n') => start_note_edit(app),
        KeyCode::Char('c') => start_category_edit(app),
        KeyCode::Char('R') => reset_category(app),
        KeyCode::Char('o') => open_homepage(app),
        KeyCode::Char('y') => copy_name(app),
        KeyCode::Char(' ') => toggle_select(app),
        KeyCode::Char('s') => cycle_sort(app),
        KeyCode::Char('u') => start_update(app),
        KeyCode::Char('U') => start_update_all(app),
        KeyCode::Char('p') => toggle_pin(app),
        KeyCode::Char('r') => {
            app.pending_refresh = true;
            app.status = "Refreshing from brew\u{2026}".to_string();
        }
        KeyCode::Char('d') => {
            app.show_deps = !app.show_deps;
            app.reset_list_selection();
        }
        _ => {}
    }
}

pub(super) fn move_selection(app: &mut App, delta: i32) {
    match app.focus {
        Focus::Sidebar => {
            let len = app.sidebar_categories().len();
            if len == 0 {
                return;
            }
            let i = app.sidebar_state.selected().unwrap_or(0) as i32;
            let new_i = (i + delta).rem_euclid(len as i32) as usize;
            app.sidebar_state.select(Some(new_i));
            app.reset_list_selection();
        }
        Focus::List => {
            // Stepping happens over the selectable rows, not the rendered
            // ones, so moving through grouped results skips the headings
            // (and still wraps around at either end).
            let selectable = app.selectable_rows();
            if selectable.is_empty() {
                app.list_state.select(None);
                return;
            }
            let current = app
                .list_state
                .selected()
                .and_then(|i| selectable.iter().position(|&s| s == i))
                .unwrap_or(0) as i32;
            let next = (current + delta).rem_euclid(selectable.len() as i32) as usize;
            app.list_state.select(Some(selectable[next]));
        }
    }
}

pub(super) fn handle_text_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            // The live filter has already reshaped the list while typing, so
            // cancelling has to put back what was there when `/` was pressed.
            if app.mode == InputMode::Filter {
                app.filter = app.filter_backup.clone();
                app.reset_list_selection();
            }
            app.input_buffer.clear();
            app.mode = InputMode::Normal;
        }
        KeyCode::Enter => {
            apply_input(app);
            app.input_buffer.clear();
            app.mode = InputMode::Normal;
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
            sync_live_filter(app);
        }
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
            sync_live_filter(app);
        }
        _ => {}
    }
}

/// While the search prompt is open, every keystroke is applied immediately -
/// the point of searching is watching the candidates narrow down live. Note
/// and category input stay buffered; nothing about them is worth previewing.
fn sync_live_filter(app: &mut App) {
    if app.mode == InputMode::Filter {
        app.filter = app.input_buffer.clone();
        app.reset_list_selection();
    }
}

/// Handles a keypress while the action menu (`m` / right-click) is open:
/// up/down (or j/k) move the highlight, Enter activates the highlighted
/// action.
/// To keep the menu fast for anyone who already knows the shortcuts,
/// pressing a menu item's own letter activates it immediately too.
pub(super) fn handle_menu_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.mode = InputMode::Normal,
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_index = app
                .menu_index
                .checked_sub(1)
                .unwrap_or(MENU_ITEMS.len() - 1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.menu_index = (app.menu_index + 1) % MENU_ITEMS.len();
        }
        KeyCode::Enter => {
            let (key, _) = MENU_ITEMS[app.menu_index];
            app.mode = InputMode::Normal;
            trigger_menu_action(app, key);
        }
        KeyCode::Char(' ') => {
            app.mode = InputMode::Normal;
            trigger_menu_action(app, ' ');
        }
        KeyCode::Char(c) if MENU_ITEMS.iter().any(|(k, _)| *k == c) => {
            app.mode = InputMode::Normal;
            trigger_menu_action(app, c);
        }
        _ => {}
    }
}

/// Handles a keypress while the update-confirmation overlay is open. Only
/// an explicit yes proceeds; anything else (including an unrecognized key)
/// cancels, since `brew upgrade` is the one action in this app that reaches
/// outside `lagerregal`'s own state and modifies the real Homebrew install.
pub(super) fn handle_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.pending_brew = Some((app.confirm_action, std::mem::take(&mut app.confirm_targets)));
            app.mode = InputMode::Normal;
        }
        _ => {
            app.confirm_targets.clear();
            app.mode = InputMode::Normal;
        }
    }
}

/// Dispatches a mouse event to the handler for whatever mode is currently
/// active - mirrors the `match app.mode` in the keyboard path in `run_app`,
/// just one level down since mouse handling needs more per-mode code than
/// fits cleanly inline there.
pub(super) fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    match app.mode {
        InputMode::Normal => handle_normal_mouse(app, mouse),
        InputMode::Menu => handle_menu_mouse(app, mouse),
        InputMode::Confirm => handle_confirm_mouse(app, mouse),
        InputMode::CategoryPick => handle_picker_mouse(app, mouse),
        // A click is as good as a keypress for dismissing the help overlay.
        InputMode::Help => {
            if matches!(mouse.kind, MouseEventKind::Down(_)) {
                app.mode = InputMode::Normal;
            }
        }
        // Text input (filter/note/category) stays keyboard-only - there's
        // no sensible click target inside a single-line text field here.
        InputMode::Filter | InputMode::Note | InputMode::Category => {}
    }
}

/// Maps a screen row inside a hitbox to an item index, given where the
/// hitbox's first row starts and how many rows are currently scrolled past
/// (the widget's own `offset()`, which ratatui keeps in sync with what it
/// actually rendered last frame).
pub(super) fn row_to_index(row: u16, rows_top: u16, offset: usize) -> Option<usize> {
    row.checked_sub(rows_top)
        .map(|delta| offset + delta as usize)
}

pub(super) fn handle_normal_mouse(app: &mut App, mouse: MouseEvent) {
    let pos = Position::new(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if app.hitboxes.sidebar.contains(pos) {
                app.focus = Focus::Sidebar;
                move_selection(app, 1);
            } else if app.hitboxes.list.contains(pos) {
                app.focus = Focus::List;
                move_selection(app, 1);
            }
        }
        MouseEventKind::ScrollUp => {
            if app.hitboxes.sidebar.contains(pos) {
                app.focus = Focus::Sidebar;
                move_selection(app, -1);
            } else if app.hitboxes.list.contains(pos) {
                app.focus = Focus::List;
                move_selection(app, -1);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if app.hitboxes.sidebar.contains(pos) {
                app.focus = Focus::Sidebar;
                if let Some(idx) = row_to_index(
                    mouse.row,
                    app.hitboxes.sidebar_rows_top,
                    app.sidebar_state.offset(),
                ) && idx < app.sidebar_categories().len()
                {
                    app.sidebar_state.select(Some(idx));
                    app.reset_list_selection();
                }
            } else if app.hitboxes.list.contains(pos) {
                app.focus = Focus::List;
                if let Some(idx) = row_to_index(
                    mouse.row,
                    app.hitboxes.list_rows_top,
                    app.list_state.offset(),
                ) {
                    // Clicking a group heading drops the cursor onto the
                    // first package underneath it, so headings behave like
                    // a target rather than a dead row. Clicking past the
                    // last row resolves to nothing and is ignored.
                    let target = app.selectable_rows().into_iter().find(|&s| s >= idx);
                    if let Some(target) = target {
                        app.list_state.select(Some(target));
                        let now = Instant::now();
                        // Only a direct hit on the package row counts
                        // towards a double-click - a heading resolving to
                        // the same row shouldn't launch that package.
                        let is_double_click = target == idx
                            && app.last_list_click.is_some_and(|(t, i)| {
                                i == idx && now.duration_since(t) < Duration::from_millis(400)
                            });
                        if is_double_click {
                            app.last_list_click = None;
                            launch_selected(app);
                        } else {
                            app.last_list_click = Some((now, target));
                        }
                    }
                }
            }
        }
        // Right-click a package as a shortcut straight to its action menu -
        // select it first (in case it wasn't already), like a left click.
        MouseEventKind::Down(MouseButton::Right) if app.hitboxes.list.contains(pos) => {
            app.focus = Focus::List;
            if let Some(idx) = row_to_index(
                mouse.row,
                app.hitboxes.list_rows_top,
                app.list_state.offset(),
            ) && let Some(target) = app.selectable_rows().into_iter().find(|&s| s >= idx)
            {
                app.list_state.select(Some(target));
            }
            if app.selected_package().is_some() {
                app.menu_index = 0;
                app.mode = InputMode::Menu;
            }
        }
        _ => {}
    }
}

pub(super) fn handle_menu_mouse(app: &mut App, mouse: MouseEvent) {
    if !matches!(mouse.kind, MouseEventKind::Down(_)) {
        return;
    }
    let pos = Position::new(mouse.column, mouse.row);
    if app.hitboxes.menu.contains(pos)
        && let Some(idx) = row_to_index(mouse.row, app.hitboxes.menu_rows_top, 0)
        && idx < MENU_ITEMS.len()
    {
        let (key, _) = MENU_ITEMS[idx];
        app.mode = InputMode::Normal;
        trigger_menu_action(app, key);
        return;
    }
    // Clicked outside the menu (or below its items) - dismiss it, same as Esc.
    app.mode = InputMode::Normal;
}

pub(super) fn handle_confirm_mouse(app: &mut App, mouse: MouseEvent) {
    if !matches!(mouse.kind, MouseEventKind::Down(_)) {
        return;
    }
    let pos = Position::new(mouse.column, mouse.row);
    if app.hitboxes.confirm.contains(pos) {
        app.pending_brew = Some((app.confirm_action, std::mem::take(&mut app.confirm_targets)));
    } else {
        app.confirm_targets.clear();
    }
    app.mode = InputMode::Normal;
}

pub(super) fn trigger_menu_action(app: &mut App, key: char) {
    match key {
        'u' => start_update(app),
        'p' => toggle_pin(app),
        'n' => start_note_edit(app),
        'c' => start_category_edit(app),
        'R' => reset_category(app),
        'o' => open_homepage(app),
        'y' => copy_name(app),
        ' ' => toggle_select(app),
        'x' => start_uninstall(app),
        _ => {}
    }
}

pub(super) fn start_note_edit(app: &mut App) {
    if let Some(pkg) = app.selected_package() {
        app.input_buffer = pkg.note.clone().unwrap_or_default();
        app.mode = InputMode::Note;
    }
}

pub(super) fn start_category_edit(app: &mut App) {
    if app.selected_package().is_some() {
        app.input_buffer.clear();
        app.picker_state = Default::default();
        app.picker_state.select(Some(0));
        app.mode = InputMode::CategoryPick;
    }
}

/// Handles a keypress while the category picker is open. Typing narrows the
/// candidate list, arrows move the highlight (j/k stay literal - they're
/// letters someone may be typing), Enter applies. The row after the last
/// candidate is "new category", which drops into the free-text editor with
/// whatever was typed carried over as the starting point.
pub(super) fn handle_picker_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.input_buffer.clear();
            app.mode = InputMode::Normal;
        }
        KeyCode::Up => {
            let last = app.picker_candidates().len();
            let i = app.picker_state.selected().unwrap_or(0);
            app.picker_state
                .select(Some(if i == 0 { last } else { i - 1 }));
        }
        KeyCode::Down => {
            let last = app.picker_candidates().len();
            let i = app.picker_state.selected().unwrap_or(0);
            app.picker_state
                .select(Some(if i >= last { 0 } else { i + 1 }));
        }
        KeyCode::Enter => {
            let candidates = app.picker_candidates();
            let i = app.picker_state.selected().unwrap_or(0);
            if let Some(category) = candidates.get(i).cloned() {
                app.input_buffer.clear();
                app.mode = InputMode::Normal;
                apply_category(app, &category);
            } else {
                // The "new category" row: switch to free text, keeping the
                // typed filter as the likely start of the new name.
                app.mode = InputMode::Category;
            }
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
            app.clamp_picker_selection();
        }
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
            // Whatever was highlighted is probably gone from the narrowed
            // list; the top hit is the best guess for what's being typed at.
            app.picker_state.select(Some(0));
        }
        _ => {}
    }
}

pub(super) fn handle_picker_mouse(app: &mut App, mouse: MouseEvent) {
    let last = app.picker_candidates().len();
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            let i = app.picker_state.selected().unwrap_or(0);
            app.picker_state.select(Some((i + 1).min(last)));
        }
        MouseEventKind::ScrollUp => {
            let i = app.picker_state.selected().unwrap_or(0);
            app.picker_state.select(Some(i.saturating_sub(1)));
        }
        MouseEventKind::Down(_) => {
            let pos = Position::new(mouse.column, mouse.row);
            if app.hitboxes.picker.contains(pos) {
                if let Some(idx) = row_to_index(
                    mouse.row,
                    app.hitboxes.picker_rows_top,
                    app.picker_state.offset(),
                ) && idx <= last
                {
                    app.picker_state.select(Some(idx));
                    // A click both selects and activates - mirroring the
                    // action menu, where a click is a decision, not a
                    // cursor movement.
                    handle_picker_key(app, KeyCode::Enter);
                }
            } else {
                // Clicked outside - dismiss, same as Esc.
                app.input_buffer.clear();
                app.mode = InputMode::Normal;
            }
        }
        _ => {}
    }
}

pub(super) fn toggle_select(app: &mut App) {
    let Some(name) = app.selected_package().map(|p| p.package.name.clone()) else {
        return;
    };
    if !app.selected_names.remove(&name) {
        app.selected_names.insert(name);
        // Advance to the next row so selecting a contiguous run of packages
        // (a common case for bulk categorization) doesn't need repeated
        // manual navigation between each Space press.
        move_selection(app, 1);
    }
    app.status = if app.selected_names.is_empty() {
        "Selection cleared".to_string()
    } else {
        format!("{} package(s) selected", app.selected_names.len())
    };
}

pub(super) fn cycle_sort(app: &mut App) {
    app.sort_mode = app.sort_mode.next();
    app.reset_list_selection();
    if app.sort_mode == SortMode::Size {
        let sig = app.scope_signature();
        let cached = app
            .size_sort_cache
            .as_ref()
            .map(|(s, _)| s.as_str() == sig.as_str())
            .unwrap_or(false);
        if cached {
            app.status = format!("Sorted by {}", app.sort_mode.label());
        } else {
            app.pending_size_sort = true;
            app.status = "Calculating sizes for sorting...".to_string();
        }
    } else {
        app.status = format!("Sorted by {}", app.sort_mode.label());
    }
}

/// Collects outdated targets (multi-selected, or just the highlighted
/// package) and opens the Confirm overlay - `brew upgrade` shells out and
/// can take a while, so it always needs an explicit yes rather than firing
/// on a single stray keypress.
pub(super) fn start_update(app: &mut App) {
    let bulk = !app.selected_names.is_empty();
    let targets: Vec<(PackageKind, String)> = if bulk {
        // Pinned packages drop out silently here: `brew upgrade` would
        // refuse them anyway, and a bulk update shouldn't die on the one
        // package the user deliberately froze.
        app.packages
            .iter()
            .filter(|p| {
                app.selected_names.contains(&p.package.name)
                    && p.package.outdated.is_some()
                    && !p.package.pinned
            })
            .map(|p| (p.package.kind, p.package.name.clone()))
            .collect()
    } else {
        app.selected_package()
            .filter(|p| p.package.outdated.is_some() && !p.package.pinned)
            .map(|p| (p.package.kind, p.package.name.clone()))
            .into_iter()
            .collect()
    };

    if targets.is_empty() {
        app.status = if bulk {
            "No updatable packages in selection (outdated, not pinned)".to_string()
        } else if let Some(pkg) = app.selected_package() {
            if pkg.package.pinned && pkg.package.outdated.is_some() {
                format!(
                    "\"{}\" is pinned \u{2013} press p to unpin first",
                    pkg.package.name
                )
            } else {
                format!("\"{}\" is already up to date", pkg.package.name)
            }
        } else {
            "No package selected".to_string()
        };
        return;
    }

    app.confirm_action = BrewAction::Upgrade;
    app.confirm_targets = targets;
    app.mode = InputMode::Confirm;
}

/// `U`: queue every outdated package in the current scope (minus pinned
/// ones) for one confirmed `brew upgrade` run - the "just bring everything
/// current" gesture, without having to multi-select each row first.
pub(super) fn start_update_all(app: &mut App) {
    let targets: Vec<(PackageKind, String)> = app
        .scoped_packages()
        .iter()
        .filter(|p| p.package.outdated.is_some() && !p.package.pinned)
        .map(|p| (p.package.kind, p.package.name.clone()))
        .collect();
    let pinned_skipped = app
        .scoped_packages()
        .iter()
        .filter(|p| p.package.outdated.is_some() && p.package.pinned)
        .count();

    if targets.is_empty() {
        app.status = if pinned_skipped > 0 {
            format!("Only pinned packages are outdated ({pinned_skipped})")
        } else {
            "Everything is up to date".to_string()
        };
        return;
    }

    // The footer stays visible under the confirm overlay, so the skip note
    // is readable while deciding.
    if pinned_skipped > 0 {
        app.status = format!(
            "{pinned_skipped} pinned package{} skipped",
            if pinned_skipped == 1 { "" } else { "s" }
        );
    }
    app.confirm_action = BrewAction::Upgrade;
    app.confirm_targets = targets;
    app.mode = InputMode::Confirm;
}

/// Queues the highlighted package - or the whole multi-selection - for
/// `brew uninstall`, behind the same confirm overlay as updates but styled
/// as the destructive action it is. Deliberately reachable only through the
/// action menu, not a top-level key: removal shouldn't be one stray
/// keypress away.
pub(super) fn start_uninstall(app: &mut App) {
    let bulk = !app.selected_names.is_empty();
    let targets: Vec<(PackageKind, String)> = if bulk {
        app.packages
            .iter()
            .filter(|p| app.selected_names.contains(&p.package.name))
            .map(|p| (p.package.kind, p.package.name.clone()))
            .collect()
    } else {
        app.selected_package()
            .map(|p| (p.package.kind, p.package.name.clone()))
            .into_iter()
            .collect()
    };
    if targets.is_empty() {
        app.status = "No package selected".to_string();
        return;
    }
    app.confirm_action = BrewAction::Uninstall;
    app.confirm_targets = targets;
    app.mode = InputMode::Confirm;
}

/// Pins or unpins the highlighted formula via `brew pin`/`brew unpin`.
/// Casks can't be pinned - Homebrew has no such concept for them.
pub(super) fn toggle_pin(app: &mut App) {
    let Some(pkg) = app.selected_package() else {
        return;
    };
    if pkg.package.kind == PackageKind::Cask {
        app.status = format!(
            "\"{}\" is a cask \u{2013} casks can't be pinned",
            pkg.package.name
        );
        return;
    }
    let name = pkg.package.name.clone();
    let pin = !pkg.package.pinned;
    match homebrew::set_pinned(&name, pin) {
        Ok(()) => {
            // Mirror the new state locally rather than re-reading everything
            // from brew - pinning changes exactly this one bit.
            if let Some(p) = app.packages.iter_mut().find(|p| p.package.name == name) {
                p.package.pinned = pin;
            }
            app.status = if pin {
                format!("Pinned \"{name}\" \u{2013} kept out of updates until unpinned")
            } else {
                format!("Unpinned \"{name}\"")
            };
        }
        Err(e) => app.status = format!("{e}"),
    }
}

/// Opens the action menu for the highlighted package - the slow path now
/// that Enter/double-click launch directly, reachable via `m` or a
/// right-click.
pub(super) fn open_menu(app: &mut App) {
    if app.selected_package().is_some() {
        app.menu_index = 0;
        app.mode = InputMode::Menu;
    }
}

/// Enter (and a list double-click) launch the highlighted package directly
/// rather than opening the action menu: a cask opens its app bundle via
/// `open` (fire-and-forget, no real terminal needed), a formula is queued
/// into `pending_launch` since running its CLI interactively needs the
/// terminal back from ratatui.
pub(super) fn launch_selected(app: &mut App) {
    let Some(pkg) = app.selected_package() else {
        return;
    };
    let name = pkg.package.name.clone();
    match pkg.package.kind {
        PackageKind::Cask => {
            let version = pkg.package.version.clone();
            match details::cask_app_path(&name, &version) {
                Some(path) => match Command::new("open").arg(&path).status() {
                    Ok(status) if status.success() => app.status = format!("Launched \"{name}\""),
                    _ => app.status = format!("Failed to launch \"{name}\" (is `open` on PATH?)"),
                },
                None => app.status = format!("\"{name}\" has no app to launch"),
            }
        }
        PackageKind::Formula => {
            let version = pkg.package.version.clone();
            let binaries = details::formula_binaries(&name, &version);
            match details::pick_binary(&name, &binaries) {
                Some(binary) => {
                    app.pending_launch = details::formula_bin_dir(&name, &version)
                        .map(|dir| dir.join(binary).to_string_lossy().into_owned())
                }
                None if binaries.is_empty() => {
                    app.status = format!("\"{name}\" installs no executable to launch")
                }
                None => {
                    app.status = format!(
                        "\"{name}\" installs {} executables \u{2013} run one yourself: {}",
                        binaries.len(),
                        binaries.join(", ")
                    )
                }
            }
        }
    }
}

pub(super) fn open_homepage(app: &mut App) {
    let Some(pkg) = app.selected_package() else {
        return;
    };
    if pkg.package.homepage.is_empty() {
        app.status = format!("\"{}\" has no homepage on record", pkg.package.name);
        return;
    }
    let url = pkg.package.homepage.clone();
    match Command::new("open").arg(&url).status() {
        Ok(status) if status.success() => app.status = format!("Opened {url}"),
        _ => app.status = format!("Failed to open {url} (is `open` on PATH?)"),
    }
}

pub(super) fn copy_name(app: &mut App) {
    let Some(pkg) = app.selected_package() else {
        return;
    };
    let name = pkg.package.name.clone();
    match copy_to_clipboard(&name) {
        Ok(()) => app.status = format!("Copied \"{name}\" to clipboard"),
        Err(_) => app.status = "Failed to copy to clipboard (is `pbcopy` on PATH?)".to_string(),
    }
}

pub(super) fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn pbcopy")?;
    child
        .stdin
        .as_mut()
        .context("pbcopy stdin unavailable")?
        .write_all(text.as_bytes())
        .context("failed to write to pbcopy")?;
    child.wait().context("pbcopy did not exit cleanly")?;
    Ok(())
}

pub(super) fn apply_input(app: &mut App) {
    match app.mode {
        InputMode::Filter => {
            app.filter = app.input_buffer.clone();
            app.reset_list_selection();
        }
        InputMode::Note => {
            let name = app.selected_package().map(|p| p.package.name.clone());
            if let Some(name) = name {
                // Submitting an empty note *removes* the note - otherwise a
                // cleared-out field would persist as `Some("")`, which still
                // shows the note marker while holding nothing.
                let cleared = app.input_buffer.trim().is_empty();
                if let Ok(mut state) = State::load() {
                    if cleared {
                        state.remove_note(&name);
                    } else {
                        state.set_note(&name, &app.input_buffer);
                    }
                    let _ = state.save();
                }
                if let Some(p) = app.packages.iter_mut().find(|p| p.package.name == name) {
                    p.note = if cleared {
                        None
                    } else {
                        Some(app.input_buffer.clone())
                    };
                }
                app.status = if cleared {
                    format!("Cleared note for {name}")
                } else {
                    format!("Saved note for {name}")
                };
            }
        }
        InputMode::Category => {
            let category = app.input_buffer.trim().to_string();
            if category.is_empty() {
                app.status = "Category name can't be empty".to_string();
                return;
            }
            apply_category(app, &category);
        }
        InputMode::Normal
        | InputMode::Help
        | InputMode::Menu
        | InputMode::Confirm
        | InputMode::CategoryPick => {}
    }
}

/// Persists a manual category for the highlighted package - or, when a
/// multi-selection is active, for every selected package - and mirrors the
/// change into the in-memory list. Shared by the picker and the free-text
/// editor, so both paths behave identically.
pub(super) fn apply_category(app: &mut App, category: &str) {
    let names: Vec<String> = if app.selected_names.is_empty() {
        app.selected_package()
            .map(|p| p.package.name.clone())
            .into_iter()
            .collect()
    } else {
        app.selected_names.iter().cloned().collect()
    };
    if names.is_empty() {
        return;
    }

    if let Ok(mut state) = State::load() {
        for name in &names {
            state.set_category(name, category);
        }
        let _ = state.save();
    }
    for p in app.packages.iter_mut() {
        if names.contains(&p.package.name) {
            p.category = category.to_string();
            p.source = ClassificationSource::Manual;
        }
    }
    app.status = if names.len() == 1 {
        format!("\"{}\" set to category \"{category}\"", names[0])
    } else {
        format!("{} packages set to category \"{category}\"", names.len())
    };
    app.selected_names.clear();
}

/// Clears a manual category override, falling back to whatever the
/// curated/heuristic classifier would have assigned. Applies to every
/// multi-selected package (skipping any that aren't manually classified) if
/// there's an active selection, otherwise just the highlighted package.
pub(super) fn reset_category(app: &mut App) {
    let bulk = !app.selected_names.is_empty();
    let names: Vec<String> = if bulk {
        app.packages
            .iter()
            .filter(|p| {
                app.selected_names.contains(&p.package.name)
                    && p.source == ClassificationSource::Manual
            })
            .map(|p| p.package.name.clone())
            .collect()
    } else {
        app.selected_package()
            .filter(|p| p.source == ClassificationSource::Manual)
            .map(|p| p.package.name.clone())
            .into_iter()
            .collect()
    };

    if names.is_empty() {
        // `bulk` (not just "is a package highlighted") decides the message,
        // since the highlighted row after multi-select can be a package
        // that was never actually part of the selection (Space auto-
        // advances the cursor past the last toggled row).
        app.status = if bulk {
            "No manually-classified packages in selection".to_string()
        } else if let Some(pkg) = app.selected_package() {
            format!("\"{}\" is not manually classified", pkg.package.name)
        } else {
            "No package selected".to_string()
        };
        return;
    }

    if let Ok(mut state) = State::load() {
        for name in &names {
            state.remove_category(name);
        }
        let _ = state.save();
    }
    let mut last_category = String::new();
    for name in &names {
        if let Some(p) = app.packages.iter_mut().find(|p| &p.package.name == name) {
            let (category, source) = classify::classify(name, &p.package.desc, None);
            p.category = category.clone();
            p.source = source;
            last_category = category;
        }
    }
    app.status = if names.len() == 1 {
        format!(
            "Cleared manual category for \"{}\" (now {last_category})",
            names[0]
        )
    } else {
        format!("Cleared manual category for {} package(s)", names.len())
    };
    app.selected_names.clear();
}
