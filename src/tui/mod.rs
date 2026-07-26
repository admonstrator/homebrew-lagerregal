//! The interactive dashboard: terminal setup/teardown, the event loop, and
//! the seams where the TUI hands the real terminal back to `brew`.
//!
//! Split by responsibility: [`app`] holds state and the row model, [`input`]
//! maps keys/mouse to actions, [`draw`] renders. Everything below stays
//! `pub(super)`-internal to this module tree.

use std::collections::HashMap;
use std::io::{self, BufRead, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::classify;
use crate::homebrew::{self, PackageKind};
use crate::store::State;
use crate::theme;

mod app;
mod draw;
mod input;
#[cfg(test)]
mod tests;

use app::*;
use draw::*;
use input::*;

pub fn run() -> Result<()> {
    let mut terminal = setup_terminal()?;
    // Loading Homebrew's package data takes ~1s (a real `brew` subprocess),
    // during which a blank alternate screen would look like a hang - so put
    // the terminal into alternate mode and show a splash *before* the slow
    // part, rather than after.
    let _ = draw_loading_screen(&mut terminal);

    let result = load_and_run(&mut terminal);
    restore_terminal(&mut terminal)?;
    result
}

fn load_and_run(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    // Goes through the shared cached/parallel loader, so a warm start skips
    // both `brew` calls entirely and the splash above is over in a blink.
    let packages = crate::load_packages(true, crate::refresh_requested())?;
    let state = State::load()?;
    let classified = classify::classify_all(packages, &state.categories, &state.notes);

    run_app(terminal, App::new(classified))
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn draw_loading_screen(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    terminal.draw(|f| {
        let area = centered_rect(40, 20, f.area());
        f.render_widget(Clear, area);
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("{}  lagerregal", theme::brand_icon()),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Reading installed Homebrew packages\u{2026}",
                Style::default().fg(theme::LABEL),
            )),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(theme::ACCENT));
        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .block(block),
            area,
        );
    })?;
    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        // Runs the (potentially slow) filesystem walk for size-sorting only
        // after a frame showing "Calculating..." has already been drawn, so
        // the blocking work never happens with a stale/frozen-looking screen.
        if app.pending_size_sort {
            compute_size_sort_cache(&mut app);
            app.pending_size_sort = false;
            app.status = format!("Sorted by {}", app.sort_mode.label());
            continue;
        }

        // Two-phase again for the refresh key, so the "Refreshing..." status
        // frame is on screen while the `brew` subprocesses run.
        if app.pending_refresh {
            app.pending_refresh = false;
            app.status = match refresh_packages(&mut app) {
                // Count what the header counts (on-request packages), so the
                // two numbers on screen don't silently disagree.
                Ok(()) => format!(
                    "Refreshed \u{2013} {} packages",
                    app.packages
                        .iter()
                        .filter(|p| p.package.installed_on_request)
                        .count()
                ),
                Err(e) => format!("Refresh failed: {e}"),
            };
            app.clamp_sidebar_selection();
            app.clamp_list_selection();
            continue;
        }

        // Same two-phase idea as size-sorting, but for `brew upgrade`: the
        // confirm dialog has already closed by the time this runs (it was
        // drawn, then the key that set `pending_upgrade` was handled), so
        // leaving the TUI now doesn't yank the screen out from under a
        // visible popup.
        if let Some((action, targets)) = app.pending_brew.take() {
            run_brew_actions(terminal, action, &targets)?;
            let refreshed = refresh_packages(&mut app);
            app.selected_names.clear();
            app.status = match refreshed {
                Ok(()) => format!("Finished {} {} package(s)", action.gerund(), targets.len()),
                Err(_) => format!(
                    "Finished {} {} package(s), but refreshing the list failed",
                    action.gerund(),
                    targets.len()
                ),
            };
            continue;
        }

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match app.mode {
                        InputMode::Normal => handle_normal_key(&mut app, key.code),
                        InputMode::Filter | InputMode::Note | InputMode::Category => {
                            handle_text_input_key(&mut app, key.code)
                        }
                        InputMode::Help => app.mode = InputMode::Normal,
                        InputMode::Menu => handle_menu_key(&mut app, key.code),
                        InputMode::Confirm => handle_confirm_key(&mut app, key.code),
                        InputMode::CategoryPick => handle_picker_key(&mut app, key.code),
                    }
                    app.clamp_sidebar_selection();
                    app.clamp_list_selection();
                }
                Event::Mouse(mouse) => {
                    handle_mouse(&mut app, mouse);
                    app.clamp_sidebar_selection();
                    app.clamp_list_selection();
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    // Sizes computed one-at-a-time by the detail pane accumulate quietly;
    // exiting is the natural moment to write them back.
    if app.disk_sizes_dirty {
        crate::cache::merge_sizes(&app.disk_sizes);
    }
    Ok(())
}

fn compute_size_sort_cache(app: &mut App) {
    let sig = app.scope_signature();
    // Two passes to keep the borrows apart: collect what to measure, then
    // measure while the persistent size map is mutably in play.
    let targets: Vec<(crate::homebrew::PackageKind, String, String)> = app
        .visible_packages()
        .iter()
        .map(|p| {
            (
                p.package.kind,
                p.package.name.clone(),
                p.package.version.clone(),
            )
        })
        .collect();
    let mut sizes: HashMap<String, u64> = HashMap::new();
    for (kind, name, version) in targets {
        let size = crate::cache::size_or_compute(
            &mut app.disk_sizes,
            &mut app.disk_sizes_dirty,
            kind,
            &name,
            &version,
        )
        .unwrap_or(0);
        sizes.insert(name, size);
    }
    // A size-sort walks many packages at once - the one moment where
    // persisting immediately is clearly worth a small file write.
    if app.disk_sizes_dirty {
        crate::cache::merge_sizes(&app.disk_sizes);
        app.disk_sizes_dirty = false;
    }
    app.size_sort_cache = Some((sig, sizes));
}

/// Leaves the TUI's alternate screen/raw mode, runs the confirmed `brew`
/// action for each target with the real terminal (so `brew`'s own progress
/// bars and build output render normally instead of needing to be captured
/// and re-drawn inside a ratatui pane), waits for the user to acknowledge,
/// then re-enters the TUI. A failure for one target doesn't stop the rest -
/// `brew`'s own output already explains what went wrong for that one.
fn run_brew_actions(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    action: BrewAction,
    targets: &[(PackageKind, String)],
) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    let sub = action.subcommand();
    println!();
    for (kind, name) in targets {
        println!("==> brew {sub} {name}");
        let result = match action {
            BrewAction::Upgrade => homebrew::upgrade(*kind, name),
            BrewAction::Uninstall => homebrew::uninstall(*kind, name),
        };
        match result {
            Ok(status) if status.success() => {}
            Ok(_) => println!("(brew {sub} for \"{name}\" did not finish successfully)"),
            Err(e) => println!("(failed to run brew {sub} for \"{name}\": {e})"),
        }
        println!();
    }
    println!("Press Enter to return to lagerregal...");
    let mut discard = String::new();
    let _ = io::stdin().lock().read_line(&mut discard);

    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.clear()?;
    Ok(())
}

/// Re-reads installed packages (and outdated status, best-effort) after an
/// upgrade, reapplying local state (manual categories/notes) on top -
/// versions/outdated-ness can have changed, but the user's own
/// categorization shouldn't be affected by running `brew upgrade`.
fn refresh_packages(app: &mut App) -> Result<()> {
    // Forced past the cache: `brew upgrade` just changed the very state the
    // cache describes, and the fingerprint may not have settled yet.
    let packages = crate::load_packages(true, true)?;
    let state = State::load()?;
    app.packages = classify::classify_all(packages, &state.categories, &state.notes);
    app.orphans = compute_orphans(&app.packages);
    app.size_cache = None;
    app.size_sort_cache = None;
    Ok(())
}
