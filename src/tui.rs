use std::collections::BTreeMap;
use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::classify::{self, ClassificationSource, ClassifiedPackage};
use crate::homebrew;
use crate::store::State;

const ALL_CATEGORY: &str = "All";

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
}

struct App {
    packages: Vec<ClassifiedPackage>,
    sidebar_state: ListState,
    list_state: ListState,
    focus: Focus,
    mode: InputMode,
    input_buffer: String,
    filter: String,
    status: String,
    should_quit: bool,
    show_deps: bool,
}

impl App {
    fn new(packages: Vec<ClassifiedPackage>) -> Self {
        let mut sidebar_state = ListState::default();
        sidebar_state.select(Some(0));
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        App {
            packages,
            sidebar_state,
            list_state,
            focus: Focus::List,
            mode: InputMode::Normal,
            input_buffer: String::new(),
            filter: String::new(),
            status: "Tab: switch pane | j/k: move | /: filter | n: note | c: category | d: deps | q: quit"
                .to_string(),
            should_quit: false,
            show_deps: false,
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
            .filter(|c| !known.contains(c))
            .collect();
        extra.sort();
        extra.dedup();

        let mut cats = vec![ALL_CATEGORY.to_string()];
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

    fn visible_packages(&self) -> Vec<&ClassifiedPackage> {
        let category = self.selected_category();
        let filter = self.filter.to_lowercase();
        self.scoped_packages()
            .into_iter()
            .filter(|p| category == ALL_CATEGORY || p.category == category)
            .filter(|p| {
                filter.is_empty()
                    || p.package.name.to_lowercase().contains(&filter)
                    || p.package.desc.to_lowercase().contains(&filter)
            })
            .collect()
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
    let packages = homebrew::installed_packages().context(
        "Could not read installed Homebrew packages. Is Homebrew installed and on your PATH?",
    )?;
    let state = State::load()?;
    let classified = classify::classify_all(packages, &state.categories, &state.notes);

    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, App::new(classified));
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match app.mode {
                    InputMode::Normal => handle_normal_key(&mut app, key.code),
                    InputMode::Filter | InputMode::Note | InputMode::Category => {
                        handle_text_input_key(&mut app, key.code)
                    }
                }
                app.clamp_list_selection();
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn handle_normal_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
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
        KeyCode::Char('n') => {
            if let Some(pkg) = app.selected_package() {
                app.input_buffer = pkg.note.clone().unwrap_or_default();
                app.mode = InputMode::Note;
            }
        }
        KeyCode::Char('c') => {
            if app.selected_package().is_some() {
                app.input_buffer.clear();
                app.mode = InputMode::Category;
            }
        }
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
            let name = app.selected_package().map(|p| p.package.name.clone());
            if let Some(name) = name {
                let category = app.input_buffer.clone();
                if let Ok(mut state) = State::load() {
                    state.set_category(&name, &category);
                    let _ = state.save();
                }
                if let Some(p) = app.packages.iter_mut().find(|p| p.package.name == name) {
                    p.category = category.clone();
                    p.source = ClassificationSource::Manual;
                }
                app.status = format!("\"{name}\" set to category \"{category}\"");
            }
        }
        InputMode::Normal => {}
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(35),
            Constraint::Percentage(45),
        ])
        .split(rows[0]);

    draw_sidebar(f, app, columns[0]);
    draw_list(f, app, columns[1]);
    draw_detail(f, app, columns[2]);
    draw_footer(f, app, rows[1]);
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let categories = app.sidebar_categories();
    let scoped = app.scoped_packages();
    let counts = category_counts(scoped.iter().copied());
    let total = scoped.len();
    let focused = app.focus == Focus::Sidebar;

    let items: Vec<ListItem> = categories
        .iter()
        .map(|c| {
            let count = if c == ALL_CATEGORY {
                total
            } else {
                *counts.get(c).unwrap_or(&0)
            };
            ListItem::new(format!("{c} ({count})"))
        })
        .collect();

    let title = if app.show_deps {
        "Categories (incl. deps)"
    } else {
        "Categories"
    };
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(list, area, &mut app.sidebar_state);
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::List;
    let items: Vec<ListItem> = app
        .visible_packages()
        .iter()
        .map(|p| {
            let marker = match p.source {
                ClassificationSource::Manual => "*",
                _ => " ",
            };
            ListItem::new(format!("{marker}{} [{}]", p.package.name, p.category))
        })
        .collect();

    let title = if app.filter.is_empty() {
        "Packages".to_string()
    } else {
        format!("Packages (filter: {})", app.filter)
    };
    let border_style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().title("Details").borders(Borders::ALL);

    let lines: Vec<Line> = if let Some(p) = app.selected_package() {
        let mut lines = vec![
            Line::from(Span::styled(
                p.package.name.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("Kind: {}", p.package.kind.as_str())),
            Line::from(format!("Version: {}", p.package.version)),
            Line::from(format!("Category: {} [{}]", p.category, p.source.as_str())),
            Line::from(format!("Tap: {}", p.package.tap)),
            Line::from(format!("Homepage: {}", p.package.homepage)),
            Line::from(""),
            Line::from(p.package.desc.clone()),
        ];
        if let Some(note) = &p.note {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Note:",
                Style::default().add_modifier(Modifier::ITALIC),
            )));
            lines.push(Line::from(note.clone()));
        }
        lines
    } else {
        vec![Line::from("No package selected.")]
    };

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = match app.mode {
        InputMode::Filter => format!("Filter: {}\u{2588}", app.input_buffer),
        InputMode::Note => format!("Note: {}\u{2588}", app.input_buffer),
        InputMode::Category => format!("Category: {}\u{2588}", app.input_buffer),
        InputMode::Normal => app.status.clone(),
    };
    let paragraph = Paragraph::new(text).block(Block::default().borders(Borders::ALL));
    f.render_widget(paragraph, area);
}
