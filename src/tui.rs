use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, Stdout, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, LineGauge, List, ListItem, ListState, Paragraph, Row,
    Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::classify::{self, ClassificationSource, ClassifiedPackage};
use crate::details;
use crate::homebrew::{self, Package, PackageKind};
use crate::store::State;
use crate::theme;

const ALL_CATEGORY: &str = "All";
/// Pseudo-categories layered on top of the real taxonomy in the sidebar -
/// they don't affect `p.category`, they're just filtered views (like `All`
/// already was), for two questions that come up often enough to deserve a
/// permanent spot: "what needs updating" and "what has Homebrew itself
/// given up on".
const OUTDATED_CATEGORY: &str = "Outdated";
const UNMAINTAINED_CATEGORY: &str = "Unmaintained";

/// Actions reachable both via a direct key in Normal mode and via the
/// Enter-triggered action menu, so the menu never drifts out of sync with
/// what the direct shortcuts actually do.
const MENU_ITEMS: &[(char, &str)] = &[
    ('u', "Update (brew upgrade)"),
    ('n', "Edit note"),
    ('c', "Set category"),
    ('R', "Reset category"),
    ('o', "Open homepage"),
    ('y', "Copy name"),
    (' ', "Toggle multi-select"),
];

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Sidebar,
    List,
}

#[derive(PartialEq, Clone, Copy)]
enum InputMode {
    Normal,
    Filter,
    Note,
    Category,
    Help,
    Menu,
    Confirm,
}

#[derive(PartialEq, Clone, Copy)]
enum SortMode {
    Name,
    Size,
    InstalledAt,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            SortMode::Name => "name",
            SortMode::Size => "size",
            SortMode::InstalledAt => "install date",
        }
    }

    fn next(self) -> Self {
        match self {
            SortMode::Name => SortMode::Size,
            SortMode::Size => SortMode::InstalledAt,
            SortMode::InstalledAt => SortMode::Name,
        }
    }
}

/// Screen regions from the most recently drawn frame, recorded by the
/// draw_* functions as they lay out each pane, so mouse events (which only
/// carry a column/row) can be hit-tested against the right widget without
/// re-deriving the whole layout in the event handler. Rects default to
/// zero-sized, which `Rect::contains` always reports as a miss - so a region
/// that wasn't drawn this frame (e.g. the menu popup, when no menu is open)
/// simply can't be clicked.
#[derive(Clone, Copy, Default)]
struct Hitboxes {
    sidebar: Rect,
    sidebar_rows_top: u16,
    list: Rect,
    list_rows_top: u16,
    /// The action menu's item list (Enter / right-click popup).
    menu: Rect,
    menu_rows_top: u16,
    /// The whole update-confirmation popup - treated as one big "yes"
    /// button; clicking anywhere outside it cancels. Precisely hit-testing
    /// the "[y]"/"[n]" text spans isn't worth it since their row shifts
    /// with how many packages are listed.
    confirm: Rect,
}

struct App {
    packages: Vec<ClassifiedPackage>,
    sidebar_state: ListState,
    list_state: TableState,
    focus: Focus,
    mode: InputMode,
    input_buffer: String,
    filter: String,
    status: String,
    should_quit: bool,
    show_deps: bool,
    sort_mode: SortMode,
    /// Names of packages currently multi-selected (via Space), for bulk
    /// category assignment/reset. Cleared after each bulk action.
    selected_names: HashSet<String>,
    /// Highlighted row in the Enter-triggered action menu.
    menu_index: usize,
    /// Cached on-disk size for the currently selected package, keyed by
    /// name, so `draw_detail` doesn't re-walk the filesystem on every
    /// redraw tick (every ~250ms) - only when the selection changes.
    size_cache: Option<(String, Option<u64>)>,
    /// Cached on-disk sizes for the current sort scope (category + filter +
    /// show_deps, joined into one string), used only when sorting by size.
    /// Recomputed via the two-phase "Calculating..." flow in `run_app`
    /// rather than inline, since walking every visible package's directory
    /// can take seconds and would otherwise freeze the UI with no feedback.
    size_sort_cache: Option<(String, HashMap<String, u64>)>,
    pending_size_sort: bool,
    /// Packages awaiting confirmation for `brew upgrade`, shown by the
    /// Confirm-mode overlay. Cleared on cancel; moved into `pending_upgrade`
    /// on confirm.
    confirm_targets: Vec<(PackageKind, String)>,
    /// Set once the user confirms an update; `run_app` picks this up after
    /// the *next* draw (so the confirm dialog has already visibly closed)
    /// and suspends the TUI to run `brew upgrade` with real terminal output.
    pending_upgrade: Option<Vec<(PackageKind, String)>>,
    /// Hit regions from the last frame, for mouse click/scroll dispatch.
    hitboxes: Hitboxes,
    /// (time, package row index) of the last left-click on the package
    /// list, so a second click on the same row shortly after reads as a
    /// double-click (open the action menu) instead of two plain selects.
    last_list_click: Option<(Instant, usize)>,
}

impl App {
    fn new(packages: Vec<ClassifiedPackage>) -> Self {
        let mut sidebar_state = ListState::default();
        sidebar_state.select(Some(0));
        let mut list_state = TableState::default();
        list_state.select(Some(0));

        App {
            packages,
            sidebar_state,
            list_state,
            focus: Focus::List,
            mode: InputMode::Normal,
            input_buffer: String::new(),
            filter: String::new(),
            // Empty on purpose: the footer falls back to its styled key
            // hints whenever there's no transient message to show.
            status: String::new(),
            should_quit: false,
            show_deps: false,
            sort_mode: SortMode::Name,
            selected_names: HashSet::new(),
            menu_index: 0,
            size_cache: None,
            size_sort_cache: None,
            pending_size_sort: false,
            confirm_targets: Vec::new(),
            pending_upgrade: None,
            hitboxes: Hitboxes::default(),
            last_list_click: None,
        }
    }

    /// Packages in scope given the current `show_deps` setting: by default,
    /// only packages the user explicitly installed (excluding anything
    /// pulled in purely as a dependency of something else).
    fn scoped_packages(&self) -> Vec<&ClassifiedPackage> {
        self.packages
            .iter()
            .filter(|p| self.show_deps || p.package.installed_on_request)
            .collect()
    }

    /// "All" plus every known taxonomy category, plus any custom category
    /// names the user has manually assigned that aren't part of the
    /// built-in taxonomy (appended alphabetically at the end).
    fn sidebar_categories(&self) -> Vec<String> {
        let known = classify::known_categories();
        let mut extra: Vec<String> = self
            .scoped_packages()
            .iter()
            .map(|p| p.category.clone())
            .filter(|c| !known.contains(c) && c != OUTDATED_CATEGORY && c != UNMAINTAINED_CATEGORY)
            .collect();
        extra.sort();
        extra.dedup();

        let mut cats = vec![
            ALL_CATEGORY.to_string(),
            OUTDATED_CATEGORY.to_string(),
            UNMAINTAINED_CATEGORY.to_string(),
        ];
        cats.extend(known);
        cats.extend(extra);
        cats
    }

    fn selected_category(&self) -> String {
        let cats = self.sidebar_categories();
        let idx = self.sidebar_state.selected().unwrap_or(0);
        cats.get(idx)
            .cloned()
            .unwrap_or_else(|| ALL_CATEGORY.to_string())
    }

    /// Identifies the current "what's visible" scope (category + filter +
    /// dependency toggle), so the size-sort cache can tell whether it's
    /// still valid or needs recomputing.
    fn scope_signature(&self) -> String {
        format!(
            "{}|{}|{}",
            self.selected_category(),
            self.filter,
            self.show_deps
        )
    }

    fn visible_packages(&self) -> Vec<&ClassifiedPackage> {
        let category = self.selected_category();
        let filter = self.filter.to_lowercase();
        let mut result: Vec<&ClassifiedPackage> = self
            .scoped_packages()
            .into_iter()
            .filter(|p| match category.as_str() {
                ALL_CATEGORY => true,
                OUTDATED_CATEGORY => p.package.outdated.is_some(),
                UNMAINTAINED_CATEGORY => p.package.unmaintained,
                _ => p.category == category,
            })
            .filter(|p| {
                filter.is_empty()
                    || p.package.name.to_lowercase().contains(&filter)
                    || p.package.desc.to_lowercase().contains(&filter)
                    || p.note
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase().contains(&filter))
            })
            .collect();

        match self.sort_mode {
            // Already alphabetical - packages come pre-sorted by name from
            // `homebrew::parse_brew_json` and filtering preserves order.
            SortMode::Name => {}
            SortMode::InstalledAt => {
                result.sort_by_key(|p| std::cmp::Reverse(p.package.installed_at));
            }
            SortMode::Size => {
                if let Some((sig, sizes)) = &self.size_sort_cache {
                    if *sig == self.scope_signature() {
                        result.sort_by(|a, b| {
                            let sa = sizes.get(&a.package.name).copied().unwrap_or(0);
                            let sb = sizes.get(&b.package.name).copied().unwrap_or(0);
                            sb.cmp(&sa)
                        });
                    }
                }
            }
        }
        result
    }

    fn selected_package(&self) -> Option<&ClassifiedPackage> {
        let visible = self.visible_packages();
        self.list_state
            .selected()
            .and_then(|i| visible.get(i).copied())
    }

    fn clamp_list_selection(&mut self) {
        let len = self.visible_packages().len();
        if len == 0 {
            self.list_state.select(None);
            return;
        }
        match self.list_state.selected() {
            None => self.list_state.select(Some(0)),
            Some(i) if i >= len => self.list_state.select(Some(len - 1)),
            Some(_) => {}
        }
    }
}

fn category_counts<'a>(
    packages: impl IntoIterator<Item = &'a ClassifiedPackage>,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for p in packages {
        *counts.entry(p.category.clone()).or_insert(0) += 1;
    }
    counts
}

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

        // Same two-phase idea as size-sorting, but for `brew upgrade`: the
        // confirm dialog has already closed by the time this runs (it was
        // drawn, then the key that set `pending_upgrade` was handled), so
        // leaving the TUI now doesn't yank the screen out from under a
        // visible popup.
        if let Some(targets) = app.pending_upgrade.take() {
            run_upgrades(terminal, &targets)?;
            let refreshed = refresh_packages(&mut app);
            app.selected_names.clear();
            app.status = match refreshed {
                Ok(()) => format!("Finished updating {} package(s)", targets.len()),
                Err(_) => format!(
                    "Finished updating {} package(s), but refreshing the list failed",
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
                    }
                    app.clamp_list_selection();
                }
                Event::Mouse(mouse) => {
                    handle_mouse(&mut app, mouse);
                    app.clamp_list_selection();
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn compute_size_sort_cache(app: &mut App) {
    let sig = app.scope_signature();
    let sizes: HashMap<String, u64> = app
        .visible_packages()
        .iter()
        .map(|p| {
            let size = details::package_size(p.package.kind, &p.package.name, &p.package.version)
                .unwrap_or(0);
            (p.package.name.clone(), size)
        })
        .collect();
    app.size_sort_cache = Some((sig, sizes));
}

/// Leaves the TUI's alternate screen/raw mode, runs `brew upgrade` for each
/// target with the real terminal (so `brew`'s own progress bars and build
/// output render normally instead of needing to be captured and re-drawn
/// inside a ratatui pane), waits for the user to acknowledge, then re-enters
/// the TUI. A failed `brew upgrade` for one target doesn't stop the rest -
/// `brew`'s own output already explains what went wrong for that one.
fn run_upgrades(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    targets: &[(PackageKind, String)],
) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    println!();
    for (kind, name) in targets {
        println!("==> brew upgrade {name}");
        match homebrew::upgrade(*kind, name) {
            Ok(status) if status.success() => {}
            Ok(_) => println!("(brew upgrade for \"{name}\" did not finish successfully)"),
            Err(e) => println!("(failed to run brew upgrade for \"{name}\": {e})"),
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
    app.size_cache = None;
    app.size_sort_cache = None;
    Ok(())
}

fn handle_normal_key(app: &mut App, code: KeyCode) {
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
            app.mode = InputMode::Filter;
        }
        KeyCode::Char('?') => app.mode = InputMode::Help,
        KeyCode::Enter => {
            if app.selected_package().is_some() {
                app.menu_index = 0;
                app.mode = InputMode::Menu;
            }
        }
        KeyCode::Char('n') => start_note_edit(app),
        KeyCode::Char('c') => start_category_edit(app),
        KeyCode::Char('R') => reset_category(app),
        KeyCode::Char('o') => open_homepage(app),
        KeyCode::Char('y') => copy_name(app),
        KeyCode::Char(' ') => toggle_select(app),
        KeyCode::Char('s') => cycle_sort(app),
        KeyCode::Char('u') => start_update(app),
        KeyCode::Char('d') => {
            app.show_deps = !app.show_deps;
            app.list_state.select(Some(0));
        }
        _ => {}
    }
}

fn move_selection(app: &mut App, delta: i32) {
    match app.focus {
        Focus::Sidebar => {
            let len = app.sidebar_categories().len();
            if len == 0 {
                return;
            }
            let i = app.sidebar_state.selected().unwrap_or(0) as i32;
            let new_i = (i + delta).rem_euclid(len as i32) as usize;
            app.sidebar_state.select(Some(new_i));
            app.list_state.select(Some(0));
        }
        Focus::List => {
            let len = app.visible_packages().len();
            if len == 0 {
                app.list_state.select(None);
                return;
            }
            let i = app.list_state.selected().unwrap_or(0) as i32;
            let new_i = (i + delta).rem_euclid(len as i32) as usize;
            app.list_state.select(Some(new_i));
        }
    }
}

fn handle_text_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
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
        }
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        _ => {}
    }
}

/// Handles a keypress while the Enter-triggered action menu is open: up/down
/// (or j/k) move the highlight, Enter activates the highlighted action.
/// To keep the menu fast for anyone who already knows the shortcuts,
/// pressing a menu item's own letter activates it immediately too.
fn handle_menu_key(app: &mut App, code: KeyCode) {
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
fn handle_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.pending_upgrade = Some(std::mem::take(&mut app.confirm_targets));
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
fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    match app.mode {
        InputMode::Normal => handle_normal_mouse(app, mouse),
        InputMode::Menu => handle_menu_mouse(app, mouse),
        InputMode::Confirm => handle_confirm_mouse(app, mouse),
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
fn row_to_index(row: u16, rows_top: u16, offset: usize) -> Option<usize> {
    row.checked_sub(rows_top)
        .map(|delta| offset + delta as usize)
}

fn handle_normal_mouse(app: &mut App, mouse: MouseEvent) {
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
                ) {
                    if idx < app.sidebar_categories().len() {
                        app.sidebar_state.select(Some(idx));
                        app.list_state.select(Some(0));
                    }
                }
            } else if app.hitboxes.list.contains(pos) {
                app.focus = Focus::List;
                if let Some(idx) = row_to_index(
                    mouse.row,
                    app.hitboxes.list_rows_top,
                    app.list_state.offset(),
                ) {
                    if idx < app.visible_packages().len() {
                        app.list_state.select(Some(idx));
                        let now = Instant::now();
                        let is_double_click = app.last_list_click.is_some_and(|(t, i)| {
                            i == idx && now.duration_since(t) < Duration::from_millis(400)
                        });
                        if is_double_click {
                            app.last_list_click = None;
                            if app.selected_package().is_some() {
                                app.menu_index = 0;
                                app.mode = InputMode::Menu;
                            }
                        } else {
                            app.last_list_click = Some((now, idx));
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
            ) {
                if idx < app.visible_packages().len() {
                    app.list_state.select(Some(idx));
                }
            }
            if app.selected_package().is_some() {
                app.menu_index = 0;
                app.mode = InputMode::Menu;
            }
        }
        _ => {}
    }
}

fn handle_menu_mouse(app: &mut App, mouse: MouseEvent) {
    if !matches!(mouse.kind, MouseEventKind::Down(_)) {
        return;
    }
    let pos = Position::new(mouse.column, mouse.row);
    if app.hitboxes.menu.contains(pos) {
        if let Some(idx) = row_to_index(mouse.row, app.hitboxes.menu_rows_top, 0) {
            if idx < MENU_ITEMS.len() {
                let (key, _) = MENU_ITEMS[idx];
                app.mode = InputMode::Normal;
                trigger_menu_action(app, key);
                return;
            }
        }
    }
    // Clicked outside the menu (or below its items) - dismiss it, same as Esc.
    app.mode = InputMode::Normal;
}

fn handle_confirm_mouse(app: &mut App, mouse: MouseEvent) {
    if !matches!(mouse.kind, MouseEventKind::Down(_)) {
        return;
    }
    let pos = Position::new(mouse.column, mouse.row);
    if app.hitboxes.confirm.contains(pos) {
        app.pending_upgrade = Some(std::mem::take(&mut app.confirm_targets));
    } else {
        app.confirm_targets.clear();
    }
    app.mode = InputMode::Normal;
}

fn trigger_menu_action(app: &mut App, key: char) {
    match key {
        'u' => start_update(app),
        'n' => start_note_edit(app),
        'c' => start_category_edit(app),
        'R' => reset_category(app),
        'o' => open_homepage(app),
        'y' => copy_name(app),
        ' ' => toggle_select(app),
        _ => {}
    }
}

fn start_note_edit(app: &mut App) {
    if let Some(pkg) = app.selected_package() {
        app.input_buffer = pkg.note.clone().unwrap_or_default();
        app.mode = InputMode::Note;
    }
}

fn start_category_edit(app: &mut App) {
    if app.selected_package().is_some() {
        app.input_buffer.clear();
        app.mode = InputMode::Category;
    }
}

fn toggle_select(app: &mut App) {
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

fn cycle_sort(app: &mut App) {
    app.sort_mode = app.sort_mode.next();
    app.list_state.select(Some(0));
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
fn start_update(app: &mut App) {
    let bulk = !app.selected_names.is_empty();
    let targets: Vec<(PackageKind, String)> = if bulk {
        app.packages
            .iter()
            .filter(|p| {
                app.selected_names.contains(&p.package.name) && p.package.outdated.is_some()
            })
            .map(|p| (p.package.kind, p.package.name.clone()))
            .collect()
    } else {
        app.selected_package()
            .filter(|p| p.package.outdated.is_some())
            .map(|p| (p.package.kind, p.package.name.clone()))
            .into_iter()
            .collect()
    };

    if targets.is_empty() {
        app.status = if bulk {
            "No outdated packages in selection".to_string()
        } else if let Some(pkg) = app.selected_package() {
            format!("\"{}\" is already up to date", pkg.package.name)
        } else {
            "No package selected".to_string()
        };
        return;
    }

    app.confirm_targets = targets;
    app.mode = InputMode::Confirm;
}

fn open_homepage(app: &mut App) {
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

fn copy_name(app: &mut App) {
    let Some(pkg) = app.selected_package() else {
        return;
    };
    let name = pkg.package.name.clone();
    match copy_to_clipboard(&name) {
        Ok(()) => app.status = format!("Copied \"{name}\" to clipboard"),
        Err(_) => app.status = "Failed to copy to clipboard (is `pbcopy` on PATH?)".to_string(),
    }
}

fn copy_to_clipboard(text: &str) -> Result<()> {
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

fn apply_input(app: &mut App) {
    match app.mode {
        InputMode::Filter => {
            app.filter = app.input_buffer.clone();
            app.list_state.select(Some(0));
        }
        InputMode::Note => {
            let name = app.selected_package().map(|p| p.package.name.clone());
            if let Some(name) = name {
                if let Ok(mut state) = State::load() {
                    state.set_note(&name, &app.input_buffer);
                    let _ = state.save();
                }
                if let Some(p) = app.packages.iter_mut().find(|p| p.package.name == name) {
                    p.note = Some(app.input_buffer.clone());
                }
                app.status = format!("Saved note for {name}");
            }
        }
        InputMode::Category => {
            let category = app.input_buffer.clone();
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
                    state.set_category(name, &category);
                }
                let _ = state.save();
            }
            for p in app.packages.iter_mut() {
                if names.contains(&p.package.name) {
                    p.category = category.clone();
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
        InputMode::Normal | InputMode::Help | InputMode::Menu | InputMode::Confirm => {}
    }
}

/// Clears a manual category override, falling back to whatever the
/// curated/heuristic classifier would have assigned. Applies to every
/// multi-selected package (skipping any that aren't manually classified) if
/// there's an active selection, otherwise just the highlighted package.
fn reset_category(app: &mut App) {
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

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(14),
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
        _ => {}
    }
}

/// A slim, borderless title bar - "lagerregal", live package/update counts,
/// and the dependency-inclusion state - rendered above the main panes.
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
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
                .line_set(symbols::line::THICK)
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

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
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

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    app.hitboxes.list = area;
    // Rows start below the top border and the header row.
    app.hitboxes.list_rows_top = area.y + 2;
    let focused = app.focus == Focus::List;
    // The sidebar already scopes the list to one category once something
    // other than "All" is selected, so repeating it in every row would just
    // be noise - only show the column when it's actually adding information.
    let show_category = app.selected_category() == ALL_CATEGORY;

    let rows: Vec<Row> = app
        .visible_packages()
        .iter()
        .map(|p| {
            let mut name_spans = Vec::new();
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
            // Formula vs cask, colored by where the category came from - two
            // independent facts folded into one glyph so the name column
            // doesn't grow a marker per attribute.
            name_spans.push(Span::styled(
                format!("{} ", theme::kind_icon(p.package.kind)),
                Style::default().fg(theme::source_color(p.source)),
            ));
            name_spans.push(Span::raw(p.package.name.clone()));
            if p.source == ClassificationSource::Manual {
                name_spans.push(Span::styled(
                    format!(" {}", theme::manual_icon()),
                    Style::default().fg(theme::MANUAL),
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
                cells.push(Cell::new(Line::from(vec![
                    Span::styled(
                        format!("{} ", theme::category_icon(&p.category)),
                        Style::default().fg(theme::category_color(&p.category)),
                    ),
                    Span::styled(
                        p.category.clone(),
                        Style::default().fg(theme::category_color(&p.category)),
                    ),
                ])));
            }
            cells.push(
                Cell::new(p.package.version.clone()).style(Style::default().fg(theme::LABEL)),
            );
            cells.push(Cell::new(p.package.desc.clone()));
            Row::new(cells)
        })
        .collect();

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
    let visible_count = rows.len();
    let table = Table::new(rows, widths)
        .header(
            Row::new(header).style(
                Style::default()
                    .fg(theme::LABEL)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_type(border_type)
                .border_style(border_style),
        )
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

/// The detail pane runs as a horizontal band under the sidebar/list (rather
/// than a tall column beside them), so its content is laid out in two
/// side-by-side columns to make use of the width: package info/description
/// on the left, the dependency tree on the right.
fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    // Refresh the size cache first (a filesystem walk), before borrowing a
    // package reference for line-building below - keeps the two mutable /
    // immutable uses of `app` from overlapping.
    let selected_name = app.selected_package().map(|p| p.package.name.clone());
    if let Some(name) = &selected_name {
        if app.size_cache.as_ref().map(|(n, _)| n) != Some(name) {
            if let Some(p) = app.packages.iter().find(|p| &p.package.name == name) {
                let size =
                    details::package_size(p.package.kind, &p.package.name, &p.package.version);
                app.size_cache = Some((name.clone(), size));
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

    // One helper for every "icon  label  value" row, so the detail pane's
    // columns line up without repeating the same span dance per field.
    let field = |icon: &str, label: &str, value: String, value_style: Style| {
        Line::from(vec![
            Span::styled(format!("{icon} "), Style::default().fg(theme::LABEL)),
            Span::styled(format!("{label:<10}"), Style::default().fg(theme::LABEL)),
            Span::styled(value, value_style),
        ])
    };

    let mut version_spans = vec![
        Span::styled(
            format!("{} ", theme::version_icon()),
            Style::default().fg(theme::LABEL),
        ),
        Span::styled(
            format!("{:<10}", "Version"),
            Style::default().fg(theme::LABEL),
        ),
        Span::raw(p.package.version.clone()),
    ];
    if let Some(newer) = &p.package.outdated {
        version_spans.push(Span::styled(
            format!("  {} {newer} available", theme::outdated_icon()),
            Style::default()
                .fg(theme::OUTDATED)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let mut info_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", theme::kind_icon(p.package.kind)),
                Style::default().fg(theme::source_color(p.source)),
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
        ]),
        Line::from(version_spans),
        Line::from(vec![
            Span::styled(
                format!("{} ", theme::category_icon(&p.category)),
                Style::default().fg(theme::category_color(&p.category)),
            ),
            Span::styled(
                format!("{:<10}", "Category"),
                Style::default().fg(theme::LABEL),
            ),
            Span::styled(
                p.category.clone(),
                Style::default().fg(theme::category_color(&p.category)),
            ),
            Span::styled(
                format!("  [{}]", p.source.as_str()),
                Style::default().fg(theme::source_color(p.source)),
            ),
        ]),
        field(
            theme::publisher_icon(),
            "Publisher",
            p.package.tap.clone(),
            Style::default(),
        ),
    ];
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
    if let Some((_, Some(size))) = &app.size_cache {
        info_lines.push(field(
            theme::size_icon(),
            "Size",
            details::format_size(*size),
            Style::default().fg(theme::size_color(*size)),
        ));
    }
    if let Some(installed_at) = p.package.installed_at {
        info_lines.push(field(
            theme::time_icon(),
            "Installed",
            details::format_age(installed_at),
            Style::default(),
        ));
    }
    info_lines.push(field(
        theme::link_icon(),
        "Homepage",
        p.package.homepage.clone(),
        Style::default().fg(theme::HEURISTIC),
    ));
    info_lines.push(Line::from(""));
    info_lines.push(Line::from(Span::styled(
        p.package.desc.clone(),
        Style::default().fg(theme::LABEL),
    )));
    if let Some(note) = &p.note {
        info_lines.push(Line::from(""));
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

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(inner);

    f.render_widget(
        Paragraph::new(info_lines).wrap(Wrap { trim: true }),
        columns[0],
    );
    f.render_widget(
        Paragraph::new(dep_lines).wrap(Wrap { trim: true }),
        columns[1],
    );
}

/// Key hints shown along the footer in Normal mode - the handful worth
/// keeping permanently visible, with `?` pointing at the full list.
const FOOTER_HINTS: &[(&str, &str)] = &[
    ("tab", "pane"),
    ("j/k", "move"),
    ("\u{21b5}", "menu"),
    ("/", "filter"),
    ("s", "sort"),
    ("u", "update"),
    ("?", "help"),
    ("q", "quit"),
];

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
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
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
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

fn draw_help_overlay(f: &mut Frame) {
    let area = centered_rect(60, 80, f.area());
    f.render_widget(Clear, area);

    let lines = vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Tab              Switch focus between sidebar and list"),
        Line::from("j/k, \u{2191}/\u{2193}         Move selection"),
        Line::from("/                Filter by name, description, or note"),
        Line::from("d                Toggle showing dependency-only packages"),
        Line::from("s                Cycle sort order (name / size / install date)"),
        Line::from(""),
        Line::from(Span::styled(
            "Selected package",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from("Enter            Open the action menu"),
        Line::from("u                Update via `brew upgrade` (asks for confirmation)"),
        Line::from("n                Edit note"),
        Line::from("c                Set category"),
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

fn draw_confirm_overlay(f: &mut Frame, app: &mut App) {
    let area = centered_rect(46, 40, f.area());
    app.hitboxes.confirm = area;
    f.render_widget(Clear, area);

    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "Update {} package{} via `brew upgrade`?",
                app.confirm_targets.len(),
                if app.confirm_targets.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (kind, name) in app.confirm_targets.iter().take(10) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", theme::kind_icon(*kind)),
                Style::default().fg(theme::OUTDATED),
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
            format!(" {} Confirm Update ", theme::outdated_icon()),
            Style::default()
                .fg(theme::OUTDATED)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(theme::OUTDATED));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_menu_overlay(f: &mut Frame, app: &mut App) {
    // Own the name (rather than holding the `&ClassifiedPackage` borrow)
    // before touching `app.hitboxes` below - the two would otherwise
    // overlap an immutable and a mutable borrow of `app`.
    let Some(name) = app.selected_package().map(|p| p.package.name.clone()) else {
        return;
    };
    let area = centered_rect(36, 32, f.area());
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
