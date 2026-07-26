//! TUI state: the [`App`] struct, its row/selection model, and the small
//! enums (focus, input mode, sort mode) the event loop dispatches on.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::widgets::{ListState, TableState};

use crate::classify::{self, ClassifiedPackage};
use crate::homebrew::PackageKind;

pub(super) const ALL_CATEGORY: &str = "All";
/// Pseudo-categories layered on top of the real taxonomy in the sidebar -
/// they don't affect `p.category`, they're just filtered views (like `All`
/// already was), for two questions that come up often enough to deserve a
/// permanent spot: "what needs updating" and "what has Homebrew itself
/// given up on".
pub(super) const OUTDATED_CATEGORY: &str = "Outdated";
pub(super) const UNMAINTAINED_CATEGORY: &str = "Unmaintained";
/// Dependency-only packages nothing depends on anymore - what
/// `brew autoremove` would remove. The one view that deliberately ignores
/// the on-request scoping, since orphans are by definition dependency-only
/// and would otherwise never be visible without toggling `d`.
pub(super) const ORPHANED_CATEGORY: &str = "Orphaned";

#[derive(PartialEq, Clone, Copy)]
pub(super) enum Focus {
    Sidebar,
    List,
}

#[derive(PartialEq, Clone, Copy)]
pub(super) enum InputMode {
    Normal,
    Filter,
    Note,
    /// Free-text category entry - the escape hatch for creating a category
    /// that doesn't exist yet, reached via the last row of the picker.
    Category,
    /// The category picker popup: a filterable list of every known category.
    /// This is what `c` opens, so choosing an existing category is a
    /// selection, not a spelling test.
    CategoryPick,
    Help,
    Menu,
    Confirm,
}

#[derive(PartialEq, Clone, Copy)]
pub(super) enum SortMode {
    Name,
    Size,
    InstalledAt,
}

impl SortMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            SortMode::Name => "name",
            SortMode::Size => "size",
            SortMode::InstalledAt => "install date",
        }
    }

    pub(super) fn next(self) -> Self {
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
pub(super) struct Hitboxes {
    pub(super) sidebar: Rect,
    pub(super) sidebar_rows_top: u16,
    pub(super) list: Rect,
    pub(super) list_rows_top: u16,
    /// The action menu's item list (Enter / right-click popup).
    pub(super) menu: Rect,
    pub(super) menu_rows_top: u16,
    /// The whole update-confirmation popup - treated as one big "yes"
    /// button; clicking anywhere outside it cancels. Precisely hit-testing
    /// the "[y]"/"[n]" text spans isn't worth it since their row shifts
    /// with how many packages are listed.
    pub(super) confirm: Rect,
    /// The category picker's list area (rows below its filter prompt).
    pub(super) picker: Rect,
    pub(super) picker_rows_top: u16,
}

/// Which `brew` operation the Confirm overlay is arming. Update and
/// uninstall share the entire confirm -> suspend TUI -> run -> refresh
/// pipeline; only the verb, the styling, and the `brew` subcommand differ.
#[derive(PartialEq, Clone, Copy)]
pub(super) enum BrewAction {
    Upgrade,
    Uninstall,
}

impl BrewAction {
    /// The `brew` subcommand this action runs.
    pub(super) fn subcommand(self) -> &'static str {
        match self {
            BrewAction::Upgrade => "upgrade",
            BrewAction::Uninstall => "uninstall",
        }
    }

    /// Past-tense verb for status messages ("Finished updating ...").
    pub(super) fn gerund(self) -> &'static str {
        match self {
            BrewAction::Upgrade => "updating",
            BrewAction::Uninstall => "uninstalling",
        }
    }
}

/// One rendered line of the package table.
///
/// While a search is active the table is broken up by category headings, so
/// a table row index is no longer an index into `App::visible_packages` -
/// every selection, navigation and click path goes through
/// `App::visible_rows` instead, and only `Package` rows are selectable.
pub(super) enum ListRow<'a> {
    /// A category heading above the hits belonging to it.
    Header {
        category: &'a str,
        count: usize,
    },
    Package(&'a ClassifiedPackage),
    /// Placeholder for "nothing to show here", so an empty result set is
    /// stated rather than left as a blank pane.
    Empty,
}

pub(super) struct App {
    pub(super) packages: Vec<ClassifiedPackage>,
    /// Names `brew autoremove` would remove, recomputed whenever
    /// `packages` is replaced (startup and refresh) - the set only changes
    /// when the install itself does.
    pub(super) orphans: HashSet<String>,
    pub(super) sidebar_state: ListState,
    pub(super) list_state: TableState,
    pub(super) focus: Focus,
    pub(super) mode: InputMode,
    pub(super) input_buffer: String,
    pub(super) filter: String,
    /// What `filter` held when the search prompt was opened. The filter is
    /// applied live on every keystroke, so cancelling with Esc has to be
    /// able to put the previous state back - "as if I never searched".
    pub(super) filter_backup: String,
    pub(super) status: String,
    pub(super) should_quit: bool,
    pub(super) show_deps: bool,
    pub(super) sort_mode: SortMode,
    /// Names of packages currently multi-selected (via Space), for bulk
    /// category assignment/reset. Cleared after each bulk action.
    pub(super) selected_names: HashSet<String>,
    /// Highlighted row in the Enter-triggered action menu.
    pub(super) menu_index: usize,
    /// List state of the category picker (selection + scroll offset). Kept
    /// on the App rather than rebuilt per frame so ratatui can keep the
    /// highlighted row scrolled into view across redraws.
    pub(super) picker_state: ListState,
    /// Cached on-disk size for the currently selected package, keyed by
    /// name, so `draw_detail` doesn't re-walk the filesystem on every
    /// redraw tick (every ~250ms) - only when the selection changes.
    pub(super) size_cache: Option<(String, Option<u64>)>,
    /// On-disk sizes by `name|version`, seeded from the persistent cache at
    /// startup and extended whenever a size gets computed. `disk_sizes_dirty`
    /// tracks whether there's anything new worth writing back.
    pub(super) disk_sizes: BTreeMap<String, u64>,
    pub(super) disk_sizes_dirty: bool,
    /// Cached on-disk sizes for the current sort scope (category + filter +
    /// show_deps, joined into one string), used only when sorting by size.
    /// Recomputed via the two-phase "Calculating..." flow in `run_app`
    /// rather than inline, since walking every visible package's directory
    /// can take seconds and would otherwise freeze the UI with no feedback.
    pub(super) size_sort_cache: Option<(String, HashMap<String, u64>)>,
    pub(super) pending_size_sort: bool,
    /// Set by the `r` key; `run_app` re-reads everything from `brew` after
    /// the next draw, so a "Refreshing..." status frame is on screen while
    /// the subprocesses run (same two-phase idea as size-sorting).
    pub(super) pending_refresh: bool,
    /// Packages awaiting confirmation, shown by the Confirm-mode overlay,
    /// and which brew operation they're queued for. Cleared on cancel;
    /// moved into `pending_brew` on confirm.
    pub(super) confirm_action: BrewAction,
    pub(super) confirm_targets: Vec<(PackageKind, String)>,
    /// Set once the user confirms; `run_app` picks this up after the *next*
    /// draw (so the confirm dialog has already visibly closed) and suspends
    /// the TUI to run `brew` with real terminal output.
    pub(super) pending_brew: Option<(BrewAction, Vec<(PackageKind, String)>)>,
    /// Hit regions from the last frame, for mouse click/scroll dispatch.
    pub(super) hitboxes: Hitboxes,
    /// (time, package row index) of the last left-click on the package
    /// list, so a second click on the same row shortly after reads as a
    /// double-click (open the action menu) instead of two plain selects.
    pub(super) last_list_click: Option<(Instant, usize)>,
}

impl App {
    pub(super) fn new(packages: Vec<ClassifiedPackage>) -> Self {
        let orphans = compute_orphans(&packages);
        let mut sidebar_state = ListState::default();
        sidebar_state.select(Some(0));
        let mut list_state = TableState::default();
        list_state.select(Some(0));

        App {
            packages,
            orphans,
            sidebar_state,
            list_state,
            focus: Focus::List,
            mode: InputMode::Normal,
            input_buffer: String::new(),
            filter: String::new(),
            filter_backup: String::new(),
            // Empty on purpose: the footer falls back to its styled key
            // hints whenever there's no transient message to show.
            status: String::new(),
            should_quit: false,
            show_deps: false,
            sort_mode: SortMode::Name,
            selected_names: HashSet::new(),
            menu_index: 0,
            picker_state: ListState::default(),
            size_cache: None,
            disk_sizes: crate::cache::load_sizes(),
            disk_sizes_dirty: false,
            size_sort_cache: None,
            pending_size_sort: false,
            pending_refresh: false,
            confirm_action: BrewAction::Upgrade,
            confirm_targets: Vec::new(),
            pending_brew: None,
            hitboxes: Hitboxes::default(),
            last_list_click: None,
        }
    }

    /// Packages in scope given the current `show_deps` setting: by default,
    /// only packages the user explicitly installed (excluding anything
    /// pulled in purely as a dependency of something else).
    pub(super) fn scoped_packages(&self) -> Vec<&ClassifiedPackage> {
        self.packages
            .iter()
            .filter(|p| self.show_deps || p.package.installed_on_request)
            .collect()
    }

    /// "All" plus every known taxonomy category, plus any custom category
    /// names the user has manually assigned that aren't part of the
    /// built-in taxonomy (appended alphabetically at the end).
    pub(super) fn sidebar_categories(&self) -> Vec<String> {
        let scoped = self.scoped_packages();
        let counts = category_counts(scoped.iter().copied());

        let known = classify::known_categories();
        let mut extra: Vec<String> = scoped
            .iter()
            .map(|p| p.category.clone())
            .filter(|c| {
                !known.contains(c)
                    && c != OUTDATED_CATEGORY
                    && c != UNMAINTAINED_CATEGORY
                    && c != ORPHANED_CATEGORY
            })
            .collect();
        extra.sort();
        extra.dedup();

        // The pseudo-categories are always listed, even at zero: they
        // answer a question ("is anything outdated?") where the answer "no"
        // is worth showing. Real categories with nothing in them are just
        // rows to scroll past, so they're hidden until they have contents.
        let mut cats = vec![
            ALL_CATEGORY.to_string(),
            OUTDATED_CATEGORY.to_string(),
            UNMAINTAINED_CATEGORY.to_string(),
            ORPHANED_CATEGORY.to_string(),
        ];
        cats.extend(
            known
                .into_iter()
                .chain(extra)
                .filter(|c| counts.get(c).copied().unwrap_or(0) > 0),
        );
        cats
    }

    /// Candidate rows for the category picker: every taxonomy category plus
    /// any custom ones already in use, narrowed by what's typed into the
    /// picker's filter (`input_buffer`). Pseudo-categories are excluded -
    /// they're views, and assigning a package "Outdated" would be a lie the
    /// next `brew upgrade` exposes. The picker renders one extra row after
    /// these ("new category"), which is not part of this list.
    pub(super) fn picker_candidates(&self) -> Vec<String> {
        let known = classify::known_categories();
        let mut extra: Vec<String> = self
            .packages
            .iter()
            .map(|p| p.category.clone())
            .filter(|c| !known.contains(c))
            .collect();
        extra.sort();
        extra.dedup();

        let needle = self.input_buffer.to_lowercase();
        known
            .into_iter()
            .chain(extra)
            .filter(|c| needle.is_empty() || c.to_lowercase().contains(&needle))
            .collect()
    }

    /// Keeps the picker's highlighted row inside the candidate list (plus
    /// the trailing "new category" row) as typing narrows it down.
    pub(super) fn clamp_picker_selection(&mut self) {
        let last = self.picker_candidates().len(); // the "new category" row
        match self.picker_state.selected() {
            None => self.picker_state.select(Some(0)),
            Some(i) if i > last => self.picker_state.select(Some(last)),
            Some(_) => {}
        }
    }

    /// Keeps the sidebar selection in range. The category list can shrink
    /// underneath it - toggling dependency-only packages changes which
    /// categories are non-empty - and an index left pointing past the end
    /// would silently fall back to "All".
    pub(super) fn clamp_sidebar_selection(&mut self) {
        let len = self.sidebar_categories().len();
        if let Some(i) = self.sidebar_state.selected()
            && i >= len
        {
            self.sidebar_state.select(Some(len.saturating_sub(1)));
        }
    }

    pub(super) fn selected_category(&self) -> String {
        let cats = self.sidebar_categories();
        let idx = self.sidebar_state.selected().unwrap_or(0);
        cats.get(idx)
            .cloned()
            .unwrap_or_else(|| ALL_CATEGORY.to_string())
    }

    /// Identifies the current "what's visible" scope (category + filter +
    /// dependency toggle), so the size-sort cache can tell whether it's
    /// still valid or needs recomputing.
    pub(super) fn scope_signature(&self) -> String {
        format!(
            "{}|{}|{}",
            self.selected_category(),
            self.filter,
            self.show_deps
        )
    }

    /// Whether the list is currently showing grouped search results.
    ///
    /// Searching is the one action where the sidebar's scoping gets in the
    /// way: you reach for it precisely when you *don't* know which category
    /// a package sits in. So a search spans everything, and the category of
    /// each hit is handed back as a heading instead of a filter.
    pub(super) fn grouped(&self) -> bool {
        !self.filter.is_empty()
    }

    pub(super) fn visible_packages(&self) -> Vec<&ClassifiedPackage> {
        let category = self.selected_category();
        let filter = self.filter.to_lowercase();
        let searching = !filter.is_empty();
        // Orphans are dependency-only by definition, so this one view walks
        // the full package list instead of the on-request scope - otherwise
        // it would always be empty without the `d` toggle.
        let base = if !searching && category == ORPHANED_CATEGORY {
            self.packages.iter().collect::<Vec<_>>()
        } else {
            self.scoped_packages()
        };
        let mut result: Vec<&ClassifiedPackage> = base
            .into_iter()
            .filter(|p| {
                searching
                    || match category.as_str() {
                        ALL_CATEGORY => true,
                        OUTDATED_CATEGORY => p.package.outdated.is_some(),
                        UNMAINTAINED_CATEGORY => p.package.unmaintained,
                        ORPHANED_CATEGORY => self.orphans.contains(&p.package.name),
                        _ => p.category == category,
                    }
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
                if let Some((sig, sizes)) = &self.size_sort_cache
                    && *sig == self.scope_signature()
                {
                    result.sort_by(|a, b| {
                        let sa = sizes.get(&a.package.name).copied().unwrap_or(0);
                        let sb = sizes.get(&b.package.name).copied().unwrap_or(0);
                        sb.cmp(&sa)
                    });
                }
            }
        }
        result
    }

    /// The package list as it is actually rendered. Without a search this is
    /// just the visible packages one per row; with one, they're bucketed
    /// under category headings.
    pub(super) fn visible_rows(&self) -> Vec<ListRow<'_>> {
        let packages = self.visible_packages();
        if packages.is_empty() {
            return vec![ListRow::Empty];
        }
        if !self.grouped() {
            return packages.into_iter().map(ListRow::Package).collect();
        }

        // Groups are ordered by how many hits they hold, so the category
        // that best answers the search sits at the top; within a group the
        // sort chosen with `s` survives, because bucketing is stable.
        let mut groups: Vec<(&str, Vec<&ClassifiedPackage>)> = Vec::new();
        for p in packages {
            match groups.iter_mut().find(|(c, _)| *c == p.category.as_str()) {
                Some((_, members)) => members.push(p),
                None => groups.push((p.category.as_str(), vec![p])),
            }
        }
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

        let mut rows = Vec::new();
        for (category, members) in groups {
            rows.push(ListRow::Header {
                category,
                count: members.len(),
            });
            rows.extend(members.into_iter().map(ListRow::Package));
        }
        rows
    }

    /// Row indices the cursor may land on, in display order - i.e. every row
    /// except headings and the empty-state placeholder.
    pub(super) fn selectable_rows(&self) -> Vec<usize> {
        self.visible_rows()
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, ListRow::Package(_)))
            .map(|(i, _)| i)
            .collect()
    }

    /// Puts the cursor back at the top of the list, skipping a leading
    /// heading. Used whenever the visible set changes wholesale.
    pub(super) fn reset_list_selection(&mut self) {
        self.list_state
            .select(self.selectable_rows().first().copied());
    }

    pub(super) fn selected_package(&self) -> Option<&ClassifiedPackage> {
        match self.visible_rows().get(self.list_state.selected()?)? {
            ListRow::Package(p) => Some(p),
            _ => None,
        }
    }

    pub(super) fn clamp_list_selection(&mut self) {
        let selectable = self.selectable_rows();
        let (Some(&first), Some(&last)) = (selectable.first(), selectable.last()) else {
            self.list_state.select(None);
            return;
        };
        let target = match self.list_state.selected() {
            None => first,
            // A heading (or a row past the end) is never a valid resting
            // place: snap forward to the next package, or back to the last
            // one if the cursor ran off the bottom.
            Some(i) => selectable.into_iter().find(|&s| s >= i).unwrap_or(last),
        };
        self.list_state.select(Some(target));
    }
}

/// Recomputes the autoremove-candidate set for the current package list.
pub(super) fn compute_orphans(packages: &[ClassifiedPackage]) -> HashSet<String> {
    crate::details::autoremove_candidates(packages.iter().map(|p| &p.package))
}

pub(super) fn category_counts<'a>(
    packages: impl IntoIterator<Item = &'a ClassifiedPackage>,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for p in packages {
        *counts.entry(p.category.clone()).or_insert(0) += 1;
    }
    counts
}
