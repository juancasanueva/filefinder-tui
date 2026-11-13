//! Application state and what each key does to it.

use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position as Point, Rect};

use crate::format::DateStyle;
use crate::input::Input;
use crate::model::{self, Entry, SortMode};
use crate::ops::{self, Change, History, Kind, Report, Step};
use crate::places::{self, Place, PlaceKind, Section};
use crate::search::Walk;
use crate::state::{self, Saved, Trashed};
use crate::system::System;
use crate::thumbs::{Facts, Thumbs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
    Columns,
}

/// How big the grid's tiles are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileSize {
    Small,
    Normal,
    Large,
}

impl TileSize {
    fn bigger(self) -> Self {
        match self {
            TileSize::Small => TileSize::Normal,
            _ => TileSize::Large,
        }
    }

    fn smaller(self) -> Self {
        match self {
            TileSize::Large => TileSize::Normal,
            _ => TileSize::Small,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Files,
    Details,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub is_error: bool,
}

/// A question in front of the window. While one is open every key goes to it.
pub enum Dialog {
    /// A new name for the item at `path`.
    Rename { path: PathBuf, input: Input },
    /// The name of a folder to make in `folder`.
    NewFolder { folder: PathBuf, input: Input },
    /// Whether to delete `paths` for good. `armed` is whether Delete, rather
    /// than Cancel, is the lit choice.
    Delete { paths: Vec<PathBuf>, armed: bool },
    /// Whether to empty the trash: to delete for good the `paths` it held
    /// when the question was asked, and nothing that got there since.
    EmptyTrash { paths: Vec<PathBuf>, armed: bool },
}

/// What a line of the menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    Preview,
    Rename,
    Duplicate,
    Copy,
    Cut,
    Paste,
    PasteInto,
    CopyPath,
    Compress,
    Pin,
    Terminal,
    Trash,
    Delete,
    NewFolder,
    Restore,
    EmptyTrash,
    OpenPlace,
    Unpin,
    /// Open Terminal Here, in the menu for the folder itself.
    TerminalHere,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Item {
        label: &'static str,
        action: Action,
        /// Whether it can be used right now; one that can't is shown dim.
        enabled: bool,
    },
    /// The hairline between two groups.
    Separator,
}

/// The menu for the selection, or for a place in the sidebar. While it is
/// open every key goes to it.
pub struct Menu {
    pub rows: Vec<MenuRow>,
    /// The line the keyboard is on: never a hairline.
    pub cursor: usize,
    /// Where a right click opened it; one opened from the keyboard goes
    /// next to what it is about.
    pub at: Option<Point>,
}

impl Menu {
    /// Groups of lines with a hairline between each two; the groups with
    /// nothing in them are left out.
    fn new(groups: Vec<Vec<MenuRow>>) -> Self {
        let mut rows = Vec::new();
        for group in groups.into_iter().filter(|group| !group.is_empty()) {
            if !rows.is_empty() {
                rows.push(MenuRow::Separator);
            }
            rows.extend(group);
        }
        Self {
            rows,
            cursor: 0,
            at: None,
        }
    }

    /// Moves to the next line up or down, over the hairlines; at either end
    /// it stays where it is.
    fn step(&mut self, down: bool) {
        let is_item = |index: &usize| matches!(self.rows[*index], MenuRow::Item { .. });
        let next = if down {
            (self.cursor + 1..self.rows.len()).find(is_item)
        } else {
            (0..self.cursor).rev().find(is_item)
        };
        self.cursor = next.unwrap_or(self.cursor);
    }

    /// What Enter does on the line the keyboard is on: nothing on a dim one.
    fn chosen(&self) -> Option<Action> {
        match self.rows.get(self.cursor)? {
            MenuRow::Item {
                action,
                enabled: true,
                ..
            } => Some(*action),
            _ => None,
        }
    }
}

/// A line of the menu.
fn item(label: &'static str, action: Action) -> MenuRow {
    item_if(true, label, action)
}

/// A line of the menu that is dim unless `enabled`.
fn item_if(enabled: bool, label: &'static str, action: Action) -> MenuRow {
    MenuRow::Item {
        label,
        action,
        enabled,
    }
}

/// A field in the top bar. While one has the keyboard every key goes to it.
pub enum Field {
    /// Ctrl+L: a path to go to.
    Path(Input),
    /// `/`: what the active pane searches for. The pane holds the text.
    Search,
}

/// What a part of the window stands for, as the last frame drew it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spot {
    /// The sidebar, wherever no place is.
    Sidebar,
    /// A place in the sidebar, by its number among them all.
    Place(usize),
    /// A pane's files, wherever no item is.
    Pane(usize),
    /// An item of a pane: a row of the list or of its own column, or a tile.
    Item {
        pane: usize,
        index: usize,
    },
    /// A row of a folder above the pane's own in the columns view: item
    /// `index` of `ancestors[depth]`.
    Above {
        pane: usize,
        depth: usize,
        index: usize,
    },
    /// A breadcrumb, and the folder it stands for.
    Crumb {
        pane: usize,
        path: PathBuf,
    },
    /// The path field, Ctrl+L.
    PathField,
    /// The search field, `/`.
    SearchField,
    Toggle(Toggle),
    /// The details pane, wherever its Open button isn't.
    Details,
    OpenButton,
    /// The menu, wherever no line is.
    Menu,
    /// A line of the menu, by its number among its rows.
    MenuRow(usize),
    /// A dialog, wherever no button is.
    Dialog,
    /// A dialog's button: the one that does what it asks, or Cancel.
    Choice(bool),
    /// The shortcuts overlay.
    Help,
}

/// The switches at the right end of the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    View,
    Sort,
    Hidden,
    Details,
}

/// Where the last frame drew what a click can land on. Drawing writes it;
/// the mouse reads it and nothing else, so a click lands on what was on
/// screen under it.
#[derive(Clone, Debug, Default)]
pub struct HitMap {
    /// In the order drawn: a later one is on top of an earlier one.
    spots: Vec<(Rect, Spot)>,
    /// The sidebar's lines, the ones scrolled out of sight included.
    pub sidebar_lines: usize,
}

impl HitMap {
    pub fn add(&mut self, rect: Rect, spot: Spot) {
        if !rect.is_empty() {
            self.spots.push((rect, spot));
        }
    }

    /// What is on top at a cell.
    pub fn at(&self, x: u16, y: u16) -> Option<&Spot> {
        let under = |(rect, _): &&(Rect, Spot)| rect.contains(Point::new(x, y));
        self.spots.iter().rev().find(under).map(|(_, spot)| spot)
    }

    /// Where something was drawn.
    pub fn rect_of(&self, spot: &Spot) -> Option<Rect> {
        let found = self.spots.iter().find(|(_, drawn)| drawn == spot);
        found.map(|(rect, _)| *rect)
    }
}

/// How soon a second click on the same item makes it a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Rows of a list, or lines of the sidebar or the shortcuts, that one turn
/// of the wheel scrolls; the grid scrolls by one row of tiles.
const WHEEL_ROWS: usize = 3;

/// A left click on an item, for telling a double click.
struct Click {
    pane: usize,
    path: PathBuf,
    at: Instant,
}

/// What a pane lists.
pub enum Listing {
    /// The items of the folder at `cwd`.
    Folder,
    /// The files used lately, in place of a folder's. `cwd` is the home
    /// folder meanwhile: that is where "up" leads from there.
    Recents,
    /// What a search at and below `cwd` found, or what was there before
    /// while nothing is typed.
    Search(Search),
}

/// A search in a pane: `/` and what was typed after it.
pub struct Search {
    pub input: Input,
    /// What the pane showed before: where Escape and Back return to.
    origin: Visit,
    /// The walk looking for matches, until it has no more to give.
    walk: Option<Walk>,
    /// Where the cursor belongs once the walk finds one of them; the
    /// earlier in the list, the better.
    wanted: Vec<PathBuf>,
}

impl Search {
    /// Whether matches may still arrive.
    pub fn is_running(&self) -> bool {
        self.walk.is_some()
    }
}

/// What Ctrl+C or Ctrl+X took, waiting for Ctrl+V.
struct Clipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

/// File work under way on a thread of its own.
struct Work {
    /// What the band says meanwhile.
    label: String,
    cancel: Arc<AtomicBool>,
    results: Receiver<Report>,
}

impl Drop for Work {
    /// Work nobody waits for any more stops at the next file.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// How many of the files used lately Recents lists.
const RECENTS_LIMIT: usize = 200;

/// What is said when macOS keeps the terminal out of the trash.
const TRASH_REFUSED: &str = "Trash: Permission was refused. Give your terminal \
                             Full Disk Access in System Settings to see it.";

/// A folder in the history, with what was selected there.
#[derive(Clone, Debug)]
struct Visit {
    path: PathBuf,
    selected: Option<String>,
    /// It was Recents that was showing, not the folder.
    recents: bool,
}

/// Where a pane was before it was listed again behind its back: what to
/// put back afterwards.
struct Position {
    cursor: usize,
    offset: usize,
    /// The item the cursor was on.
    under_cursor: Option<PathBuf>,
    /// The items that were selected, when there were several.
    selected: Vec<PathBuf>,
}

/// A folder above the pane's own, as the columns view lists it.
pub struct Ancestor {
    pub entries: Vec<Entry>,
    /// The entry that leads down to the pane's folder, when it is listed.
    pub on_path: Option<usize>,
}

pub struct Pane {
    pub cwd: PathBuf,
    pub listing: Listing,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    /// The items selected, while there are several of them: with one, it is
    /// the item under the cursor and this is empty.
    marks: BTreeSet<usize>,
    /// Where the range that Shift and the arrows select started.
    anchor: Option<usize>,
    /// First visible row (of tiles, in the grid); the view keeps the cursor on screen with it.
    pub offset: usize,
    /// The folders from home (or `/`) down to this one's parent. Only the columns
    /// view needs them, so they are listed when a folder is shown in that view.
    pub ancestors: Vec<Ancestor>,
    /// Rows the pane's view showed last time, for Page Up and Page Down.
    pub page_rows: usize,
    /// Tiles across the pane's grid last time, for Up and Down.
    pub grid_columns: usize,
    back: Vec<Visit>,
    forward: Vec<Visit>,
}

impl Pane {
    fn new(cwd: PathBuf, entries: Vec<Entry>) -> Self {
        Self {
            cwd,
            listing: Listing::Folder,
            entries,
            cursor: 0,
            marks: BTreeSet::new(),
            anchor: None,
            offset: 0,
            ancestors: Vec::new(),
            page_rows: 10,
            grid_columns: 1,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// The item under the cursor: the one Enter opens and the details show.
    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    /// Whether an item is selected: the one under the cursor, or each of
    /// the several that a range or Ctrl+A took.
    pub fn is_selected(&self, index: usize) -> bool {
        if self.marks.is_empty() {
            index == self.cursor
        } else {
            self.marks.contains(&index)
        }
    }

    /// What is selected, in the order it is listed.
    pub fn selection(&self) -> impl Iterator<Item = &Entry> {
        let selected = |(index, _): &(usize, &Entry)| self.is_selected(*index);
        let listed = self.entries.iter().enumerate();
        listed.filter(selected).map(|(_, entry)| entry)
    }

    /// Selects everything from where a range started to the cursor.
    fn select_from(&mut self, anchor: usize) {
        self.anchor = Some(anchor);
        let (first, last) = (anchor.min(self.cursor), anchor.max(self.cursor));
        self.marks = (first..=last).filter(|_| first < last).collect();
    }

    /// Selects every item, wherever the cursor is.
    fn select_all(&mut self) {
        self.anchor = None;
        let all = 0..self.entries.len();
        self.marks = all.filter(|_| self.entries.len() > 1).collect();
    }

    /// Leaves only the item under the cursor selected.
    fn collapse(&mut self) {
        self.marks.clear();
        self.anchor = None;
    }

    /// Where the pane is, to put back once it has been listed again.
    fn position(&self) -> Position {
        let path = |entry: &Entry| entry.path.clone();
        let several = !self.marks.is_empty();
        Position {
            cursor: self.cursor,
            offset: self.offset,
            under_cursor: self.selected().map(path),
            selected: self.selection().filter(|_| several).map(path).collect(),
        }
    }

    /// Puts the pane back where it was after listing it again: the cursor on
    /// its item, or on the row that item had, the view scrolled as it was,
    /// and what was selected selected, as far as it is still listed.
    fn put_back(&mut self, was: Position) {
        let last = self.entries.len().saturating_sub(1);
        self.cursor = was.cursor.min(last);
        self.want(was.under_cursor.into_iter().collect());
        self.offset = was.offset;
        let selected: HashSet<&PathBuf> = was.selected.iter().collect();
        let listed = self.entries.iter().enumerate();
        let marks: BTreeSet<usize> = listed
            .filter(|(_, entry)| selected.contains(&entry.path))
            .map(|(index, _)| index)
            .collect();
        // A selection the cursor's item fell out of is no longer one.
        if marks.len() > 1 && marks.contains(&self.cursor) {
            self.marks = marks;
        }
    }

    /// Whether the pane lists a folder's own items. Only there can things
    /// be pasted or made, and only that can be pinned.
    pub fn is_folder(&self) -> bool {
        matches!(self.listing, Listing::Folder)
    }

    pub fn recents(&self) -> bool {
        matches!(self.listing, Listing::Recents)
    }

    /// The search that is open in the pane.
    pub fn search(&self) -> Option<&Search> {
        match &self.listing {
            Listing::Search(search) => Some(search),
            _ => None,
        }
    }

    /// The search whose matches the pane lists: one with something typed.
    pub fn results(&self) -> Option<&Search> {
        self.search()
            .filter(|search| !search.input.chars().is_empty())
    }

    /// Ends the search that is open and hands it over. What the pane lists
    /// is for the caller to put right.
    fn take_search(&mut self) -> Option<Search> {
        self.search()?;
        match std::mem::replace(&mut self.listing, Listing::Folder) {
            Listing::Search(search) => Some(search),
            _ => None,
        }
    }

    /// Where Back returns to from what comes next.
    fn visit(&self) -> Visit {
        match &self.listing {
            // What a search found is no place to come back to: the folder
            // it looked in is, as it was.
            Listing::Search(search) => search.origin.clone(),
            listing => Visit {
                path: self.cwd.clone(),
                selected: self.selected().map(|entry| entry.name.clone()),
                recents: matches!(listing, Listing::Recents),
            },
        }
    }

    /// Lists a folder's items. A search that was open ends here, and its walk with it.
    fn show(&mut self, cwd: PathBuf, entries: Vec<Entry>, select: Option<&str>) {
        self.cwd = cwd;
        self.listing = Listing::Folder;
        self.entries = entries;
        self.offset = 0;
        self.cursor = 0;
        self.collapse();
        if let Some(name) = select {
            self.select(name);
        }
    }

    /// Puts the cursor on the item with this name, when it is listed.
    fn select(&mut self, name: &str) {
        if let Some(index) = self.entries.iter().position(|entry| entry.name == name) {
            self.cursor = index;
            self.collapse();
        }
    }

    /// Puts the cursor on the first of these that is listed. A search goes
    /// on looking for the ones before it among what its walk still finds.
    fn want(&mut self, wanted: Vec<PathBuf>) {
        let listed = |path: &PathBuf| self.entries.iter().position(|entry| entry.path == *path);
        if let Some(index) = wanted.iter().find_map(listed) {
            self.cursor = index;
            self.collapse();
        }
        if let Listing::Search(search) = &mut self.listing {
            search.wanted = wanted;
        }
    }

    /// Takes in what the search found since the last turn.
    fn collect_found(&mut self) {
        let Listing::Search(search) = &mut self.listing else {
            return;
        };
        let Some(walk) = &mut search.walk else { return };
        let (found, finished) = walk.collect();
        if finished {
            search.walk = None;
        }
        for entry in found {
            if let Some(rank) = search.wanted.iter().position(|path| *path == entry.path) {
                // Only one wanted more than this one moves the cursor again.
                search.wanted.truncate(rank);
                self.cursor = self.entries.len();
                self.marks.clear();
                self.anchor = None;
            }
            self.entries.push(entry);
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    /// Moves the cursor onto an item, the way the arrows would.
    fn point_at(&mut self, index: usize) {
        self.move_cursor(index as isize - self.cursor as isize);
    }

    /// Alt+click: adds an item to the selection, with the cursor on it, or
    /// takes it out. The last one selected stays: something always is.
    fn toggle(&mut self, index: usize) {
        if index >= self.entries.len() {
            return;
        }
        if self.marks.is_empty() {
            self.marks.insert(self.cursor);
        }
        if self.marks.insert(index) {
            self.cursor = index;
        } else if self.marks.len() > 1 {
            self.marks.remove(&index);
            if self.cursor == index {
                let after = self.marks.range(index..).next();
                let before = self.marks.range(..index).next_back();
                self.cursor = *after.or(before).unwrap_or(&index);
            }
        }
        self.anchor = Some(self.cursor);
        // One is the item under the cursor, which needs no marks.
        if self.marks.len() == 1 {
            self.marks.clear();
        }
    }

    /// One turn of the wheel: `step` rows of `across` items further down or
    /// up, as far as there are. The cursor moves only to stay in sight, in
    /// its column; a selection of several items stays as it is.
    fn wheel(&mut self, down: bool, step: usize, across: usize) {
        let across = across.max(1);
        let rows = self.page_rows.max(1);
        let last = self.entries.len().div_ceil(across).saturating_sub(rows);
        self.offset = if down {
            (self.offset + step).min(last)
        } else {
            self.offset.min(last).saturating_sub(step)
        };
        let row = self.cursor / across;
        let kept = row.clamp(self.offset, self.offset + rows - 1);
        if kept != row {
            let column = self.cursor % across;
            self.cursor = (kept * across + column).min(self.entries.len().saturating_sub(1));
        }
    }

    /// Moves one row up or down in a grid. Down from above a shorter last row
    /// lands on the last item.
    fn move_row(&mut self, down: bool, columns: usize) {
        let last = self.entries.len().saturating_sub(1);
        if !down {
            if self.cursor >= columns {
                self.cursor -= columns;
            }
        } else if self.cursor + columns <= last {
            self.cursor += columns;
        } else if self.cursor / columns < last / columns {
            self.cursor = last;
        }
    }
}

pub struct Config {
    pub home: PathBuf,
    /// A folder to open, or a file to select in its folder.
    pub start: PathBuf,
    pub date_style: DateStyle,
    /// Where the view, the tile size and the pinned folders are remembered;
    /// `None` remembers nothing.
    pub state_dir: Option<PathBuf>,
    /// What makes the thumbnails; `None` leaves every file its stand-in.
    pub thumbs: Option<Thumbs>,
    /// Where Delete moves things: the trash folder.
    pub trash_dir: PathBuf,
    /// Where drives are mounted.
    pub volumes_dir: PathBuf,
}

pub struct App {
    pub home: PathBuf,
    pub panes: Vec<Pane>,
    pub active: usize,
    pub focus: Focus,
    pub view: ViewMode,
    pub tile: TileSize,
    pub sort: SortMode,
    pub show_hidden: bool,
    pub details_visible: bool,
    pub sidebar: Vec<Section>,
    pub sidebar_cursor: usize,
    pub help_visible: bool,
    /// How many lines the shortcuts are scrolled down; drawing keeps it in range.
    pub help_scroll: usize,
    /// Lines of shortcuts shown last time, for Page Up and Page Down.
    pub help_rows: usize,
    pub dialog: Option<Dialog>,
    /// The menu that is open: Alt+Enter.
    pub menu: Option<Menu>,
    /// The field in the top bar that has the keyboard.
    pub field: Option<Field>,
    pub message: Option<Message>,
    pub should_quit: bool,
    pub date_style: DateStyle,
    pub thumbs: Option<Thumbs>,
    /// What was read about the selected item for the details pane.
    pub facts: Option<Facts>,
    clipboard: Option<Clipboard>,
    history: History,
    work: Option<Work>,
    /// Every pinned folder, the ones that are gone included: they keep their
    /// line in the file and are only left out of the sidebar.
    pins: Vec<PathBuf>,
    /// Where what this app moved to the trash came from, and when: one
    /// record for each item, as long as it is there.
    trashed: Vec<Trashed>,
    trash_dir: PathBuf,
    volumes_dir: PathBuf,
    state_dir: Option<PathBuf>,
    system: Box<dyn System>,
    /// Where the last frame drew what a click can land on.
    pub hits: HitMap,
    /// Lines of the sidebar the wheel scrolled away; drawing keeps it in range.
    pub sidebar_scroll: usize,
    /// The last left click on an item, for telling a double click.
    last_click: Option<Click>,
    /// What time it is: the real clock, or a test's.
    clock: Box<dyn Fn() -> Instant>,
}

impl App {
    pub fn new(config: Config, system: Box<dyn System>) -> Self {
        let saved = config
            .state_dir
            .as_deref()
            .map(state::load)
            .unwrap_or_default();
        let pins = config
            .state_dir
            .as_deref()
            .map(state::load_pins)
            .unwrap_or_default();
        let trashed = config
            .state_dir
            .as_deref()
            .map(state::load_trashed)
            .unwrap_or_default();
        let mut app = Self {
            sidebar: Vec::new(),
            panes: vec![Pane::new(config.home.clone(), Vec::new())],
            home: config.home,
            active: 0,
            focus: Focus::Files,
            view: saved.view,
            tile: saved.tile,
            sort: SortMode::Name,
            show_hidden: false,
            details_visible: true,
            sidebar_cursor: 0,
            help_visible: false,
            help_scroll: 0,
            help_rows: 1,
            dialog: None,
            menu: None,
            field: None,
            message: None,
            should_quit: false,
            date_style: config.date_style,
            thumbs: config.thumbs,
            facts: None,
            clipboard: None,
            history: History::default(),
            work: None,
            pins,
            trashed,
            trash_dir: config.trash_dir,
            volumes_dir: config.volumes_dir,
            state_dir: config.state_dir,
            system,
            hits: HitMap::default(),
            sidebar_scroll: 0,
            last_click: None,
            clock: Box::new(Instant::now),
        };
        app.rebuild_sidebar();
        let (folder, select) = if config.start.is_dir() {
            (config.start, None)
        } else {
            let name = config
                .start
                .file_name()
                .map(|name| name.to_string_lossy().into_owned());
            (
                config
                    .start
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                name,
            )
        };
        if !app.show_folder(&folder, select.as_deref()) {
            let home = app.home.clone();
            app.show_folder(&home, None);
        }
        app
    }

    pub fn pane(&self) -> &Pane {
        &self.panes[self.active]
    }

    fn pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active]
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.pane().selected()
    }

    pub fn places(&self) -> impl Iterator<Item = &Place> {
        self.sidebar
            .iter()
            .flat_map(|section| section.places.iter())
    }

    /// The sidebar place the active pane is in: Recents while it shows them,
    /// and otherwise the deepest place holding its folder.
    pub fn current_place(&self) -> Option<usize> {
        let pane = self.pane();
        let recents = |place: &Place| place.kind == PlaceKind::Recents;
        if pane.recents() {
            return self.places().position(recents);
        }
        self.places()
            .enumerate()
            .filter(|(_, place)| !recents(place) && pane.cwd.starts_with(&place.path))
            .max_by_key(|(_, place)| place.path.components().count())
            .map(|(index, _)| index)
    }

    /// Reads the sidebar's places again. Done at the start, on F5, when a pin
    /// changes and as the keyboard moves into it; never while drawing.
    fn rebuild_sidebar(&mut self) {
        self.sidebar = places::sections(&self.home, &self.pins, &self.trash_dir, &self.volumes_dir);
        let last = self.places().count().saturating_sub(1);
        self.sidebar_cursor = self.sidebar_cursor.min(last);
    }

    /// What the file work under way is doing, in the band's words.
    pub fn working(&self) -> Option<&str> {
        self.work.as_ref().map(|work| work.label.as_str())
    }

    /// What every turn of the event loop does besides keys: takes in the file
    /// work that finished, what a search found and the thumbnails made since
    /// the last one, and reads what the details pane says about a newly
    /// selected item, so that drawing never has to.
    pub fn tick(&mut self) {
        self.collect_work();
        for pane in &mut self.panes {
            pane.collect_found();
        }
        if let Some(thumbs) = &mut self.thumbs {
            thumbs.collect();
        }
        let selected = self.panes[self.active].selected();
        let current = match (&self.facts, selected) {
            (Some(facts), Some(entry)) => facts.describes(entry),
            (None, None) => true,
            _ => false,
        };
        if !current {
            self.facts = selected.map(Facts::read);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        // A key is the user taking over: the cursor is no longer moved to
        // what a search under way was asked to find again.
        self.pane_mut().want(Vec::new());
        if let Some(menu) = self.menu.take() {
            self.menu_key(menu, key);
            return;
        }
        if self.help_visible {
            // The shortcuts keep every key: these scroll or close them, and
            // no other reaches the files underneath.
            let page = self.help_rows.max(1);
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help_visible = false,
                KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(page),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(page),
                _ => {}
            }
            return;
        }
        if let Some(dialog) = self.dialog.take() {
            self.dialog_key(dialog, key);
            return;
        }
        if let Some(field) = self.field.take() {
            match field {
                Field::Path(input) => self.path_key(input, key),
                Field::Search => self.search_key(key),
            }
            return;
        }
        if opens_menu(key) {
            self.open_menu();
            return;
        }
        if self.file_key(key) {
            return;
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = !alt && key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !alt && !ctrl;
        match key.code {
            KeyCode::Char('?') => {
                self.help_visible = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('q' | 'Q') if plain => self.quit(),
            KeyCode::Char('/') if plain => self.search(),
            KeyCode::Char('l') if ctrl => self.ask_path(),
            // Ctrl+Alt+N, whichever way the terminal spells the letter.
            KeyCode::Char('n' | 'N') if alt && key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.split()
            }
            KeyCode::Char('w') if ctrl => self.close_split(),
            KeyCode::Esc => self.escape(),
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Left if alt => self.go_back(),
            KeyCode::Right if alt => self.go_forward(),
            // What Ghostty and Terminal send for Option+arrows on a Mac.
            KeyCode::Char('b') if alt => self.go_back(),
            KeyCode::Char('f') if alt => self.go_forward(),
            KeyCode::Home if alt => self.go_home(),
            KeyCode::F(5) => self.refresh(),
            KeyCode::Backspace if plain => self.go_up(),
            KeyCode::Char(' ') if plain => self.preview_selected(),
            KeyCode::Char('s') if plain => self.set_sort(self.sort.step(1)),
            KeyCode::Char('S') if plain => self.set_sort(self.sort.step(-1)),
            KeyCode::Char('1') if ctrl => self.set_view(ViewMode::Grid),
            KeyCode::Char('2') if ctrl => self.set_view(ViewMode::List),
            KeyCode::Char('3') if ctrl => self.set_view(ViewMode::Columns),
            KeyCode::Char('h') if ctrl => self.toggle_hidden(),
            KeyCode::Char('i') if ctrl => self.toggle_details(),
            KeyCode::Char('p') if ctrl => self.toggle_pin(),
            // Ctrl++ is Ctrl+Shift+= on most keyboards; either spelling may arrive.
            KeyCode::Char('=' | '+') if ctrl => self.set_tile(self.tile.bigger()),
            KeyCode::Char('-') if ctrl => self.set_tile(self.tile.smaller()),
            KeyCode::Char('0') if ctrl => self.set_tile(TileSize::Normal),
            _ => match self.focus {
                Focus::Sidebar => self.sidebar_key(key),
                Focus::Files => self.files_key(key),
                Focus::Details => self.details_key(key),
            },
        }
    }

    /// The mouse twin of `handle_key`: a click, a double click, a right click
    /// or a turn of the wheel lands on what the last frame drew under it and
    /// does what the key for the same thing does. Drags, moves, releases and
    /// the buttons crossterm has no name for do nothing.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let spot = self.hits.at(mouse.column, mouse.row).cloned();
        let at = Point::new(mouse.column, mouse.row);
        let down = match mouse.kind {
            MouseEventKind::Down(MouseButton::Middle) => return,
            MouseEventKind::Down(_) => None,
            MouseEventKind::ScrollDown => Some(true),
            MouseEventKind::ScrollUp => Some(false),
            _ => return,
        };
        // As with a key, the user takes over from a search that was asked
        // to put the cursor somewhere.
        for pane in &mut self.panes {
            pane.want(Vec::new());
        }
        match (mouse.kind, down) {
            (_, Some(down)) => self.wheel(spot, down),
            (MouseEventKind::Down(button), None) => self.click(button, spot, at, mouse.modifiers),
            _ => {}
        }
    }

    fn click(&mut self, button: MouseButton, spot: Option<Spot>, at: Point, held: KeyModifiers) {
        let last = self.last_click.take();
        let left = button == MouseButton::Left;
        // The menu, the shortcuts and a dialog keep the clicks to themselves.
        if self.menu.is_some() {
            match spot {
                Some(Spot::MenuRow(row)) if left => self.choose_row(row),
                Some(Spot::Menu | Spot::MenuRow(_)) => {}
                // Anywhere else it closes, with nothing done.
                _ => self.menu = None,
            }
            return;
        }
        if self.help_visible {
            return;
        }
        if self.dialog.is_some() {
            if let (true, Some(Spot::Choice(yes))) = (left, spot) {
                self.choose(yes);
            }
            return;
        }
        let Some(spot) = spot else { return };
        if !matches!(spot, Spot::PathField | Spot::SearchField) {
            self.leave_field();
        }
        match (left, spot) {
            (true, Spot::Item { pane, index }) => self.click_item(pane, index, held, last),
            (true, Spot::Above { pane, depth, index }) => {
                self.click_above(pane, depth, index);
            }
            (true, Spot::Pane(pane)) => self.enter_pane(pane),
            (true, Spot::Place(place)) => {
                self.sidebar_cursor = place;
                self.open_place();
            }
            (true, Spot::Crumb { pane, path }) => {
                self.active = pane.min(self.panes.len() - 1);
                self.navigate(&path);
            }
            (true, Spot::SearchField) => self.search(),
            (true, Spot::Toggle(toggle)) => self.flip(toggle),
            (true, Spot::OpenButton) => self.open_selected(),
            (false, Spot::Item { pane, index }) => {
                self.enter_pane(pane);
                let pane = self.pane_mut();
                if index < pane.entries.len() {
                    // Inside the selection it is about all of it.
                    if !pane.is_selected(index) {
                        pane.collapse();
                    }
                    pane.point_at(index);
                    let groups = self.files_menu();
                    self.show_menu(groups, Some(at));
                }
            }
            (false, Spot::Above { pane, depth, index }) => {
                if self.click_above(pane, depth, index) {
                    let groups = self.files_menu();
                    self.show_menu(groups, Some(at));
                }
            }
            (false, Spot::Pane(pane)) => {
                self.enter_pane(pane);
                let groups = self.folder_menu();
                self.show_menu(groups, Some(at));
            }
            (false, Spot::Place(place)) => {
                self.focus = Focus::Sidebar;
                self.sidebar_cursor = place;
                let groups = self.place_menu();
                self.show_menu(groups, Some(at));
            }
            _ => {}
        }
    }

    /// A click on a line of the menu: Enter on it. A dim line does nothing.
    fn choose_row(&mut self, row: usize) {
        let Some(menu) = &mut self.menu else { return };
        if matches!(menu.rows.get(row), Some(MenuRow::Item { .. })) {
            menu.cursor = row;
            self.handle_key(KeyEvent::from(KeyCode::Enter));
        }
    }

    /// A click on a dialog's button: Enter on the choice that does what it
    /// asks, or Escape.
    fn choose(&mut self, yes: bool) {
        if !yes {
            return self.handle_key(KeyEvent::from(KeyCode::Esc));
        }
        if let Some(Dialog::Delete { armed, .. } | Dialog::EmptyTrash { armed, .. }) =
            &mut self.dialog
        {
            *armed = true;
        }
        self.handle_key(KeyEvent::from(KeyCode::Enter));
    }

    /// A click away from the field in the top bar that has the keyboard:
    /// the path field closes as with Escape, and the search hands the
    /// keyboard to what it found as with Tab.
    fn leave_field(&mut self) {
        let key = match self.field {
            Some(Field::Path(_)) => KeyCode::Esc,
            Some(Field::Search) => KeyCode::Tab,
            None => return,
        };
        self.handle_key(KeyEvent::from(key));
    }

    /// The keyboard goes to a pane's files.
    fn enter_pane(&mut self, pane: usize) {
        self.focus = Focus::Files;
        self.active = pane.min(self.panes.len() - 1);
    }

    /// A left click on an item: the cursor there and only it selected; with
    /// Shift the range from where it started; with Alt (or Cmd) one more or
    /// one less. A second plain click on it soon after opens it like Enter.
    fn click_item(&mut self, pane: usize, index: usize, held: KeyModifiers, last: Option<Click>) {
        let now = (self.clock)();
        self.enter_pane(pane);
        let pane = self.pane_mut();
        let Some(path) = pane.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };
        if held.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) {
            return pane.toggle(index);
        }
        let anchor = pane.anchor.unwrap_or(pane.cursor);
        pane.point_at(index);
        if held.contains(KeyModifiers::SHIFT) {
            return pane.select_from(anchor);
        }
        pane.collapse();
        let again = |click: &Click| {
            click.pane == self.active
                && click.path == path
                && now.saturating_duration_since(click.at) <= DOUBLE_CLICK
        };
        if last.as_ref().is_some_and(again) {
            self.open_selected();
        } else {
            let pane = self.active;
            self.last_click = Some(Click {
                pane,
                path,
                at: now,
            });
        }
    }

    /// A click on a row of a folder above the pane's own in the columns
    /// view: goes to that folder with the row selected. False when it couldn't.
    fn click_above(&mut self, pane: usize, depth: usize, index: usize) -> bool {
        let now = (self.clock)();
        let above = self.panes.get(pane).and_then(|it| it.ancestors.get(depth));
        let Some(entry) = above.and_then(|above| above.entries.get(index)) else {
            return false;
        };
        let (path, name) = (entry.path.clone(), entry.name.clone());
        let Some(folder) = path.parent().map(Path::to_path_buf) else {
            return false;
        };
        self.enter_pane(pane);
        let arrived = self.go_to(&folder, Some(&name));
        if arrived {
            let pane = self.active;
            self.last_click = Some(Click {
                pane,
                path,
                at: now,
            });
        }
        arrived
    }

    /// A click on a switch in the top bar: what its key does. The view goes
    /// round from the grid to the list to the columns.
    fn flip(&mut self, toggle: Toggle) {
        match toggle {
            Toggle::View => self.set_view(match self.view {
                ViewMode::Grid => ViewMode::List,
                ViewMode::List => ViewMode::Columns,
                ViewMode::Columns => ViewMode::Grid,
            }),
            Toggle::Sort => self.set_sort(self.sort.step(1)),
            Toggle::Hidden => self.toggle_hidden(),
            Toggle::Details => self.toggle_details(),
        }
    }

    /// A turn of the wheel over a pane, the sidebar or the shortcuts
    /// scrolls it; under the menu or a dialog nothing does.
    fn wheel(&mut self, spot: Option<Spot>, down: bool) {
        if self.menu.is_some() || self.dialog.is_some() {
            return;
        }
        if self.help_visible {
            if spot == Some(Spot::Help) {
                let key = if down { KeyCode::Down } else { KeyCode::Up };
                for _ in 0..WHEEL_ROWS {
                    self.handle_key(KeyEvent::from(key));
                }
            }
            return;
        }
        match spot {
            Some(Spot::Item { pane, .. } | Spot::Above { pane, .. } | Spot::Pane(pane)) => {
                let grid = self.view == ViewMode::Grid;
                let Some(pane) = self.panes.get_mut(pane) else {
                    return;
                };
                if grid {
                    let across = pane.grid_columns;
                    pane.wheel(down, 1, across);
                } else {
                    pane.wheel(down, WHEEL_ROWS, 1);
                }
            }
            Some(Spot::Sidebar | Spot::Place(_)) => {
                let rows = self
                    .hits
                    .rect_of(&Spot::Sidebar)
                    .map_or(0, |rect| rect.height);
                let most = self.hits.sidebar_lines.saturating_sub(rows as usize);
                let scroll = self.sidebar_scroll.min(most);
                self.sidebar_scroll = if down {
                    (scroll + WHEEL_ROWS).min(most)
                } else {
                    scroll.saturating_sub(WHEEL_ROWS)
                };
            }
            _ => {}
        }
    }

    /// The keys that change files. True when the key was one of them.
    fn file_key(&mut self, key: KeyEvent) -> bool {
        let held = key.modifiers;
        let alt = held.contains(KeyModifiers::ALT);
        let ctrl = !alt && held.contains(KeyModifiers::CONTROL);
        let bare = !alt && !ctrl;
        // A shifted chord arrives as the capital, or as the small letter with
        // Shift held, depending on the terminal.
        let (letter, capital) = match key.code {
            KeyCode::Char(c) => (Some(c.to_ascii_lowercase()), c.is_ascii_uppercase()),
            _ => (None, false),
        };
        let shift = capital || held.contains(KeyModifiers::SHIFT);
        match (key.code, letter) {
            (_, Some('c')) if ctrl && shift => self.copy_path(),
            (_, Some('c')) if ctrl => self.take(false),
            (_, Some('x')) if ctrl && !shift => self.take(true),
            (_, Some('v')) if ctrl && !shift => self.paste(),
            (_, Some('d')) if ctrl && shift => self.duplicate(),
            (_, Some('n')) if ctrl && shift => self.ask_new_folder(),
            (_, Some('z')) if ctrl && shift => self.redo(),
            (_, Some('z')) if ctrl => self.undo(),
            (_, Some('y')) if ctrl && !shift => self.redo(),
            (KeyCode::F(2), _) => self.ask_rename(),
            (KeyCode::Delete, _) if bare && shift => self.ask_delete(),
            (KeyCode::Delete, _) if bare => self.delete_key(),
            // A Mac keyboard's own way to say Delete.
            (KeyCode::Backspace, _) if held.contains(KeyModifiers::SUPER) => self.delete_key(),
            _ => return false,
        }
        true
    }

    /// Alt+Enter: opens the menu for the selection, or for the place under
    /// the cursor while the keyboard is in the sidebar.
    fn open_menu(&mut self) {
        let groups = if self.focus == Focus::Sidebar {
            self.place_menu()
        } else {
            self.files_menu()
        };
        self.show_menu(groups, None);
    }

    /// Opens a menu of these lines, at a right click or, without one, next
    /// to what it is about.
    fn show_menu(&mut self, groups: Vec<Vec<MenuRow>>, at: Option<Point>) {
        let mut menu = Menu::new(groups);
        menu.at = at;
        if !menu.rows.is_empty() {
            self.menu = Some(menu);
        }
    }

    /// The menu's lines for what is selected in the active pane, in groups.
    fn files_menu(&self) -> Vec<Vec<MenuRow>> {
        let pane = self.pane();
        let empty_trash = item("Empty Trash…", Action::EmptyTrash);
        let pasting = self.clipboard.is_some();
        let Some(entry) = pane.selected() else {
            return self.folder_menu();
        };
        if self.in_trash() {
            let selection = vec![
                item("Restore", Action::Restore),
                item("Delete…", Action::Delete),
            ];
            return vec![selection, vec![empty_trash]];
        }
        let selection = self.selection();
        let one = selection.len() == 1;
        // An archive goes beside its items: they have to be in one folder.
        let together = selection
            .iter()
            .all(|path| path.parent() == selection[0].parent());
        let (mut paste_into, mut pin) = (None, None);
        if entry.is_dir {
            let label = if self.pins.contains(&entry.path) {
                "Unpin"
            } else {
                "Pin"
            };
            paste_into = Some(item_if(
                one && pasting,
                "Paste Into Folder",
                Action::PasteInto,
            ));
            pin = Some(item_if(one, label, Action::Pin));
        }
        let open = if entry.is_dir { "Open Folder" } else { "Open" };
        let clipboard = [
            Some(item("Copy", Action::Copy)),
            Some(item("Cut", Action::Cut)),
            paste_into,
            Some(item("Copy Path", Action::CopyPath)),
        ];
        vec![
            vec![item(open, Action::Open), item("Preview", Action::Preview)],
            vec![
                item_if(one, "Rename", Action::Rename),
                item("Duplicate", Action::Duplicate),
            ],
            clipboard.into_iter().flatten().collect(),
            vec![item_if(together, "Compress", Action::Compress)],
            pin.into_iter().collect(),
            vec![item("Open Terminal Here", Action::Terminal)],
            vec![
                item("Move to Trash", Action::Trash),
                item("Delete…", Action::Delete),
            ],
        ]
    }

    /// The menu's lines for the active pane's folder itself: what an empty
    /// folder offers, and what a right click beside the items offers.
    fn folder_menu(&self) -> Vec<Vec<MenuRow>> {
        if self.in_trash() {
            return vec![vec![item("Empty Trash…", Action::EmptyTrash)]];
        }
        let here = self.pane().is_folder();
        let pasting = self.clipboard.is_some();
        vec![vec![
            item_if(here && pasting, "Paste", Action::Paste),
            item_if(here, "New Folder", Action::NewFolder),
            item_if(here, "Open Terminal Here", Action::TerminalHere),
        ]]
    }

    /// The menu's lines for the sidebar place the keyboard is on.
    fn place_menu(&self) -> Vec<Vec<MenuRow>> {
        let Some(place) = self.places().nth(self.sidebar_cursor) else {
            return Vec::new();
        };
        let more = match place.kind {
            PlaceKind::Pinned => vec![item("Unpin", Action::Unpin)],
            PlaceKind::Trash => vec![item("Empty Trash…", Action::EmptyTrash)],
            _ => Vec::new(),
        };
        vec![vec![item("Open", Action::OpenPlace)], more]
    }

    /// A key pressed while the menu is open: it goes nowhere else.
    fn menu_key(&mut self, mut menu: Menu, key: KeyEvent) {
        match key.code {
            // Closed, with nothing done.
            KeyCode::Esc => return,
            KeyCode::Up => menu.step(false),
            KeyCode::Down => menu.step(true),
            KeyCode::Enter => {
                if let Some(action) = menu.chosen() {
                    self.run(action);
                    return;
                }
            }
            _ => {}
        }
        self.menu = Some(menu);
    }

    /// Does what a line of the menu says, with the code its key runs.
    fn run(&mut self, action: Action) {
        match action {
            Action::Open => self.open_selected(),
            Action::Preview => self.preview_selected(),
            Action::Rename => self.ask_rename(),
            Action::Duplicate => self.duplicate(),
            Action::Copy => self.take(false),
            Action::Cut => self.take(true),
            Action::Paste => self.paste(),
            Action::PasteInto => self.paste_into_selected(),
            Action::CopyPath => self.copy_path(),
            Action::Compress => self.compress(),
            Action::Pin => self.pin_selected(),
            Action::Terminal => self.open_terminal(),
            Action::TerminalHere => self.open_terminal_here(),
            Action::Trash => self.trash(),
            Action::Delete => self.ask_delete(),
            Action::NewFolder => self.ask_new_folder(),
            Action::Restore => self.restore(),
            Action::EmptyTrash => self.ask_empty_trash(),
            Action::OpenPlace => self.open_place(),
            Action::Unpin => self.unpin_place(),
        }
    }

    /// A key pressed while a dialog is open: it goes nowhere else.
    fn dialog_key(&mut self, mut dialog: Dialog, key: KeyEvent) {
        match key.code {
            // Closed, with nothing done.
            KeyCode::Esc => return,
            KeyCode::Enter => {
                self.answer(dialog);
                return;
            }
            _ => {}
        }
        match &mut dialog {
            Dialog::Rename { input, .. } | Dialog::NewFolder { input, .. } => {
                input.handle_key(key);
            }
            Dialog::Delete { armed, .. } | Dialog::EmptyTrash { armed, .. } => {
                let switches = [
                    KeyCode::Left,
                    KeyCode::Right,
                    KeyCode::Tab,
                    KeyCode::BackTab,
                ];
                if switches.contains(&key.code) {
                    *armed = !*armed;
                }
            }
        }
        self.dialog = Some(dialog);
    }

    /// Ctrl+L: turns the breadcrumbs into a field holding the pane's path.
    fn ask_path(&mut self) {
        let path = self.shown_path(&self.pane().cwd);
        self.field = Some(Field::Path(Input::new(&path)));
    }

    /// A path the way the top bar and the details pane spell it: `~` for
    /// the home folder.
    pub fn shown_path(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }

    /// A key pressed while the path field is open: it goes nowhere else.
    fn path_key(&mut self, mut input: Input, key: KeyEvent) {
        match key.code {
            // Closed, with nothing done but what it said taken back.
            KeyCode::Esc => {
                self.message = None;
                return;
            }
            KeyCode::Enter => {
                self.go_to_typed(input);
                return;
            }
            KeyCode::Tab => self.complete(&mut input),
            _ => {
                input.handle_key(key);
            }
        }
        self.field = Some(Field::Path(input));
    }

    /// Where a typed path leads: `~` is the home folder, and what doesn't
    /// start at `/` starts at the pane's folder. `.` and `..` are followed
    /// as written, without asking the disk.
    fn resolve(&self, typed: &str) -> PathBuf {
        let below_home = typed
            .strip_prefix('~')
            .filter(|rest| rest.is_empty() || rest.starts_with('/'));
        let spelled = match below_home {
            Some(rest) => self.home.join(rest.trim_start_matches('/')),
            None => self.pane().cwd.join(typed),
        };
        let mut path = PathBuf::new();
        for part in spelled.components() {
            match part {
                Component::ParentDir => {
                    path.pop();
                }
                Component::CurDir => {}
                part => path.push(part),
            }
        }
        path
    }

    /// Enter in the path field: goes to the folder typed, or to a file's
    /// folder with the file selected. The field stays open when it can't.
    fn go_to_typed(&mut self, input: Input) {
        let typed = input.text();
        let typed = typed.trim();
        let target = self.resolve(typed);
        let arrived = match Entry::from_path(target.clone()) {
            Ok(entry) if entry.is_dir => self.go_to(&target, None),
            Ok(entry) => {
                let folder = target.parent().unwrap_or(&target);
                self.go_to(folder, Some(&entry.name))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.fail(format!("{typed} doesn't exist."));
                false
            }
            Err(error) => {
                self.fail(read_error(&target, &error));
                false
            }
        };
        if !arrived {
            self.field = Some(Field::Path(input));
        }
    }

    /// Tab in the path field: fills in the folder name being typed as far
    /// as the folders that start like it agree. The folder is listed once.
    fn complete(&self, input: &mut Input) {
        let text = input.text();
        let (before, typed) = text.split_at(text.rfind('/').map_or(0, |slash| slash + 1));
        if text == "~" {
            *input = Input::new("~/");
            return;
        }
        let hidden = self.show_hidden || typed.starts_with('.');
        let Ok(entries) = model::list_dir(&self.resolve(before), hidden, SortMode::Name) else {
            return;
        };
        let start = typed.to_lowercase();
        let folders: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.is_dir && entry.name.to_lowercase().starts_with(&start))
            .map(|entry| entry.name.as_str())
            .collect();
        let filled = match folders.as_slice() {
            [] => return,
            [only] => format!("{only}/"),
            [first, others @ ..] => common_start(first, others),
        };
        *input = Input::new(&format!("{before}{filled}"));
    }

    /// `/`: opens the search field, or hands the keyboard back to the one
    /// that is open.
    fn search(&mut self) {
        if self.pane().search().is_none() {
            let origin = self.pane().visit();
            self.pane_mut().listing = Listing::Search(Search {
                input: Input::default(),
                origin,
                walk: None,
                wanted: Vec::new(),
            });
        }
        // Up, Down and Enter act on the files from the field.
        self.focus = Focus::Files;
        self.field = Some(Field::Search);
    }

    /// A key pressed while the search field has the keyboard: what isn't
    /// typed into it moves in what was found, opens it or leaves.
    fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.leave_search();
                return;
            }
            // The keyboard moves on to what was found. With nothing typed
            // there is no search to keep.
            KeyCode::Tab | KeyCode::BackTab => {
                if self.pane().results().is_none() {
                    self.leave_search();
                }
                return;
            }
            // The menu is for what was found: the keyboard moves on to
            // that, as with Tab, and the menu opens.
            _ if opens_menu(key) => {
                if self.pane().results().is_none() {
                    self.leave_search();
                }
                self.open_menu();
                return;
            }
            KeyCode::Enter | KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                self.files_key(key)
            }
            _ => {
                let mut retyped = false;
                if let Listing::Search(search) = &mut self.pane_mut().listing {
                    let before = search.input.text();
                    search.input.handle_key(key);
                    retyped = search.input.text() != before;
                }
                if retyped {
                    self.run_search();
                }
            }
        }
        // Enter on a folder went there and left the search.
        if self.pane().search().is_some() {
            self.field = Some(Field::Search);
        }
    }

    /// Lists what the search field asks for: what was there before while
    /// it is empty, and otherwise the matches of a walk started afresh.
    fn run_search(&mut self) {
        let Some(mut search) = self.pane_mut().take_search() else {
            return;
        };
        let text = search.input.text();
        search.wanted.clear();
        // The walk under way looked for something else: dropped, it stops.
        search.walk = None;
        if text.is_empty() {
            self.revisit(&search.origin);
            self.pane_mut().listing = Listing::Search(search);
        } else {
            let folder = search.origin.path.clone();
            search.walk = Some(Walk::start(folder.clone(), &text, self.show_hidden));
            let pane = self.pane_mut();
            pane.show(folder, Vec::new(), None);
            pane.listing = Listing::Search(search);
            self.refresh_ancestors();
            self.message = None;
        }
    }

    /// Ends the search and shows what was there before it, the cursor
    /// where it was.
    fn leave_search(&mut self) {
        self.field = None;
        if let Some(search) = self.pane_mut().take_search() {
            self.revisit(&search.origin);
        }
    }

    /// Enter in a dialog: does what it asked about.
    fn answer(&mut self, dialog: Dialog) {
        match dialog {
            Dialog::Rename { path, input } => match ops::rename(&path, &input.text()) {
                Ok(Some(change)) => {
                    let (old, new) = (change.name(), input.text());
                    self.changed(change, format!("Renamed {old} to {}.", new.trim()));
                }
                Ok(None) => {}
                Err(error) => self.fail(error.to_string()),
            },
            Dialog::NewFolder { folder, input } => match ops::new_folder(&folder, &input.text()) {
                Ok(Some(change)) => {
                    let text = format!("Made {}.", change.name());
                    self.changed(change, text);
                }
                Ok(None) => {}
                Err(error) => self.fail(error.to_string()),
            },
            Dialog::Delete { paths, armed: true } => {
                let neighbour = self.neighbour(&paths);
                let deleted = ops::delete(&paths);
                self.keep_trash_records(None);
                self.relist(&[], neighbour);
                match deleted.error {
                    Some(error) => self.fail(error.to_string()),
                    None => self.note(&format!("Deleted {}.", ops::count(deleted.done))),
                }
            }
            Dialog::EmptyTrash { paths, armed: true } => self.empty_trash(paths),
            Dialog::Delete { armed: false, .. } | Dialog::EmptyTrash { armed: false, .. } => {}
        }
    }

    /// Empty Trash…, in the menu: asks before deleting for good what the
    /// trash folder holds. A trash that can't be listed is left alone.
    fn ask_empty_trash(&mut self) {
        if self.busy() {
            return;
        }
        match ops::trash_items(&self.trash_dir) {
            Ok(paths) if paths.is_empty() => self.note("The trash is already empty."),
            Ok(paths) => {
                self.dialog = Some(Dialog::EmptyTrash {
                    paths,
                    armed: false,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                self.fail(TRASH_REFUSED.to_string());
            }
            Err(error) => self.fail(read_error(&self.trash_dir, &error)),
        }
    }

    /// Deletes for good what the trash held when the question was asked.
    /// Only what is directly in the trash folder is ever deleted here.
    fn empty_trash(&mut self, mut paths: Vec<PathBuf>) {
        paths.retain(|path| path.parent() == Some(self.trash_dir.as_path()));
        let deleted = ops::delete(&paths);
        self.keep_trash_records(None);
        if self.in_trash() && !self.pane().cwd.is_dir() {
            // The pane was in a folder that went with the rest.
            let trash = self.trash_dir.clone();
            self.show_folder(&trash, None);
        }
        self.relist(&[], None);
        match deleted.error {
            Some(error) => self.fail(error.to_string()),
            None => {
                let items = ops::count(deleted.done);
                self.note(&format!("Emptied the trash: {items} deleted."));
            }
        }
    }

    /// Where an item in the trash came from and when, if this app put it there.
    pub fn trashed(&self, path: &Path) -> Option<&Trashed> {
        self.trashed.iter().find(|record| record.at == path)
    }

    /// Brings the records of what is in the trash up to date after a change
    /// to files: what `change` left in the trash is on record from now on,
    /// with where it was before, and what is no longer there is forgotten.
    /// The file is written when that changed anything.
    fn keep_trash_records(&mut self, change: Option<&Change>) {
        let before = self.trashed.clone();
        // To the second, which is as fine as the file keeps it.
        let now = SystemTime::now().duration_since(UNIX_EPOCH);
        let when = UNIX_EPOCH + Duration::from_secs(now.map_or(0, |now| now.as_secs()));
        let steps = change.map(|change| change.steps.as_slice()).unwrap_or(&[]);
        for step in steps {
            let (at, from) = match step {
                Step::Trashed { from, to } => (to, from),
                // A copy or a new folder that was undone waits in the trash.
                Step::Created {
                    at,
                    parked: Some(parked),
                } => (parked, at),
                // A restore that was undone: back in the trash, from where
                // it had been restored to. A new name given inside the
                // trash is no place it came from.
                Step::Moved { from, to }
                    if from.starts_with(&self.trash_dir) && !to.starts_with(&self.trash_dir) =>
                {
                    (from, to)
                }
                _ => continue,
            };
            self.trashed.retain(|record| record.at != *at);
            self.trashed.push(Trashed {
                at: at.clone(),
                from: from.clone(),
                when,
            });
        }
        // Whichever way the change went, only what is in the trash now counts.
        self.trashed
            .retain(|record| std::fs::symlink_metadata(&record.at).is_ok());
        if self.trashed != before
            && let Some(dir) = &self.state_dir
        {
            // Without its record an item can still be deleted or moved by
            // hand: no reason to interrupt.
            let _ = state::save_trashed(dir, &self.trashed);
        }
    }

    /// Restore, in the trash's menu: moves the selection back to where it
    /// was before it was moved to the trash.
    fn restore(&mut self) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        let neighbour = self.neighbour(&paths);
        let place = |path: &PathBuf| self.trashed(path).map(|record| record.from.clone());
        let items: Vec<_> = paths
            .iter()
            .map(|path| (path.clone(), place(path)))
            .collect();
        let report = ops::restore(&items, &self.trash_dir);
        self.settle(report, neighbour);
    }

    /// Compress, in the menu: zips the selection into one archive beside it.
    fn compress(&mut self) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        let name = match ops::archive_name(&paths) {
            Ok(name) => name,
            Err(error) => return self.fail(error.to_string()),
        };
        let zip = self.system.archiver(&paths);
        self.start(format!("Compressing into {name}…"), move |cancel| {
            ops::compress(&paths, cancel, |archive| zip(archive, cancel))
        });
    }

    /// Open Terminal Here, in the menu: a terminal in the folder under the
    /// cursor, in the folder of the file under it, or else in the folder
    /// that is showing.
    fn open_terminal(&mut self) {
        let folder = match self.selected() {
            Some(entry) if entry.is_dir => Some(entry.path.as_path()),
            Some(entry) => entry.path.parent(),
            None => Some(self.pane().cwd.as_path()),
        };
        let folder = folder.map(Path::to_path_buf);
        self.terminal_in(folder);
    }

    /// Open Terminal Here, in the folder's own menu: a terminal in the
    /// folder that is showing, whatever is under the cursor.
    fn open_terminal_here(&mut self) {
        let folder = self.pane().cwd.clone();
        self.terminal_in(Some(folder));
    }

    fn terminal_in(&mut self, folder: Option<PathBuf>) {
        let started = folder.is_some_and(|folder| self.system.open_terminal(&folder).is_ok());
        if !started {
            self.fail("Couldn't start a terminal.".to_string());
        }
    }

    /// What the file commands act on: everything selected in the active
    /// pane. One method for all of them.
    fn selection(&self) -> Vec<PathBuf> {
        if self.focus == Focus::Sidebar {
            return Vec::new();
        }
        let selected = self.pane().selection();
        selected.map(|entry| entry.path.clone()).collect()
    }

    /// Says to wait when file work is under way: changes to files are
    /// refused meanwhile, not queued behind it.
    fn busy(&mut self) -> bool {
        if self.work.is_some() {
            self.note("Try again once the work under way is done.");
        }
        self.work.is_some()
    }

    /// Ctrl+C and Ctrl+X: remembers the selection for Ctrl+V.
    fn take(&mut self, cut: bool) {
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        let what = if cut { "move" } else { "paste" };
        self.note(&format!("{} ready to {what}.", ops::count(paths.len())));
        self.clipboard = Some(Clipboard { paths, cut });
    }

    /// Ctrl+V: copies what was taken into the pane's folder, or moves it there
    /// when it was cut.
    fn paste(&mut self) {
        if self.busy() {
            return;
        }
        if !self.pane().is_folder() {
            self.note("Files can't be pasted here.");
            return;
        }
        let folder = self.pane().cwd.clone();
        self.paste_into(folder);
    }

    /// Paste Into Folder, in the menu: Ctrl+V with the folder under the
    /// cursor in place of the one that is showing.
    fn paste_into_selected(&mut self) {
        if self.busy() {
            return;
        }
        let folder = self.selected().filter(|entry| entry.is_dir);
        if let Some(folder) = folder.map(|entry| entry.path.clone()) {
            self.paste_into(folder);
        }
    }

    /// Copies what was taken into a folder, or moves it there when it was cut.
    fn paste_into(&mut self, folder: PathBuf) {
        let Some(clipboard) = &self.clipboard else {
            self.note("There's nothing to paste.");
            return;
        };
        let (paths, cut) = (clipboard.paths.clone(), clipboard.cut);
        let what = ops::count(paths.len());
        if cut {
            // A cut is pasted once.
            self.clipboard = None;
            self.start(format!("Moving {what}…"), move |cancel| {
                ops::move_into(&paths, &folder, cancel)
            });
        } else {
            self.start(format!("Copying {what}…"), move |cancel| {
                ops::copy_into(&paths, &folder, cancel)
            });
        }
    }

    fn duplicate(&mut self) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        let what = match paths.as_slice() {
            [] => return,
            [one] => name_of(one),
            many => ops::count(many.len()),
        };
        self.start(format!("Duplicating {what}…"), move |cancel| {
            ops::duplicate(&paths, cancel)
        });
    }

    /// Delete: unpins in the sidebar and moves the selection to the trash
    /// everywhere else, except in the trash itself, where all that is left
    /// to do with it is to delete it for good.
    fn delete_key(&mut self) {
        if self.focus == Focus::Sidebar {
            self.unpin_place();
        } else if self.in_trash() {
            self.ask_delete();
        } else {
            self.trash();
        }
    }

    /// Whether the pane shows the trash, or a folder that is in it.
    fn in_trash(&self) -> bool {
        let pane = self.pane();
        !pane.recents() && pane.cwd.starts_with(&self.trash_dir)
    }

    /// Moves the selection to the trash.
    fn trash(&mut self) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        let neighbour = self.neighbour(&paths);
        let report = ops::trash(&paths, &self.trash_dir);
        self.settle(report, neighbour);
    }

    /// Shift+Delete: asks before deleting for good.
    fn ask_delete(&mut self) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        if !paths.is_empty() {
            self.dialog = Some(Dialog::Delete {
                paths,
                armed: false,
            });
        }
    }

    /// F2: asks for the selected item's new name, the cursor before its extension.
    fn ask_rename(&mut self) {
        if self.busy() {
            return;
        }
        // One name can only go to one item.
        let selection = self.selection();
        let [path] = selection.as_slice() else { return };
        let entries = &self.pane().entries;
        let Some(entry) = entries.iter().find(|entry| entry.path == *path) else {
            return;
        };
        let (stem, _) = ops::split_name(&entry.name, entry.is_dir);
        let mut input = Input::new(&entry.name);
        input.set_cursor(stem.chars().count());
        self.dialog = Some(Dialog::Rename {
            path: entry.path.clone(),
            input,
        });
    }

    fn ask_new_folder(&mut self) {
        if self.busy() {
            return;
        }
        if !self.pane().is_folder() {
            self.note("Folders can't be made here.");
            return;
        }
        self.dialog = Some(Dialog::NewFolder {
            folder: self.pane().cwd.clone(),
            input: Input::new("New Folder"),
        });
    }

    /// Ctrl+Shift+C: the selection's path, or the folder's when nothing is
    /// selected, as text for other apps.
    fn copy_path(&mut self) {
        let mut paths = self.selection();
        if paths.is_empty() {
            if !self.pane().is_folder() {
                // Not a folder: it has no path of its own.
                return;
            }
            paths.push(self.pane().cwd.clone());
        }
        let lines: Vec<String> = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        match self.system.copy_text(&lines.join("\n")) {
            Ok(()) => self.note("Copied the path."),
            Err(_) => self.fail("The path couldn't be copied.".to_string()),
        }
    }

    fn undo(&mut self) {
        if self.busy() {
            return;
        }
        match self.history.undo(&self.trash_dir) {
            Ok(Some(change)) => {
                self.keep_trash_records(Some(&change));
                self.relist(&change.origins(), None);
                self.note(&told("Undid", &change));
            }
            Ok(None) => self.note("There's nothing to undo."),
            Err(error) => {
                self.keep_trash_records(None);
                self.relist(&[], None);
                self.fail(error.to_string());
            }
        }
    }

    fn redo(&mut self) {
        if self.busy() {
            return;
        }
        match self.history.redo(&self.trash_dir) {
            Ok(Some(change)) => {
                self.keep_trash_records(Some(&change));
                self.relist(&change.landed(), None);
                self.note(&told("Redid", &change));
            }
            Ok(None) => self.note("There's nothing to redo."),
            Err(error) => {
                self.keep_trash_records(None);
                self.relist(&[], None);
                self.fail(error.to_string());
            }
        }
    }

    /// Runs file work on a thread of its own, so a big folder never freezes
    /// the window. `tick` takes in what it did.
    fn start(&mut self, label: String, job: impl FnOnce(&AtomicBool) -> Report + Send + 'static) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, results) = mpsc::channel();
        let flag = cancel.clone();
        let worker = thread::Builder::new()
            .name("file work".to_string())
            .spawn(move || {
                // Nobody listening any more means the app is closing.
                let _ = sender.send(job(&flag));
            });
        match worker {
            Ok(_) => {
                self.message = None;
                self.work = Some(Work {
                    label,
                    cancel,
                    results,
                });
            }
            Err(_) => self.fail("The file work couldn't be started.".to_string()),
        }
    }

    /// Takes in the file work that finished since the last turn.
    fn collect_work(&mut self) {
        let Some(work) = &self.work else { return };
        match work.results.try_recv() {
            Ok(report) => {
                self.work = None;
                self.settle(report, None);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.work = None;
                self.relist(&[], None);
                self.fail("The file work stopped before it was done.".to_string());
            }
        }
    }

    /// Takes in what a command over several items did: logs it for undo,
    /// lists the folder again and says how it went.
    fn settle(&mut self, report: Report, neighbour: Option<PathBuf>) {
        let message = outcome(&report);
        let landed = report.change.landed();
        self.keep_trash_records(Some(&report.change));
        self.history.record(report.change);
        self.relist(&landed, neighbour);
        self.message = Some(message);
    }

    /// Takes in a change made to one item.
    fn changed(&mut self, change: Change, said: String) {
        let landed = change.landed();
        self.history.record(change);
        self.relist(&landed, None);
        self.note(&said);
    }

    /// Lists what the pane shows again and puts the cursor on the first of
    /// these that is in it: one of `paths`, what was selected before, or
    /// `neighbour`. The other pane's folder is listed again too, since what
    /// changed may show there, and that pane stays where it was.
    fn relist(&mut self, paths: &[PathBuf], neighbour: Option<PathBuf>) {
        let selected = self.selected().map(|entry| entry.path.clone());
        if self.reload() {
            let wanted = paths.iter().cloned().chain(selected).chain(neighbour);
            self.pane_mut().want(wanted.collect());
        }
        for index in 0..self.panes.len() {
            // The files used lately and what a search found are not asked
            // for again on another pane's account.
            if index != self.active && self.panes[index].is_folder() {
                let was = self.panes[index].position();
                if self.in_pane(index, Self::reload) {
                    self.panes[index].put_back(was);
                }
            }
        }
    }

    /// Does to a pane what is written for the active one.
    fn in_pane<T>(&mut self, index: usize, act: impl FnOnce(&mut Self) -> T) -> T {
        let active = std::mem::replace(&mut self.active, index);
        let done = act(self);
        self.active = active;
        done
    }

    /// Lists what every pane shows again: the sort order and the hidden
    /// files are the same in all of them.
    fn reload_all(&mut self) {
        for index in 0..self.panes.len() {
            self.in_pane(index, Self::reload);
        }
    }

    /// Ctrl+Alt+N: a second pane on the right, in the folder of the first,
    /// with the keyboard. Two are as many as there are.
    fn split(&mut self) {
        if self.panes.len() > 1 {
            return;
        }
        let folder = self.pane().cwd.clone();
        self.panes.push(Pane::new(folder.clone(), Vec::new()));
        let shown = self.in_pane(1, |app| app.show_folder(&folder, None));
        if shown {
            self.active = 1;
            self.focus = Focus::Files;
        } else {
            // Why was said; a pane with nothing to show is no use.
            self.panes.pop();
        }
    }

    /// Ctrl+W: closes the right pane. The left one has the keyboard from then on.
    fn close_split(&mut self) {
        if self.panes.len() > 1 {
            self.panes.truncate(1);
            self.active = 0;
            self.focus = Focus::Files;
        }
    }

    /// The item the cursor should fall on once these are gone: the next one
    /// that stays, or else the nearest one before.
    fn neighbour(&self, leaving: &[PathBuf]) -> Option<PathBuf> {
        let pane = self.pane();
        let stays = |entry: &&Entry| !leaving.contains(&entry.path);
        let after = pane.entries.iter().skip(pane.cursor).find(stays);
        let before = pane.entries.iter().take(pane.cursor).rev().find(stays);
        after.or(before).map(|entry| entry.path.clone())
    }

    /// Escape: stops the file work under way; with none, leaves the search
    /// that is showing, or dismisses what went wrong. With none of that to
    /// do, it leaves only the item under the cursor selected.
    fn escape(&mut self) {
        if self.work.is_none() && self.pane().search().is_some() {
            self.leave_search();
        } else if self.work.is_some() || self.message.is_some() {
            self.stop_or_dismiss();
        } else {
            self.pane_mut().collapse();
        }
    }

    /// Stops the file work under way, or dismisses what went wrong.
    fn stop_or_dismiss(&mut self) {
        if let Some(work) = &self.work {
            work.cancel.store(true, Ordering::Relaxed);
        }
        self.message = None;
    }

    /// Q: closes the window, once no file is left half copied.
    fn quit(&mut self) {
        if !self.busy() {
            self.should_quit = true;
        }
    }

    fn files_key(&mut self, key: KeyEvent) {
        let grid = self.view == ViewMode::Grid;
        let pane = self.pane();
        let columns = if grid { pane.grid_columns.max(1) } else { 1 };
        let page = (pane.page_rows.max(1) * columns) as isize;
        let held = key.modifiers;
        let ctrl = held.contains(KeyModifiers::CONTROL) && !held.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('a') if ctrl => {
                self.pane_mut().select_all();
                return;
            }
            KeyCode::Left if !grid => return self.go_up(),
            KeyCode::Right if !grid => {
                if self.selected().is_some_and(|entry| entry.is_dir) {
                    self.open_selected();
                }
                return;
            }
            KeyCode::Enter => return self.open_selected(),
            _ => {}
        }
        // The rest move the cursor. With Shift held everything between it
        // and where the range started is selected; without, only its item.
        let pane = self.pane_mut();
        let anchor = pane.anchor.unwrap_or(pane.cursor);
        match key.code {
            // In the grid the arrows only move: Left and Right never change folder.
            KeyCode::Up if grid => pane.move_row(false, columns),
            KeyCode::Down if grid => pane.move_row(true, columns),
            KeyCode::Left => pane.move_cursor(-1),
            KeyCode::Right => pane.move_cursor(1),
            KeyCode::Up => pane.move_cursor(-1),
            KeyCode::Down => pane.move_cursor(1),
            KeyCode::PageUp => pane.move_cursor(-page),
            KeyCode::PageDown => pane.move_cursor(page),
            KeyCode::Home => pane.move_cursor(isize::MIN / 2),
            KeyCode::End => pane.move_cursor(isize::MAX / 2),
            _ => return,
        }
        if held.contains(KeyModifiers::SHIFT) {
            pane.select_from(anchor);
        } else {
            pane.collapse();
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        let last = self.places().count().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1),
            KeyCode::Down => self.sidebar_cursor = (self.sidebar_cursor + 1).min(last),
            KeyCode::Enter | KeyCode::Right => self.open_place(),
            _ => {}
        }
    }

    /// Enter in the sidebar: goes to the place the keyboard is on, and
    /// hands the keyboard to the files.
    fn open_place(&mut self) {
        let place = self.places().nth(self.sidebar_cursor);
        let Some((kind, path)) = place.map(|place| (place.kind, place.path.clone())) else {
            return;
        };
        match kind {
            PlaceKind::Recents => self.open_recents(),
            _ => self.navigate(&path),
        }
        self.focus = Focus::Files;
    }

    fn details_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            self.open_selected();
        }
    }

    /// Tab and Shift+Tab: the sidebar, each pane in turn, then the details.
    /// The sidebar and the details go on showing the pane that was active.
    fn move_focus(&mut self, delta: isize) {
        let mut stops = vec![(Focus::Sidebar, None)];
        stops.extend((0..self.panes.len()).map(|pane| (Focus::Files, Some(pane))));
        if self.details_visible {
            stops.push((Focus::Details, None));
        }
        let here = |(focus, pane): &(Focus, Option<usize>)| {
            *focus == self.focus && pane.is_none_or(|pane| pane == self.active)
        };
        let index = stops.iter().position(here).unwrap_or(1) as isize;
        let (focus, pane) = stops[(index + delta).rem_euclid(stops.len() as isize) as usize];
        self.focus = focus;
        self.active = pane.unwrap_or(self.active);
        if self.focus == Focus::Sidebar {
            // A drive plugged in since the last look shows up now.
            self.rebuild_sidebar();
            self.sidebar_cursor = self.current_place().unwrap_or(self.sidebar_cursor);
        }
    }

    /// Ctrl+P: pins the pane's folder to the sidebar, or unpins it.
    fn toggle_pin(&mut self) {
        if !self.pane().is_folder() {
            self.note("Only folders can be pinned.");
            return;
        }
        let folder = self.pane().cwd.clone();
        self.toggle_pin_of(folder);
    }

    /// Pin, in the menu: Ctrl+P with the folder under the cursor in place
    /// of the one that is showing.
    fn pin_selected(&mut self) {
        let folder = self.selected().filter(|entry| entry.is_dir);
        if let Some(folder) = folder.map(|entry| entry.path.clone()) {
            self.toggle_pin_of(folder);
        }
    }

    /// Pins a folder to the sidebar, or unpins it when it is pinned.
    fn toggle_pin_of(&mut self, folder: PathBuf) {
        // What Home lists is read again, so a folder made since counts.
        self.rebuild_sidebar();
        let name = places::label(&folder);
        let mut pins = self.pins.clone();
        let under_home = |place: &Place| place.kind == PlaceKind::Home && place.path == folder;
        if let Some(index) = pins.iter().position(|pin| *pin == folder) {
            pins.remove(index);
        } else if self.places().any(under_home) {
            self.note(&format!("{name} is already under Home."));
            return;
        } else {
            pins.push(folder);
        }
        self.set_pins(pins, &name);
    }

    /// Delete in the sidebar: unpins the pinned place the keyboard is on.
    fn unpin_place(&mut self) {
        let place = self.places().nth(self.sidebar_cursor);
        let Some(place) = place.filter(|place| place.kind == PlaceKind::Pinned) else {
            return;
        };
        let (path, name) = (place.path.clone(), place.label.clone());
        let pins = self.pins.iter().filter(|pin| **pin != path);
        self.set_pins(pins.cloned().collect(), &name);
    }

    /// Saves the pinned folders and lists them in the sidebar. Pins that
    /// can't be saved stay as they were; `name` is the folder they changed for.
    fn set_pins(&mut self, pins: Vec<PathBuf>, name: &str) {
        let saved = match &self.state_dir {
            Some(dir) => state::save_pins(dir, &pins),
            None => Ok(()),
        };
        match saved {
            Ok(()) => {
                self.pins = pins;
                self.rebuild_sidebar();
            }
            Err(_) => self.fail(format!("Couldn't save the pin to {name}.")),
        }
    }

    fn open_selected(&mut self) {
        let Some(entry) = self.selected() else { return };
        let (path, name, is_dir) = (entry.path.clone(), entry.name.clone(), entry.is_dir);
        if is_dir {
            self.navigate(&path);
        } else if self.system.open(&path).is_err() {
            self.fail(format!("No app is set up to open {name}."));
        }
    }

    /// Shows the selected item in a window of its own, or closes that window.
    fn preview_selected(&mut self) {
        let Some(entry) = self.selected() else { return };
        let (path, name) = (entry.path.clone(), entry.name.clone());
        if self.system.preview(&path).is_err() {
            self.fail(format!("{name} couldn't be previewed."));
        }
    }

    /// Goes to a folder, remembering where we were for Back.
    pub fn navigate(&mut self, path: &Path) {
        self.go_to(path, None);
    }

    /// Goes to a folder with the cursor on the item named, remembering
    /// where we were for Back. False when the folder couldn't be shown.
    fn go_to(&mut self, path: &Path, select: Option<&str>) -> bool {
        let from = self.pane().visit();
        if from.recents || path != from.path {
            let shown = self.show_folder(path, select);
            if shown {
                let pane = self.pane_mut();
                pane.back.push(from);
                pane.forward.clear();
            }
            return shown;
        }
        // Already there: at most a search to leave and the cursor to move.
        if self.pane().search().is_some() {
            self.leave_search();
        }
        if let Some(name) = select {
            self.pane_mut().select(name);
        }
        true
    }

    /// Shows the files used lately in place of a folder, remembering where
    /// we were for Back.
    fn open_recents(&mut self) {
        let from = self.pane().visit();
        if from.recents {
            // Already there, unless a search is in the way.
            self.leave_search();
            return;
        }
        self.show_recents(None);
        let pane = self.pane_mut();
        pane.back.push(from);
        pane.forward.clear();
    }

    fn go_up(&mut self) {
        let from = self.pane().visit();
        if from.recents {
            // Recents hang off the home folder.
            self.go_home();
            return;
        }
        let Some(parent) = from.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let child = from
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        if self.show_folder(&parent, child.as_deref()) {
            let pane = self.pane_mut();
            pane.back.push(from);
            pane.forward.clear();
        }
    }

    fn go_home(&mut self) {
        let home = self.home.clone();
        self.navigate(&home);
    }

    fn go_back(&mut self) {
        let from = self.pane().visit();
        let Some(to) = self.pane_mut().back.pop() else {
            return;
        };
        if self.revisit(&to) {
            self.pane_mut().forward.push(from);
        } else {
            self.pane_mut().back.push(to);
        }
    }

    fn go_forward(&mut self) {
        let from = self.pane().visit();
        let Some(to) = self.pane_mut().forward.pop() else {
            return;
        };
        if self.revisit(&to) {
            self.pane_mut().back.push(from);
        } else {
            self.pane_mut().forward.push(to);
        }
    }

    fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        for index in 0..self.panes.len() {
            self.in_pane(index, Self::refresh_ancestors);
        }
        self.remember();
    }

    fn set_tile(&mut self, tile: TileSize) {
        self.tile = tile;
        self.remember();
    }

    /// Saves the view and tile size for the next launch.
    fn remember(&self) {
        if let Some(dir) = &self.state_dir {
            let saved = Saved {
                view: self.view,
                tile: self.tile,
            };
            // A view that isn't remembered is no reason to interrupt.
            let _ = state::save(dir, saved);
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.reload_all();
    }

    fn toggle_details(&mut self) {
        self.details_visible = !self.details_visible;
        if !self.details_visible && self.focus == Focus::Details {
            self.focus = Focus::Files;
        }
    }

    fn set_sort(&mut self, sort: SortMode) {
        self.sort = sort;
        self.reload_all();
        self.note(sort.tip());
    }

    /// F5: reads the sidebar's places and what the pane shows again.
    fn refresh(&mut self) {
        self.rebuild_sidebar();
        self.reload();
    }

    /// Lists what the pane shows again, keeping the selection on the same
    /// item. False when it couldn't be listed.
    fn reload(&mut self) -> bool {
        if self.pane().search().is_some() {
            self.run_search();
            return true;
        }
        let here = self.pane().visit();
        self.revisit(&here)
    }

    /// Shows what a visit was of: a folder, or the files used lately.
    fn revisit(&mut self, visit: &Visit) -> bool {
        if visit.recents {
            self.show_recents(visit.selected.as_deref());
            true
        } else {
            self.show_folder(&visit.path, visit.selected.as_deref())
        }
    }

    /// Lists a folder into the active pane; on failure says why and stays put.
    fn show_folder(&mut self, path: &Path, select: Option<&str>) -> bool {
        let is_trash = path == self.trash_dir;
        let listed = match model::list_dir(path, self.show_hidden, self.sort) {
            // A trash nothing was ever moved to is an empty one.
            Err(error) if is_trash && error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            listed => listed,
        };
        match listed {
            Ok(entries) => {
                self.pane_mut().show(path.to_path_buf(), entries, select);
                self.refresh_ancestors();
                self.message = None;
                true
            }
            Err(error) if is_trash && error.kind() == io::ErrorKind::PermissionDenied => {
                self.fail(TRASH_REFUSED.to_string());
                false
            }
            Err(error) => {
                self.fail(read_error(path, &error));
                false
            }
        }
    }

    /// Asks Spotlight for the files used lately and lists them into the
    /// active pane, the newest first whatever the sort order is.
    fn show_recents(&mut self, select: Option<&str>) {
        let found = self.system.recents(RECENTS_LIMIT);
        // One that went since Spotlight last looked is simply not listed.
        let entries = found
            .into_iter()
            .filter_map(|path| Entry::from_path(path).ok());
        let entries = entries.collect();
        let home = self.home.clone();
        let pane = self.pane_mut();
        pane.show(home, entries, select);
        pane.listing = Listing::Recents;
        self.refresh_ancestors();
        self.message = None;
    }

    /// Lists the folders above the active pane's again, when the view shows
    /// them. What a search found is a column on its own.
    fn refresh_ancestors(&mut self) {
        let ancestors = if self.view == ViewMode::Columns && self.pane().results().is_none() {
            self.list_ancestors(&self.pane().cwd)
        } else {
            Vec::new()
        };
        self.pane_mut().ancestors = ancestors;
    }

    /// One listing for each folder from home (or `/` outside it) down to the parent of `cwd`.
    fn list_ancestors(&self, cwd: &Path) -> Vec<Ancestor> {
        let top = if cwd.starts_with(&self.home) {
            self.home.as_path()
        } else {
            Path::new("/")
        };
        let mut ancestors: Vec<Ancestor> = cwd
            .ancestors()
            .zip(cwd.ancestors().skip(1))
            .take_while(|(_, folder)| folder.starts_with(top))
            .map(|(child, folder)| {
                // A folder that can't be read is an empty column.
                let entries =
                    model::list_dir(folder, self.show_hidden, self.sort).unwrap_or_default();
                let on_path = entries.iter().position(|entry| entry.path == child);
                Ancestor { entries, on_path }
            })
            .collect();
        ancestors.reverse();
        ancestors
    }

    fn note(&mut self, text: &str) {
        self.message = Some(Message {
            text: text.to_string(),
            is_error: false,
        });
    }

    fn fail(&mut self, text: String) {
        self.message = Some(Message {
            text,
            is_error: true,
        });
    }
}

/// Whether a key asks for the menu: Alt+Enter, or the Menu key where a
/// keyboard has one and the terminal sends it.
fn opens_menu(key: KeyEvent) -> bool {
    let alt_enter = key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::ALT);
    alt_enter || key.code == KeyCode::Menu
}

/// How a command over several items went, for the band.
fn outcome(report: &Report) -> Message {
    if let Some(error) = &report.error {
        return Message {
            text: error.to_string(),
            is_error: true,
        };
    }
    let done = ops::count(report.done);
    let text = if report.stopped && report.change.kind == Kind::Compress {
        // One archive is made whole or not at all: there is no part to count.
        "Stopped compressing.".to_string()
    } else if report.stopped {
        let asked = ops::count(report.asked);
        format!("Stopped after {} of {asked}.", report.done)
    } else if report.done == 0 && report.skipped > 0 {
        "It is already in this folder.".to_string()
    } else {
        match report.change.kind {
            Kind::Copy => format!("Copied {done}."),
            Kind::Duplicate => format!("Duplicated {done}."),
            Kind::Trash => format!("Moved {done} to the trash."),
            Kind::Compress => format!("Compressed into {}.", report.change.name()),
            Kind::Restore => format!("Restored {done}."),
            Kind::Move | Kind::Rename | Kind::NewFolder => format!("Moved {done}."),
        }
    };
    Message {
        text,
        is_error: false,
    }
}

/// What the band says after an undo ("Undid") or a redo ("Redid").
fn told(verb: &str, change: &Change) -> String {
    let items = ops::count(change.steps.len());
    let name = change.name();
    match change.kind {
        Kind::Copy => format!("{verb} copying {items}."),
        Kind::Move => format!("{verb} moving {items}."),
        Kind::Trash => format!("{verb} moving {items} to the trash."),
        Kind::Duplicate if change.steps.len() > 1 => format!("{verb} duplicating {items}."),
        Kind::Duplicate => format!("{verb} duplicating {name}."),
        Kind::Rename => format!("{verb} renaming {name}."),
        Kind::NewFolder => format!("{verb} making {name}."),
        Kind::Compress => format!("{verb} compressing into {name}."),
        Kind::Restore => format!("{verb} restoring {items}."),
    }
}

/// What several names start with alike, whatever their case, the way the
/// first one spells it.
fn common_start(first: &str, others: &[&str]) -> String {
    let alike = |(a, b): &(char, char)| a.to_lowercase().eq(b.to_lowercase());
    let shared = |other: &&str| first.chars().zip(other.chars()).take_while(alike).count();
    let length = others.iter().map(shared).min().unwrap_or(0);
    first.chars().take(length).collect()
}

/// An item's name as the lists show it.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_error(path: &Path, error: &io::Error) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match error.kind() {
        io::ErrorKind::PermissionDenied => format!("{name}: Permission was refused."),
        io::ErrorKind::NotFound => format!("{name} doesn't exist."),
        io::ErrorKind::NotADirectory => format!("{name} isn't a folder."),
        _ => format!("{name} couldn't be read."),
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::system::Archiver;
    use crate::thumbs::testing::FakeThumbnailer;
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[derive(Clone, Default)]
    pub struct FakeSystem {
        pub opened: Rc<RefCell<Vec<PathBuf>>>,
        /// Every call to `preview`: an open window is closed by the next one.
        pub previewed: Rc<RefCell<Vec<PathBuf>>>,
        /// Every text put on the system clipboard.
        pub copied: Rc<RefCell<Vec<String>>>,
        /// What Spotlight answers when asked for the files used lately.
        pub recents: Rc<RefCell<Vec<PathBuf>>>,
        /// How many times it was asked.
        pub asked: Rc<Cell<usize>>,
        /// Every folder a terminal was opened in.
        pub terminals: Rc<RefCell<Vec<PathBuf>>>,
        /// Whether no terminal can be started.
        pub no_terminal: Rc<Cell<bool>>,
        /// How the stand-in for the program that zips behaves.
        pub archiving: Rc<Cell<Archiving>>,
    }

    /// What the stand-in for the program that zips does with an archive. One
    /// it writes holds the names of what went in, a line each.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Archiving {
        #[default]
        Works,
        /// Writes half an archive, then fails.
        Fails,
        /// Writes half an archive, then goes on until it is stopped.
        Hangs,
    }

    impl System for FakeSystem {
        fn open(&self, path: &Path) -> io::Result<()> {
            self.opened.borrow_mut().push(path.to_path_buf());
            Ok(())
        }

        fn preview(&self, path: &Path) -> io::Result<()> {
            self.previewed.borrow_mut().push(path.to_path_buf());
            Ok(())
        }

        fn copy_text(&self, text: &str) -> io::Result<()> {
            self.copied.borrow_mut().push(text.to_string());
            Ok(())
        }

        fn recents(&self, limit: usize) -> Vec<PathBuf> {
            self.asked.set(self.asked.get() + 1);
            self.recents.borrow().iter().take(limit).cloned().collect()
        }

        fn open_terminal(&self, folder: &Path) -> io::Result<()> {
            if self.no_terminal.get() {
                return Err(io::Error::other("no terminal started"));
            }
            self.terminals.borrow_mut().push(folder.to_path_buf());
            Ok(())
        }

        fn archiver(&self, items: &[PathBuf]) -> Archiver {
            let names: Vec<String> = items.iter().map(|item| name_of(item)).collect();
            let archiving = self.archiving.get();
            Box::new(move |archive, stop| {
                if archiving == Archiving::Works {
                    return fs::write(archive, names.join("\n"));
                }
                fs::write(archive, "half an archive")?;
                while archiving == Archiving::Hangs && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(match archiving {
                    Archiving::Hangs => io::ErrorKind::Interrupted.into(),
                    _ => io::Error::other("the archive couldn't be written"),
                })
            })
        }
    }

    pub struct Fixture {
        pub app: App,
        pub system: FakeSystem,
        pub home: PathBuf,
        /// The trash the app was given: a folder of the fixture's own.
        pub trash: PathBuf,
        /// Where the app looks for drives: a folder of the fixture's own,
        /// not there until a test mounts something.
        pub volumes: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// A home folder with `Documents/`, `Pictures/{Trip/, a.jpg}`, `notes.txt` and `.hidden`.
    pub fn fixture() -> Fixture {
        fixture_with(None)
    }

    /// The fixture drawing real thumbnails: a fake thumbnailer shown through half blocks.
    pub fn pictures() -> (Fixture, Arc<FakeThumbnailer>) {
        let thumbnailer = Arc::new(FakeThumbnailer::default());
        let thumbs = Thumbs::new(crate::thumbs::testing::picker(), thumbnailer.clone());
        (fixture_with(Some(thumbs)), thumbnailer)
    }

    /// Lets the file work under way finish and takes in what it did.
    pub fn finish_work(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.tick();
            if app.working().is_none() {
                return;
            }
            assert!(Instant::now() < deadline, "the file work never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Lets the search under way look everywhere and takes in what it found.
    pub fn finish_search(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.tick();
            if app
                .pane()
                .search()
                .is_none_or(|search| !search.is_running())
            {
                return;
            }
            assert!(Instant::now() < deadline, "the search never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Lets the worker finish what the last frames asked for and takes it in.
    pub fn settle(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.tick();
            if app.thumbs.as_ref().is_none_or(Thumbs::is_idle) {
                return;
            }
            assert!(Instant::now() < deadline, "the thumbnails never arrived");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn fixture_with(thumbs: Option<Thumbs>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        fs::create_dir(home.join("Documents")).unwrap();
        fs::create_dir_all(home.join("Pictures").join("Trip")).unwrap();
        fs::write(home.join("Pictures").join("a.jpg"), "jpg").unwrap();
        fs::write(home.join("notes.txt"), "hello").unwrap();
        fs::write(home.join(".hidden"), "").unwrap();
        let system = FakeSystem::default();
        let trash = home.join(".Trash");
        let volumes = home.join(".Volumes");
        let config = Config {
            home: home.clone(),
            start: home.clone(),
            date_style: DateStyle::MonthFirst,
            state_dir: None,
            thumbs,
            trash_dir: trash.clone(),
            volumes_dir: volumes.clone(),
        };
        let mut app = App::new(config, Box::new(system.clone()));
        // The tests written against the list keep it; the others switch with Ctrl+1 or Ctrl+3.
        app.view = ViewMode::List;
        Fixture {
            app,
            system,
            home,
            trash,
            volumes,
            _dir: dir,
        }
    }

    /// Starts the app in a home folder the way `main` does, remembering things in `state_dir`.
    pub fn launch(home: &Path, state_dir: Option<&Path>) -> App {
        let config = Config {
            home: home.to_path_buf(),
            start: home.to_path_buf(),
            date_style: DateStyle::MonthFirst,
            state_dir: state_dir.map(Path::to_path_buf),
            thumbs: None,
            trash_dir: home.join(".Trash"),
            volumes_dir: home.join(".Volumes"),
        };
        App::new(config, Box::new(FakeSystem::default()))
    }

    /// The fixture as a grid three tiles across and two rows tall, holding ten items:
    /// `Documents`, `Pictures`, `a.txt` to `g.txt` and `notes.txt`.
    pub fn grid_fixture() -> Fixture {
        let mut f = fixture();
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        f.app.handle_key(key(KeyCode::F(5)));
        crate::ui::testing::screen(&mut f.app, 120, 24);
        f
    }

    /// The fixture with `a.txt` to `e.txt` next to what it had: eight items
    /// in a list, `Documents`, `Pictures`, the five and `notes.txt`.
    pub fn several() -> Fixture {
        let mut f = fixture();
        for name in ["a", "b", "c", "d", "e"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        f.app.handle_key(key(KeyCode::F(5)));
        f
    }

    /// Ctrl+Alt+N: a second pane, which takes the keyboard.
    pub fn split(app: &mut App) {
        app.handle_key(ctrl_alt(KeyCode::Char('n')));
    }

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub fn ctrl_alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::ALT)
    }

    pub fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    pub fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    pub fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    /// Ctrl+Shift+letter the way most terminals send it: the capital, with Ctrl.
    pub fn ctrl_shift(letter: char) -> KeyEvent {
        ctrl(KeyCode::Char(letter.to_ascii_uppercase()))
    }

    pub fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    pub fn press(app: &mut App, codes: &[KeyCode]) {
        for code in codes {
            app.handle_key(key(*code));
        }
    }

    pub fn names(app: &App) -> Vec<String> {
        app.pane()
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn selected_name(app: &App) -> Option<&str> {
        app.selected().map(|entry| entry.name.as_str())
    }

    #[test]
    fn starts_in_the_given_folder_with_folders_first_and_hidden_files_out() {
        let f = fixture();
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(f.app.focus, Focus::Files);
    }

    #[test]
    fn starting_on_a_file_opens_its_folder_with_the_file_selected() {
        let f = fixture();
        let config = Config {
            home: f.home.clone(),
            start: f.home.join("notes.txt"),
            date_style: DateStyle::MonthFirst,
            state_dir: None,
            thumbs: None,
            trash_dir: f.trash.clone(),
            volumes_dir: f.volumes.clone(),
        };
        let app = App::new(config, Box::new(FakeSystem::default()));
        assert_eq!(app.pane().cwd, f.home);
        assert_eq!(selected_name(&app), Some("notes.txt"));
    }

    #[test]
    fn arrows_move_and_enter_opens_a_folder() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        press(
            &mut f.app,
            &[
                KeyCode::Up,
                KeyCode::Up,
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Down,
            ],
        );
        assert_eq!(selected_name(&f.app), Some("a.jpg"));
    }

    #[test]
    fn enter_opens_a_file_with_its_app() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), [f.home.join("notes.txt")]);
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn backspace_goes_to_the_parent_and_selects_where_we_came_from() {
        let mut f = fixture();
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Enter, KeyCode::Backspace],
        );
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
    }

    #[test]
    fn right_goes_into_a_folder_and_left_back_out() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        press(&mut f.app, &[KeyCode::Left]);
        assert_eq!(f.app.pane().cwd, f.home);
        // Right on a file does nothing.
        press(&mut f.app, &[KeyCode::End, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.system.opened.borrow().is_empty());
    }

    #[test]
    fn alt_arrows_walk_the_history_back_and_forward() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(selected_name(&f.app), Some("Trip"));
        f.app.handle_key(alt(KeyCode::Char('b')));
        assert_eq!(f.app.pane().cwd, f.home);
        f.app.handle_key(alt(KeyCode::Right));
        f.app.handle_key(alt(KeyCode::Char('f')));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
    }

    #[test]
    fn alt_home_goes_home() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        f.app.handle_key(alt(KeyCode::Home));
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn tab_cycles_sidebar_files_and_details() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Details);
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Sidebar);
        press(&mut f.app, &[KeyCode::BackTab, KeyCode::BackTab]);
        assert_eq!(f.app.focus, Focus::Files);
        f.app.details_visible = false;
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Sidebar);
    }

    #[test]
    fn the_sidebar_lists_home_folders_and_enter_goes_there() {
        let mut f = fixture();
        let home = &f.app.sidebar[0];
        let labels: Vec<_> = home
            .places
            .iter()
            .map(|place| place.label.as_str())
            .collect();
        assert_eq!(
            (home.title, labels),
            ("Home", vec!["Documents", "Pictures"])
        );
        press(
            &mut f.app,
            &[KeyCode::BackTab, KeyCode::Down, KeyCode::Enter],
        );
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(f.app.focus, Focus::Files);
        assert_eq!(f.app.current_place(), Some(1));
    }

    #[test]
    fn question_mark_shows_the_shortcuts_and_escape_hides_them() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        assert!(f.app.help_visible);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('q')]);
        assert_eq!(
            f.app.pane().cursor,
            0,
            "keys don't reach the files under the help"
        );
        assert!(!f.app.should_quit);
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.app.help_visible);
    }

    #[test]
    fn q_closes_the_app() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('q')]);
        assert!(f.app.should_quit);
    }

    #[test]
    fn s_cycles_the_sort_order_and_keeps_the_selection() {
        let mut f = fixture();
        fs::write(f.home.join("big.bin"), vec![0u8; 100]).unwrap();
        press(&mut f.app, &[KeyCode::F(5), KeyCode::End]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        press(&mut f.app, &[KeyCode::Char('S')]);
        assert_eq!(f.app.sort, SortMode::Size);
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", "big.bin", "notes.txt"]
        );
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(f.app.message.as_ref().unwrap().text, "Biggest first");
        press(&mut f.app, &[KeyCode::Char('s')]);
        assert_eq!(f.app.sort, SortMode::Name);
        assert_eq!(f.app.message.as_ref().unwrap().text, "Sorted by name");
    }

    #[test]
    fn a_folder_that_cannot_be_read_says_so_and_stays_put() {
        let mut f = fixture();
        let locked = f.home.join("Documents");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        press(&mut f.app, &[KeyCode::Enter]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(f.app.pane().cwd, f.home);
        let message = f.app.message.clone().unwrap();
        assert!(message.is_error);
        assert_eq!(message.text, "Documents: Permission was refused.");
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
    }

    fn ancestor_names(app: &App, column: usize) -> Vec<&str> {
        app.pane().ancestors[column]
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    #[test]
    fn the_view_starts_as_a_grid_and_ctrl_1_2_3_switch_and_remember_it() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch(&f.home, Some(state.path()));
        assert_eq!(app.view, ViewMode::Grid);
        assert!(
            !state.path().join("state").exists(),
            "nothing is written until something changes"
        );

        app.handle_key(ctrl(KeyCode::Char('3')));
        assert_eq!(app.view, ViewMode::Columns);
        assert_eq!(
            launch(&f.home, Some(state.path())).view,
            ViewMode::Columns,
            "the next launch opens in the same view"
        );
        app.handle_key(ctrl(KeyCode::Char('2')));
        assert_eq!(app.view, ViewMode::List);
        let saved = fs::read_to_string(state.path().join("state")).unwrap();
        assert_eq!(saved, "view=list\ntiles=normal\n");
        app.handle_key(ctrl(KeyCode::Char('1')));
        assert_eq!(app.view, ViewMode::Grid);
        assert_eq!(launch(&f.home, Some(state.path())).view, ViewMode::Grid);
    }

    #[test]
    fn switching_the_view_keeps_the_folder_and_the_selection() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        for digit in ['1', '3', '2'] {
            f.app.handle_key(ctrl(KeyCode::Char(digit)));
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
            assert_eq!(selected_name(&f.app), Some("a.jpg"));
        }
    }

    #[test]
    fn in_the_grid_left_and_right_move_one_item_and_up_and_down_one_row() {
        let mut f = grid_fixture();
        assert_eq!(f.app.pane().grid_columns, 3);
        press(&mut f.app, &[KeyCode::Right]);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(selected_name(&f.app), Some("c.txt"));
        press(&mut f.app, &[KeyCode::Left]);
        assert_eq!(selected_name(&f.app), Some("b.txt"));
        // Left at the start of a row goes on to the end of the row above.
        press(&mut f.app, &[KeyCode::Left]);
        assert_eq!(selected_name(&f.app), Some("a.txt"));
        press(&mut f.app, &[KeyCode::Right, KeyCode::Up]);
        assert_eq!(selected_name(&f.app), Some("Documents"));
        press(&mut f.app, &[KeyCode::Up, KeyCode::Left]);
        assert_eq!(
            selected_name(&f.app),
            Some("Documents"),
            "the first item is as far as it goes"
        );
    }

    #[test]
    fn in_the_grid_down_above_a_shorter_last_row_lands_on_the_last_item() {
        let mut f = grid_fixture();
        press(
            &mut f.app,
            &[KeyCode::Right, KeyCode::Down, KeyCode::Down, KeyCode::Down],
        );
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(
            selected_name(&f.app),
            Some("notes.txt"),
            "the last item is as far as it goes"
        );
        press(&mut f.app, &[KeyCode::Up]);
        assert_eq!(selected_name(&f.app), Some("e.txt"));
    }

    #[test]
    fn in_the_grid_left_and_right_never_leave_or_enter_folders() {
        let mut f = grid_fixture();
        press(&mut f.app, &[KeyCode::Left]);
        assert_eq!(f.app.pane().cwd, f.home, "Left on the first tile stays");
        press(&mut f.app, &[KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home, "Right on a folder moves on");
        assert_eq!(selected_name(&f.app), Some("Pictures"));
    }

    #[test]
    fn in_the_grid_home_end_and_pages_still_work_and_enter_opens() {
        let mut f = grid_fixture();
        assert_eq!(f.app.pane().page_rows, 2);
        press(&mut f.app, &[KeyCode::PageDown]);
        assert_eq!(selected_name(&f.app), Some("e.txt"), "two rows of three");
        press(&mut f.app, &[KeyCode::PageUp]);
        assert_eq!(selected_name(&f.app), Some("Documents"));
        press(&mut f.app, &[KeyCode::End]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), [f.home.join("notes.txt")]);
        press(&mut f.app, &[KeyCode::Home, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));
    }

    #[test]
    fn ctrl_h_shows_and_hides_hidden_files_keeping_the_selection() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        assert!(f.app.show_hidden);
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", ".hidden", "notes.txt"]
        );
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        assert!(!f.app.show_hidden);
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
    }

    #[test]
    fn ctrl_i_hides_the_details_and_takes_the_keyboard_back_to_the_files() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Details);
        f.app.handle_key(ctrl(KeyCode::Char('i')));
        assert!(!f.app.details_visible);
        assert_eq!(f.app.focus, Focus::Files);
        f.app.handle_key(ctrl(KeyCode::Char('i')));
        assert!(f.app.details_visible);
        assert_eq!(f.app.focus, Focus::Files);
        // From the sidebar the keyboard stays where it is.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('i')));
        assert_eq!(f.app.focus, Focus::Sidebar);
    }

    #[test]
    fn ctrl_plus_minus_and_zero_resize_the_tiles_and_remember_the_size() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch(&f.home, Some(state.path()));
        assert_eq!(app.tile, TileSize::Normal);
        let mut sizes = Vec::new();
        for c in ['=', '=', '-', '-', '-', '0', '-', '+'] {
            app.handle_key(ctrl(KeyCode::Char(c)));
            sizes.push(app.tile);
        }
        use TileSize::{Large, Normal, Small};
        assert_eq!(
            sizes,
            [Large, Large, Normal, Small, Small, Normal, Small, Normal]
        );
        app.handle_key(ctrl(KeyCode::Char('+')));
        let saved = fs::read_to_string(state.path().join("state")).unwrap();
        assert_eq!(saved, "view=grid\ntiles=large\n");
        assert_eq!(launch(&f.home, Some(state.path())).tile, Large);
    }

    #[test]
    fn in_columns_right_goes_into_a_folder_and_left_back_out() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        assert!(
            f.app.pane().ancestors.is_empty(),
            "home is the first column"
        );
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", "notes.txt"]
        );
        assert_eq!(f.app.pane().ancestors[0].on_path, Some(1));

        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(selected_name(&f.app), Some("a.jpg"));
        press(&mut f.app, &[KeyCode::Up, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        assert_eq!(f.app.pane().ancestors.len(), 2);
        assert_eq!(ancestor_names(&f.app, 1), ["Trip", "a.jpg"]);
        assert_eq!(f.app.pane().ancestors[1].on_path, Some(0));

        press(&mut f.app, &[KeyCode::Left]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(selected_name(&f.app), Some("Trip"));
        assert_eq!(f.app.pane().ancestors.len(), 1);
    }

    #[test]
    fn columns_above_are_listed_once_and_again_when_something_changes() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        fs::write(f.home.join("later.txt"), vec![0u8; 100]).unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Up]);
        crate::ui::testing::screen(&mut f.app, 120, 24);
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", "notes.txt"],
            "moving and drawing don't read the folders again"
        );

        press(&mut f.app, &[KeyCode::F(5)]);
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", "later.txt", "notes.txt"]
        );
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", ".hidden", "later.txt", "notes.txt"]
        );
        press(&mut f.app, &[KeyCode::Char('S')]);
        assert_eq!(f.app.sort, SortMode::Size);
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", "later.txt", "notes.txt", ".hidden"]
        );
        assert_eq!(f.app.pane().ancestors[0].on_path, Some(1));

        // The other views have no use for them.
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        assert!(f.app.pane().ancestors.is_empty());
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        assert_eq!(f.app.pane().ancestors.len(), 1);
    }

    #[test]
    fn space_previews_the_selected_file_and_again_closes_it() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::Char(' ')]);
        assert_eq!(*f.system.previewed.borrow(), [f.home.join("notes.txt")]);
        assert!(f.system.opened.borrow().is_empty(), "nothing is opened");
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(f.app.message, None);
        // The port closes the window it has open when it is asked again.
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert_eq!(f.system.previewed.borrow().len(), 2);
    }

    #[test]
    fn space_with_nothing_selected_does_nothing() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));
        assert!(f.app.selected().is_none());
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert!(f.system.previewed.borrow().is_empty());
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn the_newest_thumbnails_asked_for_are_made_first() {
        let (mut f, thumbnailer) = pictures();
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        press(&mut f.app, &[KeyCode::F(5)]);
        thumbnailer.hold();
        // The first page asks for a.txt to g.txt; the last one for e.txt to notes.txt.
        crate::ui::testing::screen(&mut f.app, 120, 24);
        press(&mut f.app, &[KeyCode::End]);
        crate::ui::testing::screen(&mut f.app, 120, 24);
        thumbnailer.release();
        settle(&mut f.app);

        let asked = thumbnailer.asked();
        assert_eq!(asked.len(), 8, "each file once: {asked:?}");
        // The one the worker had already picked up aside, what is on screen now
        // doesn't wait behind what scrolled away.
        let rest = &asked[1..];
        let place = |name: &str| rest.iter().position(|asked| asked == name);
        for showing in ["e.txt", "f.txt", "g.txt", "notes.txt"] {
            for scrolled_away in ["a.txt", "b.txt", "c.txt", "d.txt"] {
                if let (Some(showing), Some(scrolled_away)) = (place(showing), place(scrolled_away))
                {
                    assert!(showing < scrolled_away, "{asked:?}");
                }
            }
        }
        assert_eq!(place("notes.txt").or(Some(0)), Some(0), "{asked:?}");
    }

    #[test]
    fn columns_start_at_the_root_outside_the_home_folder() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Backspace]);
        let above = f.home.parent().unwrap();
        assert_eq!(f.app.pane().cwd, above);
        // One column for each folder from `/` down to the parent of this one.
        assert_eq!(f.app.pane().ancestors.len(), above.components().count() - 1);
        let nearest = f.app.pane().ancestors.last().unwrap();
        let on_path = &nearest.entries[nearest.on_path.unwrap()];
        assert_eq!(on_path.path, above);
    }

    // File operations. Everything below happens inside the fixture's own
    // folder, with a trash of its own.

    fn said(app: &App) -> (&str, bool) {
        match &app.message {
            Some(message) => (message.text.as_str(), message.is_error),
            None => ("", false),
        }
    }

    fn read(path: PathBuf) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// The names in a folder on disk, sorted and without the hidden ones
    /// (the fixture's `.hidden` and its trash); none when it isn't there.
    fn on_disk(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();
        names
    }

    /// Puts the cursor on an item of the folder that is showing.
    fn select(app: &mut App, name: &str) {
        press(app, &[KeyCode::Home]);
        while selected_name(app) != Some(name) {
            let before = app.pane().cursor;
            press(app, &[KeyCode::Down]);
            assert_ne!(app.pane().cursor, before, "{name} isn't listed");
        }
    }

    /// Empties the text field of the dialog that is open.
    fn clear_field(app: &mut App) {
        press(app, &[KeyCode::End]);
        press(app, &[KeyCode::Backspace; 40]);
    }

    fn field(app: &App) -> (String, usize) {
        match &app.dialog {
            Some(Dialog::Rename { input, .. } | Dialog::NewFolder { input, .. }) => {
                (input.text(), input.cursor())
            }
            _ => panic!("no text field is open"),
        }
    }

    #[test]
    fn ctrl_c_then_ctrl_v_copies_into_the_folder_and_never_overwrites() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(f.app.working(), Some("Copying 1 item…"));
        finish_work(&mut f.app);
        assert_eq!(f.app.working(), None);

        let documents = f.home.join("Documents");
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(
            read(f.home.join("notes.txt")),
            "hello",
            "the original stays"
        );
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(said(&f.app), ("Copied 1 item.", false));

        // A copy can be pasted again, and the one already there is left as it is.
        fs::write(documents.join("notes.txt"), "changed since").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["notes (copy).txt", "notes.txt"]);
        assert_eq!(read(documents.join("notes.txt")), "changed since");
        assert_eq!(read(documents.join("notes (copy).txt")), "hello");
        assert_eq!(selected_name(&f.app), Some("notes (copy).txt"));
    }

    #[test]
    fn ctrl_x_then_ctrl_v_moves_once() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(said(&f.app), ("1 item ready to move.", false));
        assert_eq!(
            read(f.home.join("notes.txt")),
            "hello",
            "a cut moves nothing yet"
        );
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(f.app.working(), Some("Moving 1 item…"));
        finish_work(&mut f.app);

        assert_eq!(read(f.home.join("Documents").join("notes.txt")), "hello");
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(said(&f.app), ("Moved 1 item.", false));

        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(f.app.working(), None);
        assert_eq!(said(&f.app), ("There's nothing to paste.", false));
        assert_eq!(on_disk(&f.home.join("Documents")), ["notes.txt"]);
    }

    #[test]
    fn a_cut_pasted_where_it_already_is_changes_nothing() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl(KeyCode::Char('x')));
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(said(&f.app), ("It is already in this folder.", false));
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));
    }

    #[test]
    fn f2_renames_in_a_dialog_that_keeps_every_key_to_itself() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        let at = f.app.pane().cursor;
        press(&mut f.app, &[KeyCode::F(2)]);
        assert_eq!(
            field(&f.app),
            ("notes.txt".to_string(), 5),
            "the cursor waits before the extension"
        );

        // Typed into the name: `q` doesn't quit and the arrows don't reach the files.
        press(
            &mut f.app,
            &[KeyCode::Char('q'), KeyCode::Down, KeyCode::Up],
        );
        press(&mut f.app, &[KeyCode::Char('?'), KeyCode::Char('s')]);
        assert!(!f.app.should_quit && !f.app.help_visible);
        assert_eq!((f.app.pane().cursor, f.app.sort), (at, SortMode::Name));
        assert_eq!(field(&f.app).0, "notesq?s.txt");
        press(&mut f.app, &[KeyCode::Backspace; 8]);
        type_text(&mut f.app, "todo");
        press(&mut f.app, &[KeyCode::Enter]);

        assert!(f.app.dialog.is_none());
        assert_eq!(read(f.home.join("todo.txt")), "hello");
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(names(&f.app), ["Documents", "Pictures", "todo.txt"]);
        assert_eq!(selected_name(&f.app), Some("todo.txt"));
        assert_eq!(said(&f.app), ("Renamed notes.txt to todo.txt.", false));

        // A folder has no extension: the cursor is at the end. Escape leaves it be.
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::F(2)]);
        assert_eq!(field(&f.app), ("Documents".to_string(), 9));
        type_text(&mut f.app, " old");
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.dialog.is_none());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "todo.txt"]);
    }

    #[test]
    fn a_rename_that_cannot_be_done_says_why_and_changes_nothing() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(said(&f.app), ("Pictures already exists here.", true));

        press(&mut f.app, &[KeyCode::F(2)]);
        type_text(&mut f.app, "/2");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(said(&f.app), ("A name can't contain \"/\".", true));

        // Unchanged, or emptied: nothing happens and nothing is said.
        press(&mut f.app, &[KeyCode::Esc, KeyCode::F(2), KeyCode::Enter]);
        assert_eq!(f.app.message, None);
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.message, None);

        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(on_disk(&f.home.join("Pictures")), ["Trip", "a.jpg"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
    }

    #[test]
    fn ctrl_shift_d_duplicates_next_to_the_original() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl_shift('d'));
        assert_eq!(f.app.working(), Some("Duplicating notes.txt…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes (copy).txt")), "hello");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(selected_name(&f.app), Some("notes (copy).txt"));
        assert_eq!(said(&f.app), ("Duplicated 1 item.", false));

        // The chord's other spelling: the small letter with Shift held.
        select(&mut f.app, "notes.txt");
        let spelled = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        f.app.handle_key(KeyEvent::new(KeyCode::Char('d'), spelled));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes (copy 2).txt")), "hello");

        // Ctrl+D without Shift is not it.
        f.app.handle_key(ctrl(KeyCode::Char('d')));
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(&f.home).len(), 5, "{:?}", on_disk(&f.home));
    }

    #[test]
    fn ctrl_shift_n_asks_for_a_name_and_makes_the_folder() {
        let mut f = fixture();
        f.app.handle_key(ctrl_shift('n'));
        assert!(matches!(f.app.dialog, Some(Dialog::NewFolder { .. })));
        assert_eq!(field(&f.app), ("New Folder".to_string(), 10));
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.dialog.is_none());
        assert!(f.home.join("New Folder").is_dir());
        assert_eq!(selected_name(&f.app), Some("New Folder"));
        assert_eq!(said(&f.app), ("Made New Folder.", false));

        // The same name again is refused; nothing is made or replaced.
        fs::write(f.home.join("New Folder").join("kept.txt"), "kept").unwrap();
        f.app.handle_key(ctrl_shift('n'));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(said(&f.app), ("New Folder already exists here.", true));
        assert_eq!(on_disk(&f.home.join("New Folder")), ["kept.txt"]);

        let spelled = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        f.app.handle_key(KeyEvent::new(KeyCode::Char('n'), spelled));
        clear_field(&mut f.app);
        type_text(&mut f.app, "Plans");
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.home.join("Plans").exists(), "Escape makes nothing");

        // Ctrl+N without Shift is not it.
        f.app.handle_key(ctrl(KeyCode::Char('n')));
        assert!(f.app.dialog.is_none());
    }

    #[test]
    fn delete_moves_the_selection_to_the_trash_and_selects_its_neighbour() {
        let mut f = fixture();
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(read(f.trash.join("Pictures").join("a.jpg")), "jpg");
        assert!(!f.home.join("Pictures").exists());
        assert_eq!(names(&f.app), ["Documents", "notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"), "the next one");
        assert_eq!(said(&f.app), ("Moved 1 item to the trash.", false));

        // The last one gone, the cursor falls on the one before.
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert_eq!(selected_name(&f.app), Some("Documents"));

        // Command+Backspace is the Mac's Delete.
        let command = KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER);
        f.app.handle_key(command);
        assert!(names(&f.app).is_empty());
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(on_disk(&f.trash), ["Documents", "Pictures", "notes.txt"]);

        // Backspace by itself still only goes to the parent folder.
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(f.app.pane().cwd, f.home.parent().unwrap());
        assert_eq!(on_disk(&f.trash).len(), 3);
    }

    #[test]
    fn shift_delete_asks_first_and_cancel_is_the_default() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        let notes = f.home.join("notes.txt");
        f.app.handle_key(shift(KeyCode::Delete));
        let Some(Dialog::Delete { paths, armed }) = &f.app.dialog else {
            panic!("it didn't ask");
        };
        assert_eq!(
            (paths.as_slice(), *armed),
            ([notes.clone()].as_slice(), false)
        );

        // Enter takes the lit choice, which is Cancel; Escape cancels too.
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.dialog.is_none());
        f.app.handle_key(shift(KeyCode::Delete));
        press(&mut f.app, &[KeyCode::Right, KeyCode::Esc]);
        assert!(f.app.dialog.is_none());
        // Keys don't reach the files under the question.
        f.app.handle_key(shift(KeyCode::Delete));
        press(
            &mut f.app,
            &[KeyCode::Up, KeyCode::Char('q'), KeyCode::Delete],
        );
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.app.should_quit);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(read(notes.clone()), "hello");
        assert_eq!(f.app.message, None);

        // Left, Right and Tab switch between the two choices.
        f.app.handle_key(shift(KeyCode::Delete));
        let mut lit = Vec::new();
        for code in [KeyCode::Right, KeyCode::Left, KeyCode::Tab] {
            press(&mut f.app, &[code]);
            lit.push(matches!(
                f.app.dialog,
                Some(Dialog::Delete { armed: true, .. })
            ));
        }
        assert_eq!(lit, [true, false, true]);
        press(&mut f.app, &[KeyCode::Enter]);

        assert!(f.app.dialog.is_none());
        assert!(!notes.exists());
        assert!(on_disk(&f.trash).is_empty(), "deleted, not trashed");
        assert_eq!(names(&f.app), ["Documents", "Pictures"]);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        assert_eq!(said(&f.app), ("Deleted 1 item.", false));
        // It can't be undone.
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));
        assert!(!notes.exists());
    }

    #[test]
    fn ctrl_shift_c_copies_the_path_as_text() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        let path = f.home.join("notes.txt").display().to_string();
        f.app.handle_key(ctrl_shift('c'));
        assert_eq!(
            f.system.copied.borrow().as_slice(),
            std::slice::from_ref(&path)
        );
        assert_eq!(said(&f.app), ("Copied the path.", false));
        // It is text for other apps, not a file to paste.
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(said(&f.app), ("There's nothing to paste.", false));

        // Ctrl+C by itself copies the file, not its path.
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(f.system.copied.borrow().len(), 1);
        let spelled = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        f.app.handle_key(KeyEvent::new(KeyCode::Char('c'), spelled));
        assert_eq!(*f.system.copied.borrow(), [path.clone(), path]);
    }

    #[test]
    fn ctrl_z_undoes_and_ctrl_shift_z_or_ctrl_y_redoes() {
        let mut f = fixture();
        let notes = f.home.join("notes.txt");
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert!(!notes.exists());

        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(read(notes.clone()), "hello");
        assert!(on_disk(&f.trash).is_empty());
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(said(&f.app), ("Undid moving 1 item to the trash.", false));

        f.app.handle_key(ctrl_shift('z'));
        assert!(!notes.exists());
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert_eq!(names(&f.app), ["Documents", "Pictures"]);
        assert_eq!(said(&f.app), ("Redid moving 1 item to the trash.", false));

        f.app.handle_key(ctrl(KeyCode::Char('z')));
        let spelled = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        f.app.handle_key(KeyEvent::new(KeyCode::Char('z'), spelled));
        assert!(!notes.exists(), "the chord's other spelling redoes too");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert!(!notes.exists(), "and so does Ctrl+Y");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(read(notes.clone()), "hello");

        // A rename and a new folder go back and forth the same way.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "todo.txt");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl_shift('n'));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(
            on_disk(&f.home),
            ["Documents", "New Folder", "Pictures", "todo.txt"]
        );

        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid making New Folder.", false));
        assert!(f.trash.join("New Folder").is_dir(), "trashed, not deleted");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid renaming notes.txt.", false));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));

        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(said(&f.app), ("Redid renaming notes.txt.", false));
        assert_eq!(selected_name(&f.app), Some("todo.txt"));
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(said(&f.app), ("Redid making New Folder.", false));
        assert_eq!(selected_name(&f.app), Some("New Folder"));
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(said(&f.app), ("There's nothing to redo.", false));
        assert_eq!(
            on_disk(&f.home),
            ["Documents", "New Folder", "Pictures", "todo.txt"]
        );
    }

    #[test]
    fn undoing_a_paste_moves_the_copy_to_the_trash() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert!(names(&f.app).is_empty());
        assert!(on_disk(&f.home.join("Documents")).is_empty());
        assert_eq!(read(f.trash.join("notes.txt")), "hello", "not deleted");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(said(&f.app), ("Undid copying 1 item.", false));
    }

    #[test]
    fn an_undo_the_world_no_longer_allows_is_refused_and_clobbers_nothing() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        fs::write(f.home.join("notes.txt"), "written since").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        let home = f.home.file_name().unwrap().to_string_lossy();
        let refused = format!("Can't undo that: notes.txt already exists in {home}.");
        assert_eq!(said(&f.app), (refused.as_str(), true));
        assert_eq!(read(f.home.join("notes.txt")), "written since");
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
    }

    #[test]
    fn while_file_work_is_under_way_other_changes_wait_and_escape_stops_it() {
        let mut f = fixture();
        // Enough files for the copy to still be going when the next keys arrive.
        let pictures = f.home.join("Pictures");
        for n in 0..600 {
            fs::write(pictures.join(format!("{n}.txt")), "x").unwrap();
        }
        select(&mut f.app, "Pictures");
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(f.app.working(), Some("Copying 1 item…"));

        let wait = ("Try again once the work under way is done.", false);
        for refused in [
            key(KeyCode::Delete),
            shift(KeyCode::Delete),
            key(KeyCode::F(2)),
            ctrl_shift('n'),
            ctrl_shift('d'),
            ctrl(KeyCode::Char('v')),
            ctrl(KeyCode::Char('z')),
            ctrl(KeyCode::Char('y')),
            key(KeyCode::Char('q')),
        ] {
            f.app.message = None;
            f.app.handle_key(refused);
            assert_eq!(said(&f.app), wait, "{refused:?}");
            assert!(f.app.dialog.is_none() && !f.app.should_quit);
        }
        assert_eq!(read(f.home.join("notes.txt")), "hello");

        press(&mut f.app, &[KeyCode::Esc]);
        finish_work(&mut f.app);
        assert_eq!(said(&f.app), ("Stopped after 0 of 1 item.", false));
        assert_eq!(on_disk(&pictures).len(), 602, "the original is whole");
        assert!(on_disk(&f.home.join("Pictures (copy)")).len() < 602);

        // What got across before the stop is on record: undo moves it to the trash.
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert!(!f.home.join("Pictures (copy)").exists());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        // And with the work over, the keys work again.
        press(&mut f.app, &[KeyCode::Char('q')]);
        assert!(f.app.should_quit);
    }

    #[test]
    fn with_the_keyboard_in_the_sidebar_the_file_keys_leave_the_files_alone() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::BackTab]);
        assert_eq!(f.app.focus, Focus::Sidebar);
        press(&mut f.app, &[KeyCode::Delete, KeyCode::F(2)]);
        f.app.handle_key(shift(KeyCode::Delete));
        f.app.handle_key(ctrl_shift('d'));
        f.app.handle_key(ctrl(KeyCode::Char('x')));
        assert!(f.app.dialog.is_none());
        assert_eq!(f.app.working(), None);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(said(&f.app), ("There's nothing to paste.", false));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
    }

    #[test]
    fn the_keys_from_before_still_do_what_they_did() {
        let mut f = fixture();
        // The new chords' letters, typed plain, change nothing.
        type_text(&mut f.app, "cxvdnzy");
        assert!(f.app.dialog.is_none());
        assert_eq!((f.app.working(), &f.app.message), (None, &None));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);

        // S and Shift+S sort, in either spelling of the shifted one.
        press(&mut f.app, &[KeyCode::Char('s')]);
        assert_eq!(f.app.sort, SortMode::Kind);
        f.app.handle_key(shift(KeyCode::Char('S')));
        assert_eq!(f.app.sort, SortMode::Name);
        press(&mut f.app, &[KeyCode::Char('S')]);
        assert_eq!(f.app.sort, SortMode::Size);
        // Escape dismisses what the band says, and Q closes the window.
        assert!(f.app.message.is_some());
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
        press(&mut f.app, &[KeyCode::Char('q')]);
        assert!(f.app.should_quit);
    }

    // The sidebar's other sections. Pins are remembered in a folder inside
    // the fixture, drives come from a folder of its own, and Spotlight is a
    // fake that answers what the test tells it to.

    type Listed = (&'static str, Vec<&'static str>);

    fn home_places() -> Listed {
        ("Home", vec!["Documents", "Pictures"])
    }

    fn views() -> Listed {
        ("Views", vec!["Recents", "Trash"])
    }

    /// Every section of the sidebar with the labels of its places.
    fn sections(app: &App) -> Vec<(&str, Vec<&str>)> {
        app.sidebar
            .iter()
            .map(|section| {
                let labels = section.places.iter().map(|place| place.label.as_str());
                (section.title, labels.collect())
            })
            .collect()
    }

    fn place_of(app: &App, label: &str) -> Option<usize> {
        app.places().position(|place| place.label == label)
    }

    /// Moves the keyboard into the sidebar and onto a place.
    fn go_to_place(app: &mut App, label: &str) {
        while app.focus != Focus::Sidebar {
            press(app, &[KeyCode::BackTab]);
        }
        let index = place_of(app, label).unwrap_or_else(|| panic!("{label} isn't listed"));
        press(app, &[KeyCode::Up; 40]);
        press(app, &vec![KeyCode::Down; index]);
        assert_eq!(app.sidebar_cursor, index);
    }

    fn open_place(app: &mut App, label: &str) {
        go_to_place(app, label);
        press(app, &[KeyCode::Enter]);
    }

    fn pin(app: &mut App) {
        app.handle_key(ctrl(KeyCode::Char('p')));
    }

    #[test]
    fn the_sidebar_holds_home_and_the_views_and_leaves_empty_sections_out() {
        let f = fixture();
        assert_eq!(sections(&f.app), [home_places(), views()]);
    }

    #[test]
    fn ctrl_p_pins_the_folder_and_ctrl_p_again_unpins_it() {
        let f = fixture();
        let state = f.home.join(".state");
        let mut app = launch(&f.home, Some(&state));
        let trip = f.home.join("Pictures").join("Trip");
        app.navigate(&trip);
        assert_eq!(app.current_place(), Some(1), "Pictures holds it");

        pin(&mut app);
        let pinned = ("Pinned", vec!["Trip"]);
        assert_eq!(sections(&app), [home_places(), pinned, views()]);
        assert_eq!(read(state.join("pinned")), format!("{}\n", trip.display()));
        assert_eq!(app.message, None);
        assert_eq!(app.current_place(), Some(2), "the pin is the deepest");
        assert_eq!(app.pane().cwd, trip);

        pin(&mut app);
        assert_eq!(sections(&app), [home_places(), views()]);
        assert_eq!(read(state.join("pinned")), "");
        assert_eq!(app.current_place(), Some(1));
    }

    #[test]
    fn pins_survive_a_restart_and_a_folder_that_is_gone_is_kept_but_not_listed() {
        let f = fixture();
        let state = f.home.join(".state");
        let trip = f.home.join("Pictures").join("Trip");
        let gone = f.home.join("Pictures").join("Gone");
        fs::create_dir(&gone).unwrap();
        let mut app = launch(&f.home, Some(&state));
        for folder in [&gone, &trip] {
            app.navigate(folder);
            pin(&mut app);
        }
        assert_eq!(sections(&app)[1], ("Pinned", vec!["Gone", "Trip"]));
        fs::remove_dir(&gone).unwrap();

        let mut again = launch(&f.home, Some(&state));
        assert_eq!(sections(&again)[1], ("Pinned", vec!["Trip"]));
        // Its line stays in the file, for when the folder comes back.
        let both = format!("{}\n{}\n", gone.display(), trip.display());
        assert_eq!(read(state.join("pinned")), both);
        again.navigate(&trip);
        pin(&mut again);
        assert_eq!(read(state.join("pinned")), format!("{}\n", gone.display()));
        assert_eq!(sections(&again), [home_places(), views()]);
    }

    #[test]
    fn what_home_already_lists_and_what_is_not_a_folder_cannot_be_pinned() {
        let f = fixture();
        let state = f.home.join(".state");
        let mut app = launch(&f.home, Some(&state));
        app.navigate(&f.home.join("Pictures"));
        pin(&mut app);
        assert_eq!(said(&app), ("Pictures is already under Home.", false));

        open_place(&mut app, "Recents");
        pin(&mut app);
        assert_eq!(said(&app), ("Only folders can be pinned.", false));
        assert_eq!(sections(&app), [home_places(), views()]);
        assert!(!state.join("pinned").exists());
    }

    #[test]
    fn a_pin_that_cannot_be_saved_says_so_and_is_not_kept() {
        let f = fixture();
        // A file is where the folder for the pins would go.
        let state = f.home.join("notes.txt").join("state");
        let mut app = launch(&f.home, Some(&state));
        app.navigate(&f.home.join("Pictures").join("Trip"));
        pin(&mut app);
        assert_eq!(said(&app), ("Couldn't save the pin to Trip.", true));
        assert_eq!(sections(&app), [home_places(), views()]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
    }

    #[test]
    fn delete_in_the_sidebar_unpins_a_pinned_place_and_leaves_the_others() {
        let f = fixture();
        let state = f.home.join(".state");
        let mut app = launch(&f.home, Some(&state));
        let trip = f.home.join("Pictures").join("Trip");
        app.navigate(&trip);
        pin(&mut app);

        for label in ["Pictures", "Recents", "Trash"] {
            go_to_place(&mut app, label);
            press(&mut app, &[KeyCode::Delete]);
        }
        assert_eq!(sections(&app)[1], ("Pinned", vec!["Trip"]));
        assert!(app.dialog.is_none());

        go_to_place(&mut app, "Trip");
        press(&mut app, &[KeyCode::Delete]);
        assert_eq!(sections(&app), [home_places(), views()]);
        assert_eq!(read(state.join("pinned")), "");
        // Only the pin went: the folder and everything else is where it was.
        assert!(trip.is_dir());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert!(on_disk(&f.home.join(".Trash")).is_empty());
        assert_eq!(app.pane().cwd, trip);
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.sidebar_cursor, 2, "on what moved up into its row");
    }

    #[test]
    fn recents_lists_what_spotlight_says_was_used_lately() {
        let mut f = fixture();
        let photo = f.home.join("Pictures").join("a.jpg");
        let notes = f.home.join("notes.txt");
        *f.system.recents.borrow_mut() =
            vec![photo.clone(), f.home.join("gone.txt"), notes.clone()];
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(f.system.asked.get(), 0, "not before it is opened");

        open_place(&mut f.app, "Recents");
        assert!(f.app.pane().recents());
        assert_eq!(names(&f.app), ["a.jpg", "notes.txt"], "Spotlight's order");
        assert_eq!(f.app.focus, Focus::Files);
        assert_eq!(f.app.current_place(), place_of(&f.app, "Recents"));

        // Enter opens a file and Space previews it.
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Enter, KeyCode::Char(' ')],
        );
        let asked_for = [notes];
        assert_eq!(*f.system.opened.borrow(), asked_for);
        assert_eq!(*f.system.previewed.borrow(), asked_for);

        // Back returns to where we were, and forward to Recents.
        f.app.handle_key(alt(KeyCode::Left));
        assert!(!f.app.pane().recents());
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        f.app.handle_key(alt(KeyCode::Right));
        assert!(f.app.pane().recents());
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
    }

    #[test]
    fn up_from_recents_is_the_home_folder() {
        let mut f = fixture();
        *f.system.recents.borrow_mut() = vec![f.home.join("notes.txt")];
        let leaving = [
            key(KeyCode::Backspace),
            key(KeyCode::Left),
            alt(KeyCode::Home),
        ];
        for leave in leaving {
            f.app.navigate(&f.home.join("Pictures"));
            open_place(&mut f.app, "Recents");
            assert!(f.app.pane().recents());
            f.app.handle_key(leave);
            assert!(!f.app.pane().recents());
            assert_eq!(f.app.pane().cwd, f.home);
            assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
            // Recents is one step back.
            f.app.handle_key(alt(KeyCode::Left));
            assert!(f.app.pane().recents());
            assert_eq!(names(&f.app), ["notes.txt"]);
        }
    }

    #[test]
    fn f5_in_recents_asks_spotlight_again_and_drawing_never_does() {
        let mut f = fixture();
        open_place(&mut f.app, "Recents");
        assert!(names(&f.app).is_empty());
        assert_eq!(f.system.asked.get(), 1);

        f.system.recents.borrow_mut().push(f.home.join("notes.txt"));
        for _ in 0..3 {
            f.app.tick();
            crate::ui::testing::screen(&mut f.app, 120, 24);
        }
        assert_eq!(f.system.asked.get(), 1);
        assert!(names(&f.app).is_empty());

        press(&mut f.app, &[KeyCode::F(5)]);
        assert_eq!(f.system.asked.get(), 2);
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert!(f.app.pane().recents());
    }

    #[test]
    fn in_recents_nothing_is_pasted_or_made_and_the_rest_acts_on_the_real_files() {
        let mut f = fixture();
        let photo = f.home.join("Pictures").join("a.jpg");
        let plan = f.home.join("Documents").join("plan.md");
        let notes = f.home.join("notes.txt");
        fs::write(&plan, "plan").unwrap();
        *f.system.recents.borrow_mut() = vec![photo.clone(), plan.clone(), notes.clone()];
        open_place(&mut f.app, "Recents");

        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(said(&f.app), ("Files can't be pasted here.", false));
        assert_eq!(f.app.working(), None);
        f.app.handle_key(ctrl_shift('n'));
        assert_eq!(said(&f.app), ("Folders can't be made here.", false));
        assert!(f.app.dialog.is_none());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);

        // The path that is copied is the file's own, wherever it is.
        f.app.handle_key(ctrl_shift('c'));
        assert_eq!(*f.system.copied.borrow(), [photo.display().to_string()]);

        // Ctrl+C takes the file for a paste somewhere else.
        select(&mut f.app, "plan.md");
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));

        // F2 renames it where it is.
        select(&mut f.app, "a.jpg");
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "b.jpg");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(on_disk(&f.home.join("Pictures")), ["Trip", "b.jpg"]);
        assert!(f.app.pane().recents());

        // Delete moves it to the trash.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert!(!notes.exists());
        assert_eq!(said(&f.app), ("Moved 1 item to the trash.", false));
        assert!(f.app.pane().recents());
        assert_eq!(names(&f.app), ["plan.md"]);

        f.app.navigate(&f.home.join("Pictures"));
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("Pictures").join("plan.md")), "plan");
        assert_eq!(read(plan), "plan");
    }

    #[test]
    fn trash_opens_the_trash_folder_where_delete_asks_to_delete_for_good() {
        let mut f = fixture();
        for name in ["notes.txt", "Pictures"] {
            select(&mut f.app, name);
            press(&mut f.app, &[KeyCode::Delete]);
        }
        open_place(&mut f.app, "Trash");
        assert_eq!(f.app.pane().cwd, f.trash);
        assert_eq!(names(&f.app), ["Pictures", "notes.txt"]);
        assert_eq!(f.app.focus, Focus::Files);
        assert_eq!(f.app.current_place(), place_of(&f.app, "Trash"));

        // Delete doesn't move it to the trash again: it asks, like Shift+Delete.
        select(&mut f.app, "notes.txt");
        let command = KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER);
        for delete in [key(KeyCode::Delete), command] {
            f.app.handle_key(delete);
            let Some(Dialog::Delete { paths, armed }) = &f.app.dialog else {
                panic!("it didn't ask");
            };
            let asked = [f.trash.join("notes.txt")];
            assert_eq!((paths.as_slice(), *armed), (asked.as_slice(), false));
            press(&mut f.app, &[KeyCode::Esc]);
            assert_eq!(on_disk(&f.trash), ["Pictures", "notes.txt"]);
        }
        press(
            &mut f.app,
            &[KeyCode::Delete, KeyCode::Right, KeyCode::Enter],
        );
        assert_eq!(on_disk(&f.trash), ["Pictures"]);
        assert_eq!(said(&f.app), ("Deleted 1 item.", false));

        // The same in a folder that is in the trash.
        press(&mut f.app, &[KeyCode::Home, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.trash.join("Pictures"));
        select(&mut f.app, "a.jpg");
        press(&mut f.app, &[KeyCode::Delete]);
        assert!(matches!(f.app.dialog, Some(Dialog::Delete { .. })));
        assert_eq!(on_disk(&f.trash.join("Pictures")), ["Trip", "a.jpg"]);
    }

    #[test]
    fn a_trash_nothing_was_ever_moved_to_opens_empty() {
        let mut f = fixture();
        assert!(!f.trash.exists());
        open_place(&mut f.app, "Trash");
        assert_eq!(f.app.pane().cwd, f.trash);
        assert!(names(&f.app).is_empty());
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn a_trash_that_cannot_be_listed_says_how_to_allow_it() {
        let mut f = fixture();
        fs::create_dir(&f.trash).unwrap();
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o000)).unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        open_place(&mut f.app, "Trash");
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        let refused = "Trash: Permission was refused. Give your terminal \
                       Full Disk Access in System Settings to see it.";
        assert_eq!(said(&f.app), (refused, true));
    }

    #[test]
    fn drives_lists_the_mounted_volumes_except_the_systems_own() {
        let mut f = fixture();
        fs::create_dir_all(f.volumes.join("USB").join("Photos")).unwrap();
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        fs::write(f.volumes.join("stray.txt"), "").unwrap();
        // What `/Volumes/Macintosh HD` is: a link to the root.
        std::os::unix::fs::symlink("/", f.volumes.join("Macintosh HD")).unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        let drives = ("Drives", vec!["Backup", "USB"]);
        assert_eq!(sections(&f.app), [home_places(), views(), drives]);

        open_place(&mut f.app, "USB");
        assert_eq!(f.app.pane().cwd, f.volumes.join("USB"));
        assert_eq!(names(&f.app), ["Photos"]);
        assert_eq!(f.app.focus, Focus::Files);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.volumes.join("USB").join("Photos"));
        assert_eq!(f.app.current_place(), place_of(&f.app, "USB"));
    }

    #[test]
    fn the_sidebar_is_read_again_on_f5_and_when_the_keyboard_moves_into_it() {
        let mut f = fixture();
        fs::create_dir(f.home.join("Music")).unwrap();
        f.app.tick();
        crate::ui::testing::screen(&mut f.app, 120, 24);
        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(sections(&f.app), [home_places(), views()], "not yet");
        press(&mut f.app, &[KeyCode::F(5)]);
        let home = ("Home", vec!["Documents", "Music", "Pictures"]);
        assert_eq!(sections(&f.app), [home.clone(), views()]);

        // A drive plugged in shows up when the keyboard gets there.
        fs::create_dir_all(f.volumes.join("USB")).unwrap();
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Details);
        assert_eq!(sections(&f.app), [home.clone(), views()]);
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Sidebar);
        assert_eq!(sections(&f.app), [home, views(), ("Drives", vec!["USB"])]);
    }

    // The path field (Ctrl+L) and the search (/). Both live in the top bar
    // and take the keyboard while they are open.

    fn path_field(app: &App) -> (String, usize) {
        match &app.field {
            Some(Field::Path(input)) => (input.text(), input.cursor()),
            _ => panic!("the path field isn't open"),
        }
    }

    fn open_path_field(app: &mut App) {
        app.handle_key(ctrl(KeyCode::Char('l')));
    }

    /// Ctrl+L, a path typed over what the field held, Enter.
    fn go_to_typed(app: &mut App, path: &str) {
        open_path_field(app);
        clear_field(app);
        type_text(app, path);
        press(app, &[KeyCode::Enter]);
    }

    /// Types over what the path field holds, presses Tab and says what it holds then.
    fn completed(app: &mut App, typed: &str) -> String {
        clear_field(app);
        type_text(app, typed);
        press(app, &[KeyCode::Tab]);
        let (text, cursor) = path_field(app);
        assert_eq!(cursor, text.chars().count(), "the cursor ends up after it");
        text
    }

    #[test]
    fn ctrl_l_opens_a_field_holding_the_panes_path_with_the_cursor_after_it() {
        let mut f = fixture();
        open_path_field(&mut f.app);
        assert_eq!(path_field(&f.app), ("~".to_string(), 1));
        press(&mut f.app, &[KeyCode::Esc]);

        f.app.navigate(&f.home.join("Pictures").join("Trip"));
        open_path_field(&mut f.app);
        assert_eq!(path_field(&f.app), ("~/Pictures/Trip".to_string(), 15));
        press(&mut f.app, &[KeyCode::Esc]);

        // Outside the home folder a path is spelled out.
        let outside = tempfile::tempdir().unwrap();
        let outside = outside.path().canonicalize().unwrap();
        f.app.navigate(&outside);
        open_path_field(&mut f.app);
        let spelled = outside.display().to_string();
        let length = spelled.chars().count();
        assert_eq!(path_field(&f.app), (spelled, length));
    }

    #[test]
    fn enter_in_the_path_field_goes_where_the_path_leads_and_back_returns() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        let absolute = f.home.join("Documents").display().to_string();
        // From Pictures each time: what is typed, and where it leads.
        let leads = [
            ("~", f.home.clone()),
            ("~/Pictures/Trip/", pictures.join("Trip")),
            ("Trip", pictures.join("Trip")),
            ("../Documents/.", f.home.join("Documents")),
            ("./Trip/../..", f.home.clone()),
            (absolute.as_str(), f.home.join("Documents")),
        ];
        for (typed, folder) in leads {
            f.app.navigate(&pictures);
            press(&mut f.app, &[KeyCode::End]);
            go_to_typed(&mut f.app, typed);
            assert_eq!(f.app.pane().cwd, folder, "{typed}");
            assert!(f.app.field.is_none(), "{typed}");
            assert_eq!(f.app.message, None, "{typed}");
            // It is a step in the history like any other.
            f.app.handle_key(alt(KeyCode::Left));
            assert_eq!(f.app.pane().cwd, pictures, "{typed}");
            assert_eq!(selected_name(&f.app), Some("a.jpg"), "{typed}");
        }
    }

    #[test]
    fn a_path_to_a_file_opens_its_folder_with_the_file_selected() {
        let mut f = fixture();
        f.app.navigate(&f.home.join("Documents"));
        go_to_typed(&mut f.app, "~/Pictures/a.jpg");
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(selected_name(&f.app), Some("a.jpg"));
        assert!(f.app.field.is_none());
        assert!(f.system.opened.borrow().is_empty(), "selected, not opened");
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));

        // One in the folder that is showing only moves the cursor.
        let mut f = fixture();
        go_to_typed(&mut f.app, "notes.txt");
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(f.app.field.is_none());
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(
            selected_name(&f.app),
            Some("notes.txt"),
            "nowhere to go back to"
        );
    }

    #[test]
    fn a_path_that_does_not_exist_keeps_the_field_open_and_says_so() {
        let mut f = fixture();
        go_to_typed(&mut f.app, "~/Nowhere/else");
        assert_eq!(path_field(&f.app), ("~/Nowhere/else".to_string(), 14));
        assert_eq!(said(&f.app), ("~/Nowhere/else doesn't exist.", true));
        assert_eq!(f.app.pane().cwd, f.home);
        let all = crate::ui::testing::screen(&mut f.app, 120, 24).join("\n");
        assert!(all.contains("~/Nowhere/else doesn't exist."), "{all}");

        // So does a folder that is there and can't be read.
        let locked = f.home.join("Documents");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        clear_field(&mut f.app);
        type_text(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(path_field(&f.app).0, "Documents");
        assert_eq!(said(&f.app), ("Documents: Permission was refused.", true));
        assert_eq!(f.app.pane().cwd, f.home);

        // It can be put right where it stands.
        clear_field(&mut f.app);
        type_text(&mut f.app, "~/Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert!(f.app.field.is_none());
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn tab_completes_folder_names_as_far_as_they_are_certain() {
        let mut f = fixture();
        fs::create_dir(f.home.join("Pictures").join("Trips 2025")).unwrap();
        fs::write(f.home.join("Pictures").join("Trailer.mov"), "").unwrap();
        open_path_field(&mut f.app);
        // One folder starts like that: its name, and a slash to go on from.
        assert_eq!(completed(&mut f.app, "~/Doc"), "~/Documents/");
        assert_eq!(completed(&mut f.app, "~/pIC"), "~/Pictures/", "any case");
        assert_eq!(completed(&mut f.app, "Pic"), "Pictures/", "from the folder");
        assert_eq!(completed(&mut f.app, "~"), "~/");
        // Several do: as far as they are alike.
        assert_eq!(completed(&mut f.app, "~/Pictures/t"), "~/Pictures/Trip");
        assert_eq!(
            completed(&mut f.app, "~/Pictures/Trips"),
            "~/Pictures/Trips 2025/"
        );
        // None does: nothing. A file is not a folder.
        assert_eq!(completed(&mut f.app, "~/Pictures/zz"), "~/Pictures/zz");
        assert_eq!(completed(&mut f.app, "~/Pictures/Trai"), "~/Pictures/Trai");
        assert_eq!(completed(&mut f.app, "~/no"), "~/no");
        assert_eq!(completed(&mut f.app, "~/Nowhere/x"), "~/Nowhere/x");
        // Tab went nowhere else.
        assert_eq!((f.app.focus, &f.app.pane().cwd), (Focus::Files, &f.home));
    }

    #[test]
    fn hidden_folders_complete_only_when_they_are_shown_or_the_dot_is_typed() {
        let mut f = fixture();
        fs::create_dir_all(f.home.join("Box").join(".git")).unwrap();
        fs::create_dir(f.home.join("Box").join("src")).unwrap();
        open_path_field(&mut f.app);
        assert_eq!(completed(&mut f.app, "~/Box/"), "~/Box/src/");
        assert_eq!(completed(&mut f.app, "~/Box/."), "~/Box/.git/");
        press(&mut f.app, &[KeyCode::Esc]);
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        open_path_field(&mut f.app);
        // Two of them now, with nothing alike.
        assert_eq!(completed(&mut f.app, "~/Box/"), "~/Box/");
    }

    #[test]
    fn tab_reads_the_folder_as_it_is_when_pressed_and_drawing_reads_nothing() {
        let mut f = fixture();
        open_path_field(&mut f.app);
        type_text(&mut f.app, "/Mus");
        fs::create_dir(f.home.join("Music")).unwrap();
        for _ in 0..3 {
            f.app.tick();
            crate::ui::testing::screen(&mut f.app, 120, 24);
        }
        assert_eq!(path_field(&f.app).0, "~/Mus");
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(path_field(&f.app).0, "~/Music/");
    }

    #[test]
    fn escape_closes_the_path_field_and_changes_nothing() {
        let mut f = fixture();
        f.app.navigate(&f.home.join("Pictures"));
        press(&mut f.app, &[KeyCode::End]);
        open_path_field(&mut f.app);
        type_text(&mut f.app, "/Trip");
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.field.is_none());
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(selected_name(&f.app), Some("a.jpg"));
        assert_eq!(f.app.message, None);
        // Nothing was added to the history: back is still the home folder.
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn while_the_path_field_is_open_every_key_goes_to_it() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down]);
        open_path_field(&mut f.app);
        clear_field(&mut f.app);
        type_text(&mut f.app, "qsS/? ");
        assert_eq!(path_field(&f.app), ("qsS/? ".to_string(), 6));
        for other in [
            key(KeyCode::Up),
            key(KeyCode::Down),
            key(KeyCode::PageDown),
            key(KeyCode::BackTab),
            key(KeyCode::F(2)),
            key(KeyCode::F(5)),
            key(KeyCode::Delete),
            shift(KeyCode::Delete),
            ctrl(KeyCode::Char('h')),
            ctrl(KeyCode::Char('c')),
            ctrl(KeyCode::Char('v')),
            ctrl(KeyCode::Char('p')),
            ctrl(KeyCode::Char('3')),
            ctrl(KeyCode::Char('l')),
            ctrl_shift('n'),
            alt(KeyCode::Char('b')),
        ] {
            f.app.handle_key(other);
            assert_eq!(path_field(&f.app).0, "qsS/? ", "{other:?}");
        }
        assert!(!f.app.should_quit && !f.app.help_visible && f.app.dialog.is_none());
        let (sort, view, hidden) = (f.app.sort, f.app.view, f.app.show_hidden);
        assert_eq!(
            (sort, view, hidden),
            (SortMode::Name, ViewMode::List, false)
        );
        assert_eq!((f.app.focus, f.app.pane().cursor), (Focus::Files, 1));
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.app.pane().search().is_none());
        assert_eq!((f.app.working(), &f.app.message), (None, &None));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
    }

    /// The fixture with things named after a trip at three depths:
    /// `Documents/trip-plan.md`, `Pictures/Trip/day-trip.jpg` and, out of
    /// sight, `.cache/trip.log`.
    fn searchable() -> Fixture {
        let f = fixture();
        fs::write(f.home.join("Documents").join("trip-plan.md"), "plan").unwrap();
        let trip = f.home.join("Pictures").join("Trip");
        fs::write(trip.join("day-trip.jpg"), "jpg").unwrap();
        fs::create_dir(f.home.join(".cache")).unwrap();
        fs::write(f.home.join(".cache").join("trip.log"), "log").unwrap();
        f
    }

    const FOLDER: [&str; 3] = ["Documents", "Pictures", "notes.txt"];
    const TRIPS: [&str; 3] = ["trip-plan.md", "Trip", "day-trip.jpg"];

    /// Types into the search field and lets the walk look everywhere.
    fn search_for(app: &mut App, text: &str) {
        type_text(app, text);
        finish_search(app);
    }

    fn search_text(app: &App) -> String {
        let search = app.pane().search().expect("no search is open");
        search.input.text()
    }

    fn typing_a_search(app: &App) -> bool {
        matches!(app.field, Some(Field::Search))
    }

    #[test]
    fn slash_lists_what_matches_at_and_below_the_folder_as_it_is_typed() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('/')]);
        assert!(typing_a_search(&f.app));
        assert_eq!(search_text(&f.app), "");

        type_text(&mut f.app, "TR");
        assert!(names(&f.app).is_empty(), "typing doesn't wait for the walk");
        assert!(f.app.pane().search().unwrap().is_running());
        finish_search(&mut f.app);
        // The nearest first, a file before a folder: the order they were found in.
        assert_eq!(names(&f.app), TRIPS);
        assert_eq!(f.app.pane().cursor, 0);
        assert_eq!(f.app.pane().cwd, f.home, "the folder being searched");
        assert!(typing_a_search(&f.app));
        assert_eq!(f.app.message, None);

        // What is typed next is looked for in place of it.
        search_for(&mut f.app, "ip-");
        assert_eq!(search_text(&f.app), "TRip-");
        assert_eq!(names(&f.app), ["trip-plan.md"]);
        search_for(&mut f.app, "x");
        assert!(names(&f.app).is_empty());
        press(&mut f.app, &[KeyCode::Backspace; 4]);
        finish_search(&mut f.app);
        assert_eq!(search_text(&f.app), "TR");
        assert_eq!(names(&f.app), TRIPS);
    }

    #[test]
    fn a_search_skips_hidden_items_until_they_are_shown_and_is_never_sorted() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('/')]);
        search_for(&mut f.app, "trip");
        assert_eq!(names(&f.app), TRIPS);
        press(&mut f.app, &[KeyCode::Tab]);

        // Another order sorts the folders, not what was found.
        press(&mut f.app, &[KeyCode::Char('S')]);
        finish_search(&mut f.app);
        assert_eq!(f.app.sort, SortMode::Size);
        assert_eq!(names(&f.app), TRIPS);

        f.app.handle_key(ctrl(KeyCode::Char('h')));
        finish_search(&mut f.app);
        let with_hidden = ["trip.log", "trip-plan.md", "Trip", "day-trip.jpg"];
        assert_eq!(names(&f.app), with_hidden);
        assert_eq!(search_text(&f.app), "trip");
    }

    #[test]
    fn an_empty_search_field_shows_the_folder_as_it_was() {
        let mut f = searchable();
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Char('/')]);
        assert_eq!(names(&f.app), FOLDER);
        assert_eq!(selected_name(&f.app), Some("Pictures"));

        search_for(&mut f.app, "trip");
        assert_eq!(names(&f.app), TRIPS);
        press(&mut f.app, &[KeyCode::Backspace; 4]);
        assert_eq!(search_text(&f.app), "");
        assert_eq!(names(&f.app), FOLDER);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        assert!(typing_a_search(&f.app), "the field stays open");
        assert!(!f.app.pane().search().unwrap().is_running());
    }

    #[test]
    fn up_down_and_enter_act_on_what_was_found_without_leaving_the_field() {
        let mut f = searchable();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('/')]);
        search_for(&mut f.app, "trip");
        press(&mut f.app, &[KeyCode::Down, KeyCode::Down, KeyCode::Up]);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        assert!(typing_a_search(&f.app));
        assert_eq!(search_text(&f.app), "trip");

        // Enter on a file opens it, and the search stays.
        press(&mut f.app, &[KeyCode::Up, KeyCode::Enter]);
        let plan = f.home.join("Documents").join("trip-plan.md");
        assert_eq!(*f.system.opened.borrow(), [plan]);
        assert!(typing_a_search(&f.app));
        assert_eq!(names(&f.app), TRIPS);

        // Enter on a folder goes into it, leaving the search.
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        assert_eq!(names(&f.app), ["day-trip.jpg"]);
        assert!(f.app.field.is_none() && f.app.pane().search().is_none());

        // Back is the folder that was searched, as it was: not what was found.
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(names(&f.app), FOLDER);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(f.app.pane().search().is_none());
    }

    #[test]
    fn escape_leaves_the_search_and_shows_the_folder_with_the_cursor_where_it_was() {
        let mut f = searchable();
        select(&mut f.app, "notes.txt");
        // From the field, and from what was found once the keyboard is there.
        for at_what_was_found in [false, true] {
            press(&mut f.app, &[KeyCode::Char('/')]);
            search_for(&mut f.app, "trip");
            press(&mut f.app, &[KeyCode::Down]);
            if at_what_was_found {
                press(&mut f.app, &[KeyCode::Tab]);
                assert!(f.app.field.is_none() && f.app.pane().search().is_some());
            }
            press(&mut f.app, &[KeyCode::Esc]);
            assert!(f.app.field.is_none() && f.app.pane().search().is_none());
            assert_eq!(f.app.pane().cwd, f.home);
            assert_eq!(names(&f.app), FOLDER);
            assert_eq!(selected_name(&f.app), Some("notes.txt"));
        }
    }

    #[test]
    fn what_a_search_finds_works_like_any_item_but_nothing_is_pasted_or_made_there() {
        let mut f = searchable();
        let plan = f.home.join("Documents").join("trip-plan.md");
        press(&mut f.app, &[KeyCode::Char('/')]);
        search_for(&mut f.app, "trip");
        // Tab hands the keyboard to what was found; the search stays.
        press(&mut f.app, &[KeyCode::Tab]);
        assert!(f.app.field.is_none());
        assert_eq!(f.app.focus, Focus::Files);

        press(&mut f.app, &[KeyCode::Char(' '), KeyCode::Enter]);
        let asked_for = [plan.clone()];
        assert_eq!(*f.system.previewed.borrow(), asked_for);
        assert_eq!(*f.system.opened.borrow(), asked_for);
        assert_eq!(search_text(&f.app), "trip", "the space wasn't typed");
        f.app.handle_key(ctrl_shift('c'));
        assert_eq!(*f.system.copied.borrow(), [plan.display().to_string()]);
        f.app.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(said(&f.app), ("1 item ready to move.", false));
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));

        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(said(&f.app), ("Files can't be pasted here.", false));
        assert_eq!(f.app.working(), None);
        f.app.handle_key(ctrl_shift('n'));
        assert_eq!(said(&f.app), ("Folders can't be made here.", false));
        assert!(f.app.dialog.is_none());

        // F2 renames it where it is; the search looks again and finds it.
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "trip-notes.md");
        press(&mut f.app, &[KeyCode::Enter]);
        finish_search(&mut f.app);
        assert_eq!(on_disk(&f.home.join("Documents")), ["trip-notes.md"]);
        assert_eq!(names(&f.app), ["trip-notes.md", "Trip", "day-trip.jpg"]);
        assert_eq!(selected_name(&f.app), Some("trip-notes.md"));
        let renamed = "Renamed trip-plan.md to trip-notes.md.";
        assert_eq!(said(&f.app), (renamed, false));

        // Delete moves it to the trash, and the cursor to what was next to it.
        select(&mut f.app, "day-trip.jpg");
        press(&mut f.app, &[KeyCode::Delete]);
        finish_search(&mut f.app);
        assert_eq!(read(f.trash.join("day-trip.jpg")), "jpg");
        assert_eq!(names(&f.app), ["trip-notes.md", "Trip"]);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        assert_eq!(said(&f.app), ("Moved 1 item to the trash.", false));

        // `/` hands the keyboard back to the field, as it was left.
        press(&mut f.app, &[KeyCode::Char('/')]);
        assert!(typing_a_search(&f.app));
        assert_eq!(search_text(&f.app), "trip");
        assert_eq!(names(&f.app), ["trip-notes.md", "Trip"]);
    }

    #[test]
    fn while_the_search_field_is_open_what_is_typed_is_searched_for() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "q s?/");
        assert_eq!(search_text(&f.app), "q s?/");
        for other in [
            key(KeyCode::F(2)),
            key(KeyCode::F(5)),
            ctrl(KeyCode::Char('h')),
            ctrl(KeyCode::Char('c')),
            ctrl(KeyCode::Char('p')),
            ctrl(KeyCode::Char('3')),
            ctrl(KeyCode::Char('l')),
            ctrl_shift('n'),
            alt(KeyCode::Char('b')),
        ] {
            f.app.handle_key(other);
            assert_eq!(search_text(&f.app), "q s?/", "{other:?}");
            assert!(typing_a_search(&f.app), "{other:?}");
        }
        assert!(!f.app.should_quit && !f.app.help_visible && f.app.dialog.is_none());
        let (sort, view, hidden) = (f.app.sort, f.app.view, f.app.show_hidden);
        assert_eq!(
            (sort, view, hidden),
            (SortMode::Name, ViewMode::List, false)
        );
        assert!(f.system.previewed.borrow().is_empty());
        assert_eq!(f.app.pane().cwd, f.home);

        // With nothing typed there is no search to hand the keyboard on to.
        press(&mut f.app, &[KeyCode::Backspace; 5]);
        press(&mut f.app, &[KeyCode::Tab]);
        assert!(f.app.field.is_none() && f.app.pane().search().is_none());
        assert_eq!(f.app.focus, Focus::Files);
        assert_eq!(names(&f.app), FOLDER);
    }

    #[test]
    fn escape_stops_the_file_work_under_way_before_it_leaves_a_search() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('/')]);
        search_for(&mut f.app, "trip");
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl_shift('d'));
        assert_eq!(f.app.working(), Some("Duplicating trip-plan.md…"));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(
            search_text(&f.app),
            "trip",
            "the first one was for the work"
        );
        finish_work(&mut f.app);
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.pane().search().is_none());
        assert_eq!(names(&f.app), FOLDER);
    }

    #[test]
    fn a_search_from_recents_looks_below_home_and_escape_returns_to_them() {
        let mut f = searchable();
        *f.system.recents.borrow_mut() = vec![f.home.join("notes.txt")];
        open_place(&mut f.app, "Recents");
        press(&mut f.app, &[KeyCode::Char('/')]);
        assert_eq!(
            names(&f.app),
            ["notes.txt"],
            "as it was, until something is typed"
        );
        search_for(&mut f.app, "trip");
        assert_eq!(names(&f.app), TRIPS);
        assert!(!f.app.pane().recents());
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.pane().recents());
        assert_eq!(names(&f.app), ["notes.txt"]);
    }

    #[test]
    fn with_no_field_open_the_keys_do_what_they_did() {
        let mut f = searchable();
        // A search and a path field, come and gone.
        press(&mut f.app, &[KeyCode::Char('/')]);
        search_for(&mut f.app, "trip");
        press(&mut f.app, &[KeyCode::Esc]);
        open_path_field(&mut f.app);
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.pane().is_folder());

        press(&mut f.app, &[KeyCode::Char('s')]);
        assert_eq!(f.app.sort, SortMode::Kind);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
        press(&mut f.app, &[KeyCode::Char('?')]);
        assert!(f.app.help_visible);
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        // Tab moves between the panes again, and L alone is only a letter.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Char('l')]);
        assert_eq!(f.app.focus, Focus::Details);
        assert!(f.app.field.is_none());
        press(&mut f.app, &[KeyCode::Char('q')]);
        assert!(f.app.should_quit);
    }

    // Two panes side by side, and selections of several items. The eight
    // items of `several()` are Documents, Pictures, a.txt to e.txt, notes.txt.

    /// The names of what is selected in the active pane.
    fn chosen(app: &App) -> Vec<&str> {
        let selection = app.pane().selection();
        selection.map(|entry| entry.name.as_str()).collect()
    }

    fn pane_names(app: &App, pane: usize) -> Vec<&str> {
        let entries = app.panes[pane].entries.iter();
        entries.map(|entry| entry.name.as_str()).collect()
    }

    /// The name of the item a pane's cursor is on.
    fn under_cursor(app: &App, pane: usize) -> Option<&str> {
        app.panes[pane].selected().map(|entry| entry.name.as_str())
    }

    /// Where the keyboard is: the region, and the pane that is the active one.
    fn keyboard(app: &App) -> (Focus, usize) {
        (app.focus, app.active)
    }

    #[test]
    fn ctrl_alt_n_splits_the_window_in_two_and_ctrl_w_closes_the_right_pane() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(f.app.panes.len(), 1, "one pane: there is none to close");
        assert_eq!(keyboard(&f.app), (Focus::Files, 0));

        // The new pane is on the right, in the same folder, with the keyboard.
        press(&mut f.app, &[KeyCode::Tab]);
        split(&mut f.app);
        assert_eq!(f.app.panes.len(), 2);
        assert_eq!(keyboard(&f.app), (Focus::Files, 1));
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        assert_eq!(f.app.panes[0].cwd, pictures);
        assert_eq!(under_cursor(&f.app, 0), Some("a.jpg"));

        // Two is as many as there are, in either spelling of the chord.
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl_alt(KeyCode::Char('N')));
        split(&mut f.app);
        assert_eq!(f.app.panes.len(), 2);
        assert_eq!(keyboard(&f.app), (Focus::Files, 1));
        assert_eq!(f.app.pane().cwd, pictures.join("Trip"));

        // From the left pane too it is the right one that closes.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(f.app.panes.len(), 1);
        assert_eq!(keyboard(&f.app), (Focus::Files, 0));
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(selected_name(&f.app), Some("a.jpg"));

        // Closed from the right pane or the sidebar, the left one takes the keyboard.
        f.app.handle_key(ctrl_alt(KeyCode::Char('N')));
        assert_eq!((f.app.panes.len(), f.app.active), (2, 1));
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(f.app.panes.len(), 1);
        assert_eq!(keyboard(&f.app), (Focus::Files, 0));
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Sidebar);
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(f.app.panes.len(), 1);
        assert_eq!(keyboard(&f.app), (Focus::Files, 0));
    }

    #[test]
    fn each_pane_has_its_own_folder_history_cursor_and_listing() {
        let mut f = searchable();
        let pictures = f.home.join("Pictures");
        let recent = f.home.join("notes.txt");
        f.system.recents.borrow_mut().push(recent);
        press(&mut f.app, &[KeyCode::Down]);
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, pictures.join("Trip"));
        assert_eq!(f.app.panes[0].cwd, f.home);
        assert_eq!(under_cursor(&f.app, 0), Some("Pictures"));

        // Back in the right pane; the left one has nowhere to go back to.
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(keyboard(&f.app), (Focus::Files, 0));
        assert_eq!(f.app.pane().cwd, f.home);
        press(&mut f.app, &[KeyCode::End]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(under_cursor(&f.app, 1), Some("Trip"));

        // A search on the left, the files used lately on the right.
        press(&mut f.app, &[KeyCode::Char('/')]);
        search_for(&mut f.app, "trip");
        assert_eq!(names(&f.app), TRIPS);
        assert_eq!(pane_names(&f.app, 1), ["Trip", "a.jpg"]);
        // Out of the field, then on to the right pane and round to the sidebar.
        press(&mut f.app, &[KeyCode::Tab; 4]);
        assert_eq!(keyboard(&f.app), (Focus::Sidebar, 1));
        open_place(&mut f.app, "Recents");
        assert_eq!(keyboard(&f.app), (Focus::Files, 1));
        assert!(f.app.pane().recents());
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert!(f.app.panes[0].results().is_some());
        assert_eq!(pane_names(&f.app, 0), TRIPS);

        // Up from there is the right pane's business alone.
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.app.pane().is_folder());
        assert_eq!(pane_names(&f.app, 0), TRIPS);
    }

    #[test]
    fn tab_goes_from_the_sidebar_through_both_panes_to_the_details() {
        let mut f = fixture();
        split(&mut f.app);
        let walk = |app: &mut App, code: KeyCode| {
            let stops = (0..4).map(|_| {
                press(app, &[code]);
                keyboard(app)
            });
            stops.collect::<Vec<_>>()
        };
        assert_eq!(
            walk(&mut f.app, KeyCode::Tab),
            [
                (Focus::Details, 1),
                (Focus::Sidebar, 1),
                (Focus::Files, 0),
                (Focus::Files, 1)
            ]
        );
        assert_eq!(
            walk(&mut f.app, KeyCode::BackTab),
            [
                (Focus::Files, 0),
                (Focus::Sidebar, 0),
                (Focus::Details, 0),
                (Focus::Files, 1)
            ]
        );
        // Without the details there are three stops.
        f.app.handle_key(ctrl(KeyCode::Char('i')));
        assert_eq!(
            walk(&mut f.app, KeyCode::Tab)[..3],
            [(Focus::Sidebar, 1), (Focus::Files, 0), (Focus::Files, 1)]
        );
    }

    #[test]
    fn what_acts_on_the_pane_acts_on_the_one_with_the_keyboard() {
        let mut f = fixture();
        let (documents, pictures) = (f.home.join("Documents"), f.home.join("Pictures"));
        press(&mut f.app, &[KeyCode::Enter]);
        split(&mut f.app);
        press(
            &mut f.app,
            &[KeyCode::Backspace, KeyCode::Down, KeyCode::Enter],
        );
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(f.app.panes[0].cwd, documents);

        // The sidebar's place and the path field are the right pane's.
        assert_eq!(f.app.current_place(), place_of(&f.app, "Pictures"));
        open_path_field(&mut f.app);
        assert_eq!(path_field(&f.app).0, "~/Pictures");
        press(&mut f.app, &[KeyCode::Esc]);
        // A new folder is made in its folder.
        f.app.handle_key(ctrl_shift('n'));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(on_disk(&pictures), ["New Folder", "Trip", "a.jpg"]);
        assert!(on_disk(&documents).is_empty());

        // And the left one's once the keyboard is there.
        press(&mut f.app, &[KeyCode::BackTab]);
        assert_eq!(f.app.current_place(), place_of(&f.app, "Documents"));
        open_path_field(&mut f.app);
        assert_eq!(path_field(&f.app).0, "~/Documents");
        press(&mut f.app, &[KeyCode::Esc]);
        // F5 reads the one pane again.
        fs::write(documents.join("new.txt"), "").unwrap();
        fs::write(pictures.join("new.txt"), "").unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        assert_eq!(names(&f.app), ["new.txt"]);
        assert_eq!(pane_names(&f.app, 1), ["New Folder", "Trip", "a.jpg"]);
    }

    #[test]
    fn what_is_copied_or_cut_in_one_pane_is_pasted_into_the_other_ones_folder() {
        let mut f = fixture();
        let (documents, pictures) = (f.home.join("Documents"), f.home.join("Pictures"));
        select(&mut f.app, "notes.txt");
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter, KeyCode::BackTab]);
        assert_eq!(f.app.panes[1].cwd, documents);

        // Copied on the left, pasted on the right.
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(f.app.active, 1);
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(pane_names(&f.app, 0), FOLDER);
        assert_eq!(said(&f.app), ("Copied 1 item.", false));

        // Cut on the right, pasted on the left: both panes see it move.
        f.app.handle_key(ctrl(KeyCode::Char('x')));
        press(&mut f.app, &[KeyCode::BackTab]);
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&pictures), ["Trip", "a.jpg", "notes.txt"]);
        assert!(on_disk(&documents).is_empty());
        assert_eq!(read(pictures.join("notes.txt")), "hello");
        assert_eq!(f.app.active, 0);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(pane_names(&f.app, 1).is_empty(), "the right pane saw it go");
        assert_eq!(said(&f.app), ("Moved 1 item.", false));
    }

    #[test]
    fn after_a_change_both_panes_list_their_folder_again_and_keep_their_place() {
        let mut f = several();
        select(&mut f.app, "c.txt");
        split(&mut f.app);
        f.app.panes[0].offset = 2;

        // A folder made on the right shows on the left, which stays on its item.
        f.app.handle_key(ctrl_shift('n'));
        clear_field(&mut f.app);
        type_text(&mut f.app, "Album");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(selected_name(&f.app), Some("Album"));
        assert_eq!(
            pane_names(&f.app, 0)[..3],
            ["Album", "Documents", "Pictures"]
        );
        assert_eq!(under_cursor(&f.app, 0), Some("c.txt"));
        assert_eq!(f.app.panes[0].offset, 2, "scrolled where it was");

        // The item the left pane is on goes, from the right: the one that
        // takes its row is the left pane's now.
        select(&mut f.app, "c.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        let left = [
            "Album",
            "Documents",
            "Pictures",
            "a.txt",
            "b.txt",
            "d.txt",
            "e.txt",
            "notes.txt",
        ];
        assert_eq!(pane_names(&f.app, 0), left);
        assert_eq!(under_cursor(&f.app, 0), Some("d.txt"));
        assert_eq!(names(&f.app), left);
        assert_eq!(selected_name(&f.app), Some("d.txt"));

        // What a folder holds is counted again where the folder is listed.
        select(&mut f.app, "a.txt");
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        let listed = &f.app.panes[0].entries[1];
        assert_eq!(listed.name, "Documents");
        assert_eq!(listed.item_count(), Some(1));
        assert_eq!(under_cursor(&f.app, 0), Some("d.txt"));

        // Undone from the left, the copy leaves the right pane's list.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert!(pane_names(&f.app, 1).is_empty());
        assert_eq!(f.app.panes[0].entries[1].item_count(), Some(0));
    }

    #[test]
    fn the_view_the_sort_order_and_hidden_files_are_the_same_in_both_panes() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        fs::write(pictures.join(".secret"), "").unwrap();
        fs::write(pictures.join("big.bin"), vec![0u8; 100]).unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(pane_names(&f.app, 0), ["Trip", "a.jpg", "big.bin"]);
        assert_eq!(names(&f.app), FOLDER);

        // Asked for with the keyboard on the right, they change on the left too.
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        assert_eq!(
            pane_names(&f.app, 0),
            ["Trip", ".secret", "a.jpg", "big.bin"]
        );
        assert_eq!(names(&f.app)[2..], [".hidden", "notes.txt"]);
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        assert_eq!(pane_names(&f.app, 0), ["Trip", "a.jpg", "big.bin"]);
        assert_eq!(names(&f.app), FOLDER);

        press(&mut f.app, &[KeyCode::Char('S')]);
        assert_eq!(f.app.sort, SortMode::Size);
        assert_eq!(pane_names(&f.app, 0), ["Trip", "big.bin", "a.jpg"]);
        assert_eq!(under_cursor(&f.app, 0), Some("Trip"));

        // The columns list the folders above in each pane.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        assert_eq!(f.app.panes[0].ancestors.len(), 1);
        assert_eq!(f.app.panes[1].ancestors.len(), 0);
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        assert_eq!(f.app.panes[0].ancestors.len(), 0);
    }

    #[test]
    fn ctrl_shift_n_and_ctrl_n_do_what_they_did_and_split_nothing() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('n')));
        assert!(f.app.dialog.is_none());
        let held = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        for chord in [ctrl_shift('n'), KeyEvent::new(KeyCode::Char('n'), held)] {
            f.app.handle_key(chord);
            assert!(matches!(f.app.dialog, Some(Dialog::NewFolder { .. })));
            press(&mut f.app, &[KeyCode::Esc]);
        }
        // The new chords' letters alone, or with Alt alone, are only letters.
        type_text(&mut f.app, "nwa");
        f.app.handle_key(alt(KeyCode::Char('n')));
        assert_eq!(f.app.panes.len(), 1);
        assert_eq!(chosen(&f.app), ["Documents"]);
        assert_eq!((f.app.working(), &f.app.message), (None, &None));
        assert_eq!(on_disk(&f.home), FOLDER);
    }

    #[test]
    fn shift_arrows_select_a_range_from_where_it_started() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(selected_name(&f.app), Some("c.txt"));

        // Back toward where it started it shrinks; past it, it grows the other way.
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app), ["a.txt"]);
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app), ["Pictures", "a.txt"]);
        assert_eq!(selected_name(&f.app), Some("Pictures"));

        f.app.handle_key(shift(KeyCode::End));
        assert_eq!(
            chosen(&f.app),
            ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "notes.txt"]
        );
        f.app.handle_key(shift(KeyCode::Home));
        assert_eq!(chosen(&f.app), ["Documents", "Pictures", "a.txt"]);
        // A page is the three rows a window this short shows.
        crate::ui::testing::screen(&mut f.app, 120, 6);
        f.app.handle_key(shift(KeyCode::PageDown));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);
        f.app.handle_key(shift(KeyCode::PageUp));
        assert_eq!(chosen(&f.app), ["Documents", "Pictures", "a.txt"]);
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn in_the_grid_shift_left_and_right_extend_by_a_tile_and_up_and_down_by_a_row() {
        let mut f = grid_fixture();
        press(&mut f.app, &[KeyCode::Right]);
        f.app.handle_key(shift(KeyCode::Right));
        assert_eq!(chosen(&f.app), ["Pictures", "a.txt"]);
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(
            chosen(&f.app),
            ["Pictures", "a.txt", "b.txt", "c.txt", "d.txt"]
        );
        f.app.handle_key(shift(KeyCode::Left));
        assert_eq!(chosen(&f.app), ["Pictures", "a.txt", "b.txt", "c.txt"]);
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app), ["Pictures"]);
        f.app.handle_key(shift(KeyCode::Left));
        assert_eq!(chosen(&f.app), ["Documents", "Pictures"]);
        assert_eq!(selected_name(&f.app), Some("Documents"));
        assert_eq!(f.app.pane().cwd, f.home, "no folder was entered or left");
    }

    #[test]
    fn ctrl_a_selects_everything_in_the_pane_and_leaves_the_cursor() {
        let mut f = several();
        select(&mut f.app, "b.txt");
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(chosen(&f.app).len(), 8);
        assert_eq!(chosen(&f.app), pane_names(&f.app, 0));
        assert_eq!(selected_name(&f.app), Some("b.txt"));

        // Only the files' own key: the sidebar has nothing to select.
        press(&mut f.app, &[KeyCode::Down, KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(chosen(&f.app), ["c.txt"]);
        // In an empty folder there is nothing to take.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Home, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert!(chosen(&f.app).is_empty());
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn a_plain_move_or_escape_leaves_only_the_item_under_the_cursor_selected() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Down));
        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(chosen(&f.app), ["d.txt"]);
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app), ["c.txt", "d.txt"]);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(chosen(&f.app), ["c.txt"]);
        assert_eq!(selected_name(&f.app), Some("c.txt"));

        // Escape dismisses what the band says before it touches the selection.
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("8 items ready to paste.", false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
        assert_eq!(chosen(&f.app).len(), 8);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(chosen(&f.app), ["c.txt"]);

        // Another folder starts with one item selected, and so does the way back.
        select(&mut f.app, "Documents");
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(chosen(&f.app), ["Documents", "Pictures"]);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(chosen(&f.app), ["Trip"]);
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(chosen(&f.app), ["Pictures"]);
    }

    #[test]
    fn copy_cut_and_paste_take_the_whole_selection() {
        let mut f = several();
        let (documents, pictures) = (f.home.join("Documents"), f.home.join("Pictures"));
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("3 items ready to paste.", false));
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(f.app.working(), Some("Copying 3 items…"));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(read(documents.join("c.txt")), "c");
        assert_eq!(on_disk(&f.home).len(), 8, "the originals stay");
        assert_eq!(said(&f.app), ("Copied 3 items.", false));

        f.app.handle_key(ctrl(KeyCode::Char('a')));
        f.app.handle_key(ctrl(KeyCode::Char('x')));
        assert_eq!(said(&f.app), ("3 items ready to move.", false));
        press(&mut f.app, &[KeyCode::Backspace]);
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(
            on_disk(&pictures),
            ["Trip", "a.jpg", "a.txt", "b.txt", "c.txt"]
        );
        assert!(on_disk(&documents).is_empty());
        assert_eq!(said(&f.app), ("Moved 3 items.", false));
    }

    #[test]
    fn delete_and_shift_delete_take_the_whole_selection() {
        let mut f = several();
        select(&mut f.app, "b.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Down));
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(
            on_disk(&f.home),
            ["Documents", "Pictures", "a.txt", "e.txt", "notes.txt"]
        );
        assert_eq!(on_disk(&f.trash), ["b.txt", "c.txt", "d.txt"]);
        assert_eq!(said(&f.app), ("Moved 3 items to the trash.", false));
        assert_eq!(chosen(&f.app), ["e.txt"]);
        // One undo brings the three of them back.
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(on_disk(&f.home).len(), 8);
        assert!(on_disk(&f.trash).is_empty());

        // Deleting for good asks about all of them, once.
        select(&mut f.app, "d.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Delete));
        let Some(Dialog::Delete { paths, armed }) = &f.app.dialog else {
            panic!("it didn't ask");
        };
        assert_eq!(*paths, [f.home.join("d.txt"), f.home.join("e.txt")]);
        assert!(!*armed);
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Enter]);
        assert_eq!(
            on_disk(&f.home),
            [
                "Documents",
                "Pictures",
                "a.txt",
                "b.txt",
                "c.txt",
                "notes.txt"
            ]
        );
        assert!(on_disk(&f.trash).is_empty(), "gone for good");
        assert_eq!(said(&f.app), ("Deleted 2 items.", false));
        assert_eq!(chosen(&f.app), ["notes.txt"]);
    }

    #[test]
    fn duplicate_and_copy_path_take_the_selection_and_the_rest_the_item_under_the_cursor() {
        let mut f = several();
        let (a, b) = (f.home.join("a.txt"), f.home.join("b.txt"));
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));

        // One name can't go to two items.
        press(&mut f.app, &[KeyCode::F(2)]);
        assert!(f.app.dialog.is_none());
        // A path on each line.
        f.app.handle_key(ctrl_shift('c'));
        let lines = format!("{}\n{}", a.display(), b.display());
        assert_eq!(*f.system.copied.borrow(), [lines]);
        // Space, Enter and the Open button are for the one under the cursor.
        press(&mut f.app, &[KeyCode::Char(' '), KeyCode::Enter]);
        press(
            &mut f.app,
            &[KeyCode::Tab, KeyCode::Enter, KeyCode::BackTab],
        );
        assert_eq!(*f.system.previewed.borrow(), std::slice::from_ref(&b));
        assert_eq!(*f.system.opened.borrow(), [b.clone(), b.clone()]);
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);

        f.app.handle_key(ctrl_shift('d'));
        assert_eq!(f.app.working(), Some("Duplicating 2 items…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("a (copy).txt")), "a");
        assert_eq!(read(f.home.join("b (copy).txt")), "b");
        assert_eq!(on_disk(&f.home).len(), 10);
        assert_eq!(said(&f.app), ("Duplicated 2 items.", false));

        // With one item selected F2 asks for its new name, as before.
        select(&mut f.app, "c.txt");
        press(&mut f.app, &[KeyCode::F(2)]);
        let Some(Dialog::Rename { path, .. }) = &f.app.dialog else {
            panic!("it didn't ask");
        };
        assert_eq!(*path, f.home.join("c.txt"));
    }

    #[test]
    fn each_pane_keeps_its_own_selection_while_the_keyboard_is_elsewhere() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));
        split(&mut f.app);
        assert_eq!(chosen(&f.app), ["Documents"]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        press(&mut f.app, &[KeyCode::BackTab]);
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(chosen(&f.app).len(), 8);
        // Round by the details and the sidebar.
        press(&mut f.app, &[KeyCode::Tab; 3]);
        assert_eq!(keyboard(&f.app), (Focus::Files, 0));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);

        // Pasted into the other pane's folder, it is still what is selected here.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Home, KeyCode::Enter]);
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&f.home.join("Documents")), ["a.txt", "b.txt"]);
        press(&mut f.app, &[KeyCode::BackTab]);
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);
        assert_eq!(selected_name(&f.app), Some("b.txt"));
    }

    #[test]
    fn with_one_pane_and_one_item_selected_the_keys_do_what_they_did() {
        let mut f = several();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Down]);
        assert_eq!(chosen(&f.app), ["a.txt"]);
        press(&mut f.app, &[KeyCode::Up]);
        assert_eq!(chosen(&f.app), ["Pictures"]);
        press(&mut f.app, &[KeyCode::End]);
        assert_eq!(chosen(&f.app), ["notes.txt"]);
        press(&mut f.app, &[KeyCode::Home]);
        assert_eq!(chosen(&f.app), ["Documents"]);

        // Delete takes the one item, and Escape only dismisses what was said.
        select(&mut f.app, "a.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(on_disk(&f.trash), ["a.txt"]);
        assert_eq!(on_disk(&f.home).len(), 7);
        assert_eq!(said(&f.app), ("Moved 1 item to the trash.", false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
        assert_eq!(chosen(&f.app), ["b.txt"]);
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));

        // Tab goes round the three regions and the one pane stays the active one.
        let mut stops = Vec::new();
        for _ in 0..3 {
            press(&mut f.app, &[KeyCode::Tab]);
            stops.push(keyboard(&f.app));
        }
        assert_eq!(
            stops,
            [(Focus::Details, 0), (Focus::Sidebar, 0), (Focus::Files, 0)]
        );
        press(&mut f.app, &[KeyCode::F(2)]);
        assert!(matches!(f.app.dialog, Some(Dialog::Rename { .. })));
    }

    // The menu. The program that zips and the terminal are fakes, the trash
    // is the fixture's own folder, and so is where its records are kept.

    /// The lines of the menu that is open: the labels, and a dash for each
    /// hairline between two groups.
    fn menu(app: &App) -> Vec<&str> {
        let rows = &app.menu.as_ref().expect("no menu is open").rows;
        let line = |row: &MenuRow| match row {
            MenuRow::Item { label, .. } => *label,
            MenuRow::Separator => "-",
        };
        rows.iter().map(line).collect()
    }

    /// The labels of the lines that can't be used right now.
    fn dim(app: &App) -> Vec<&str> {
        let rows = &app.menu.as_ref().expect("no menu is open").rows;
        let unusable = |row: &MenuRow| match row {
            MenuRow::Item {
                label,
                enabled: false,
                ..
            } => Some(*label),
            _ => None,
        };
        rows.iter().filter_map(unusable).collect()
    }

    fn menu_cursor(app: &App) -> usize {
        app.menu.as_ref().expect("no menu is open").cursor
    }

    /// Alt+Enter, down to the line with this label, and Enter.
    fn pick(app: &mut App, label: &str) {
        app.handle_key(alt(KeyCode::Enter));
        let lines = menu(app);
        let row = lines.iter().position(|line| *line == label);
        let row = row.unwrap_or_else(|| panic!("{label} isn't in {lines:?}"));
        while menu_cursor(app) != row {
            let before = menu_cursor(app);
            press(app, &[KeyCode::Down]);
            assert_ne!(menu_cursor(app), before, "{label} can't be reached");
        }
        press(app, &[KeyCode::Enter]);
    }

    /// Every name in a folder on disk, the hidden ones too.
    fn all_on_disk(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    const FILE_MENU: [&str; 16] = [
        "Open",
        "Preview",
        "-",
        "Rename",
        "Duplicate",
        "-",
        "Copy",
        "Cut",
        "Copy Path",
        "-",
        "Compress",
        "-",
        "Open Terminal Here",
        "-",
        "Move to Trash",
        "Delete…",
    ];

    const FOLDER_MENU: [&str; 19] = [
        "Open Folder",
        "Preview",
        "-",
        "Rename",
        "Duplicate",
        "-",
        "Copy",
        "Cut",
        "Paste Into Folder",
        "Copy Path",
        "-",
        "Compress",
        "-",
        "Pin",
        "-",
        "Open Terminal Here",
        "-",
        "Move to Trash",
        "Delete…",
    ];

    #[test]
    fn alt_enter_and_the_menu_key_open_a_menu_for_the_item_under_the_cursor() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), FILE_MENU);
        assert!(dim(&f.app).is_empty());
        assert_eq!(menu_cursor(&f.app), 0);
        // Escape closes it with nothing done.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.menu.is_none());
        assert!(f.system.opened.borrow().is_empty());
        assert_eq!(selected_name(&f.app), Some("notes.txt"));

        // A folder has more to offer; with nothing copied, nothing to paste.
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Menu]);
        assert_eq!(menu(&f.app), FOLDER_MENU);
        assert_eq!(dim(&f.app), ["Paste Into Folder"]);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.pane().cwd, f.home, "it was not opened");

        // From the details pane it is the same item's menu.
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Details);
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), FOLDER_MENU);
    }

    #[test]
    fn the_menu_keeps_every_key_to_itself_and_up_and_down_skip_the_hairlines() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl_shift('c'));
        assert!(f.app.message.is_some());
        f.app.handle_key(alt(KeyCode::Enter));

        let mut stops = Vec::new();
        for code in [
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Up,
            KeyCode::Up,
        ] {
            press(&mut f.app, &[code]);
            stops.push(menu_cursor(&f.app));
        }
        assert_eq!(stops, [1, 3, 1, 0, 0], "over the hairline, and no further");
        press(&mut f.app, &[KeyCode::Down; 30]);
        assert_eq!(menu_cursor(&f.app), FILE_MENU.len() - 1);

        // No other key reaches what is underneath.
        for other in [
            key(KeyCode::Char('q')),
            key(KeyCode::Char('?')),
            key(KeyCode::Char('/')),
            key(KeyCode::Char(' ')),
            key(KeyCode::Delete),
            key(KeyCode::F(2)),
            key(KeyCode::Tab),
            key(KeyCode::Backspace),
            key(KeyCode::Home),
            ctrl(KeyCode::Char('a')),
            ctrl(KeyCode::Char('l')),
            ctrl_shift('n'),
            alt(KeyCode::Left),
        ] {
            f.app.handle_key(other);
            assert_eq!(menu_cursor(&f.app), FILE_MENU.len() - 1, "{other:?}");
        }
        assert!(!f.app.should_quit && !f.app.help_visible);
        assert!(f.app.dialog.is_none() && f.app.field.is_none());
        assert_eq!(f.app.focus, Focus::Files);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(f.system.previewed.borrow().is_empty());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);

        // Escape closes the menu first; what was said goes with the next one.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.menu.is_none());
        assert_eq!(said(&f.app), ("Copied the path.", false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn the_menu_items_run_what_their_keys_run() {
        let mut f = fixture();
        let notes = f.home.join("notes.txt");
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Open");
        assert!(f.app.menu.is_none(), "chosen, it closes");
        let only_notes = std::slice::from_ref(&notes);
        assert_eq!(f.system.opened.borrow().as_slice(), only_notes);
        pick(&mut f.app, "Preview");
        assert_eq!(f.system.previewed.borrow().as_slice(), only_notes);
        pick(&mut f.app, "Copy Path");
        assert_eq!(*f.system.copied.borrow(), [notes.display().to_string()]);
        assert_eq!(said(&f.app), ("Copied the path.", false));

        pick(&mut f.app, "Rename");
        assert_eq!(field(&f.app), ("notes.txt".to_string(), 5));
        press(&mut f.app, &[KeyCode::Esc]);

        pick(&mut f.app, "Duplicate");
        assert_eq!(f.app.working(), Some("Duplicating notes.txt…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes (copy).txt")), "hello");

        select(&mut f.app, "notes (copy).txt");
        pick(&mut f.app, "Move to Trash");
        assert_eq!(on_disk(&f.trash), ["notes (copy).txt"]);
        assert_eq!(said(&f.app), ("Moved 1 item to the trash.", false));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);

        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Delete…");
        let Some(Dialog::Delete { paths, armed }) = &f.app.dialog else {
            panic!("it didn't ask");
        };
        assert_eq!((paths.as_slice(), *armed), ([notes].as_slice(), false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");

        select(&mut f.app, "Pictures");
        pick(&mut f.app, "Open Folder");
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
    }

    #[test]
    fn copy_cut_and_paste_work_from_the_menu_and_paste_into_folder_pastes_into_it() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Copy");
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));

        select(&mut f.app, "Pictures");
        f.app.handle_key(alt(KeyCode::Enter));
        assert!(dim(&f.app).is_empty(), "now there is something to paste");
        press(&mut f.app, &[KeyCode::Esc]);
        pick(&mut f.app, "Paste Into Folder");
        assert_eq!(f.app.working(), Some("Copying 1 item…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("Pictures").join("notes.txt")), "hello");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(f.app.pane().cwd, f.home, "the pane stays where it was");

        // Cut here, pasted in a folder with nothing in it to open a menu on.
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Cut");
        assert_eq!(said(&f.app), ("1 item ready to move.", false));
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        pick(&mut f.app, "Paste");
        assert_eq!(f.app.working(), Some("Moving 1 item…"));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&f.home.join("Documents")), ["notes.txt"]);
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
    }

    #[test]
    fn in_an_empty_folder_the_menu_offers_paste_a_new_folder_and_a_terminal() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(names(&f.app).is_empty());
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), ["Paste", "New Folder", "Open Terminal Here"]);
        assert_eq!(dim(&f.app), ["Paste"], "nothing was copied or cut");
        // A dim line does nothing, and the menu stays.
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(menu_cursor(&f.app), 0);
        assert_eq!((f.app.working(), &f.app.message), (None, &None));
        press(&mut f.app, &[KeyCode::Esc]);

        pick(&mut f.app, "New Folder");
        assert_eq!(field(&f.app), ("New Folder".to_string(), 10));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(on_disk(&f.home.join("Documents")), ["New Folder"]);
    }

    #[test]
    fn what_the_listing_forbids_is_dim_in_the_menu_and_does_nothing() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        // Two folders selected, the cursor on the second.
        press(&mut f.app, &[KeyCode::Home]);
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(chosen(&f.app), ["Documents", "Pictures"]);
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), FOLDER_MENU);
        assert_eq!(dim(&f.app), ["Rename", "Paste Into Folder", "Pin"]);
        press(&mut f.app, &[KeyCode::Esc]);
        for refused in ["Rename", "Paste Into Folder", "Pin"] {
            pick(&mut f.app, refused);
            assert!(f.app.menu.is_some(), "{refused}");
            assert!(f.app.dialog.is_none() && f.app.working().is_none());
            press(&mut f.app, &[KeyCode::Esc]);
        }
        assert_eq!(chosen(&f.app), ["Documents", "Pictures"]);
        assert_eq!(sections(&f.app), [home_places(), views()]);

        // What a search found in several folders has no one folder for an archive.
        fs::write(f.home.join("Pictures").join("b.txt"), "b").unwrap();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "b.txt");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(chosen(&f.app).len(), 2);
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(dim(&f.app), ["Rename", "Compress"]);
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Esc]);

        // Recents with nothing in them: nothing is pasted or made there.
        open_place(&mut f.app, "Recents");
        assert!(names(&f.app).is_empty());
        f.app.handle_key(alt(KeyCode::Enter));
        let nothing = ["Paste", "New Folder", "Open Terminal Here"];
        assert_eq!(menu(&f.app), nothing);
        assert_eq!(dim(&f.app), nothing);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        assert!(f.app.dialog.is_none() && f.app.menu.is_some());
        assert_eq!(on_disk(&f.home).len(), 8);
    }

    #[test]
    fn in_the_sidebar_the_menu_is_for_the_place_under_the_cursor() {
        let mut f = fixture();
        let trip = f.home.join("Pictures").join("Trip");
        f.app.navigate(&trip);
        pin(&mut f.app);
        for (place, lines) in [
            ("Documents", vec!["Open"]),
            ("Trip", vec!["Open", "-", "Unpin"]),
            ("Recents", vec!["Open"]),
            ("Trash", vec!["Open", "-", "Empty Trash…"]),
        ] {
            go_to_place(&mut f.app, place);
            f.app.handle_key(alt(KeyCode::Enter));
            assert_eq!(menu(&f.app), lines, "{place}");
            assert!(dim(&f.app).is_empty());
            press(&mut f.app, &[KeyCode::Esc]);
            assert_eq!(f.app.focus, Focus::Sidebar);
        }
        assert_eq!(f.app.pane().cwd, trip, "nothing was opened yet");

        // Open goes there, as Enter does.
        go_to_place(&mut f.app, "Documents");
        pick(&mut f.app, "Open");
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));
        assert_eq!(f.app.focus, Focus::Files);
        go_to_place(&mut f.app, "Recents");
        pick(&mut f.app, "Open");
        assert!(f.app.pane().recents());

        // Unpin takes the pin away, as Delete does there.
        go_to_place(&mut f.app, "Trip");
        pick(&mut f.app, "Unpin");
        assert_eq!(sections(&f.app), [home_places(), views()]);
        assert!(trip.is_dir(), "the folder itself is left alone");
    }

    #[test]
    fn open_terminal_here_starts_one_in_the_folder_or_in_the_files_folder() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        select(&mut f.app, "Pictures");
        pick(&mut f.app, "Open Terminal Here");
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Open Terminal Here");
        // With nothing under the cursor it is the folder that is showing.
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        pick(&mut f.app, "Open Terminal Here");
        assert_eq!(
            *f.system.terminals.borrow(),
            [pictures, f.home.clone(), f.home.join("Documents")]
        );
        assert_eq!(f.app.message, None);

        f.system.no_terminal.set(true);
        pick(&mut f.app, "Open Terminal Here");
        assert_eq!(said(&f.app), ("Couldn't start a terminal.", true));
        assert_eq!(f.system.terminals.borrow().len(), 3);
    }

    #[test]
    fn pin_in_the_menu_pins_the_folder_under_the_cursor() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        pick(&mut f.app, "Pin");
        assert_eq!(sections(&f.app)[1], ("Pinned", vec!["Trip"]));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));

        // Pinned, the line takes the pin away again.
        f.app.handle_key(alt(KeyCode::Enter));
        assert!(menu(&f.app).contains(&"Unpin") && !menu(&f.app).contains(&"Pin"));
        press(&mut f.app, &[KeyCode::Esc]);
        pick(&mut f.app, "Unpin");
        assert_eq!(sections(&f.app), [home_places(), views()]);

        // What Home already lists is refused in the words Ctrl+P uses.
        press(&mut f.app, &[KeyCode::Backspace]);
        select(&mut f.app, "Documents");
        pick(&mut f.app, "Pin");
        assert_eq!(said(&f.app), ("Documents is already under Home.", false));
        assert_eq!(sections(&f.app), [home_places(), views()]);
    }

    #[test]
    fn compress_makes_a_zip_beside_the_item_and_undo_moves_it_to_the_trash() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Compress");
        assert_eq!(f.app.working(), Some("Compressing into notes.txt.zip…"));
        finish_work(&mut f.app);
        assert_eq!(
            all_on_disk(&f.home),
            [
                ".hidden",
                "Documents",
                "Pictures",
                "notes.txt",
                "notes.txt.zip"
            ],
            "the archive, and nothing left over from making it"
        );
        assert_eq!(read(f.home.join("notes.txt.zip")), "notes.txt");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(said(&f.app), ("Compressed into notes.txt.zip.", false));
        // The folder was listed again, the cursor on the archive.
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", "notes.txt", "notes.txt.zip"]
        );
        assert_eq!(selected_name(&f.app), Some("notes.txt.zip"));

        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(read(f.trash.join("notes.txt.zip")), "notes.txt");
        let undid = "Undid compressing into notes.txt.zip.";
        assert_eq!(said(&f.app), (undid, false));
        // There it is known like anything else this app moved to the trash.
        let known = f.app.trashed(&f.trash.join("notes.txt.zip"));
        assert_eq!(
            known.map(|record| &record.from),
            Some(&f.home.join("notes.txt.zip"))
        );
        f.app.handle_key(ctrl_shift('z'));
        assert_eq!(read(f.home.join("notes.txt.zip")), "notes.txt");
        assert!(on_disk(&f.trash).is_empty());
        let redid = "Redid compressing into notes.txt.zip.";
        assert_eq!(said(&f.app), (redid, false));
    }

    #[test]
    fn compressing_several_items_makes_archive_zip_and_a_taken_name_keeps_both() {
        let mut f = fixture();
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(chosen(&f.app), ["Documents", "Pictures"]);
        pick(&mut f.app, "Compress");
        assert_eq!(f.app.working(), Some("Compressing into Archive.zip…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("Archive.zip")), "Documents\nPictures");
        assert_eq!(selected_name(&f.app), Some("Archive.zip"));
        assert_eq!(said(&f.app), ("Compressed into Archive.zip.", false));

        // Again: the name is taken, and what has it is not written over.
        fs::write(f.home.join("Archive.zip"), "the first one").unwrap();
        press(&mut f.app, &[KeyCode::Home]);
        f.app.handle_key(shift(KeyCode::Down));
        pick(&mut f.app, "Compress");
        let second = "Compressing into Archive (copy).zip…";
        assert_eq!(f.app.working(), Some(second));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("Archive.zip")), "the first one");
        assert_eq!(
            read(f.home.join("Archive (copy).zip")),
            "Documents\nPictures"
        );
        assert_eq!(selected_name(&f.app), Some("Archive (copy).zip"));

        // The same for one item.
        fs::write(f.home.join("notes.txt.zip"), "someone else's").unwrap();
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Compress");
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes.txt.zip")), "someone else's");
        assert_eq!(read(f.home.join("notes.txt (copy).zip")), "notes.txt");
        assert_eq!(
            all_on_disk(&f.home),
            [
                ".hidden",
                "Archive (copy).zip",
                "Archive.zip",
                "Documents",
                "Pictures",
                "notes.txt",
                "notes.txt (copy).zip",
                "notes.txt.zip",
            ]
        );
    }

    #[test]
    fn a_compress_that_is_stopped_or_fails_leaves_nothing_behind() {
        let mut f = fixture();
        let untouched = [".hidden", "Documents", "Pictures", "notes.txt"];
        f.system.archiving.set(Archiving::Hangs);
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Compress");
        assert_eq!(f.app.working(), Some("Compressing into notes.txt.zip…"));
        // The window isn't frozen meanwhile, and other changes wait.
        f.app.tick();
        press(&mut f.app, &[KeyCode::Delete]);
        let wait = ("Try again once the work under way is done.", false);
        assert_eq!(said(&f.app), wait);
        assert_eq!(f.app.working(), Some("Compressing into notes.txt.zip…"));

        press(&mut f.app, &[KeyCode::Esc]);
        finish_work(&mut f.app);
        assert_eq!(said(&f.app), ("Stopped compressing.", false));
        assert_eq!(all_on_disk(&f.home), untouched);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));

        f.system.archiving.set(Archiving::Fails);
        pick(&mut f.app, "Compress");
        finish_work(&mut f.app);
        let why = "Couldn't compress: The archive couldn't be written.";
        assert_eq!(said(&f.app), (why, true));
        assert_eq!(all_on_disk(&f.home), untouched);
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));
    }

    /// The fixture's home with an app of its own that remembers things in
    /// `.state`, showing a list.
    fn remembering(f: &Fixture) -> (App, PathBuf) {
        let state = f.home.join(".state");
        let mut app = launch(&f.home, Some(&state));
        app.view = ViewMode::List;
        (app, state)
    }

    #[test]
    fn what_goes_to_the_trash_is_on_record_with_where_it_came_from_until_it_leaves() {
        let f = fixture();
        let (mut app, state) = remembering(&f);
        let (notes, in_trash) = (f.home.join("notes.txt"), f.trash.join("notes.txt"));
        select(&mut app, "notes.txt");
        press(&mut app, &[KeyCode::Delete]);
        let records = state::load_trashed(&state);
        let [record] = records.as_slice() else {
            panic!("not one record: {records:?}");
        };
        assert_eq!((&record.at, &record.from), (&in_trash, &notes));
        let age = std::time::SystemTime::now().duration_since(record.when);
        assert!(age.unwrap() < std::time::Duration::from_secs(60));
        // One line: when, where it is, where it was.
        let line = format!("\t{}\t{}\n", in_trash.display(), notes.display());
        let written = read(state.join("trashed"));
        assert!(written.ends_with(&line), "{written}");
        assert_eq!(written.lines().count(), 1);
        assert_eq!(app.trashed(&in_trash), Some(record));

        // Undoing the trash removes its record; redoing it records it again.
        app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(read(state.join("trashed")), "");
        assert_eq!(app.trashed(&in_trash), None);
        app.handle_key(ctrl_shift('z'));
        assert_eq!(state::load_trashed(&state).len(), 1);

        // What left the trash behind the app's back is dropped the next
        // time the file is written.
        fs::remove_file(&in_trash).unwrap();
        select(&mut app, "Pictures");
        press(&mut app, &[KeyCode::Delete]);
        let records = state::load_trashed(&state);
        let kept: Vec<&PathBuf> = records.iter().map(|record| &record.at).collect();
        assert_eq!(kept, [&f.trash.join("Pictures")]);

        // The records are there after a restart.
        let (again, _) = remembering(&f);
        let from = again.trashed(&f.trash.join("Pictures"));
        assert_eq!(
            from.map(|record| &record.from),
            Some(&f.home.join("Pictures"))
        );
    }

    const TRASH_MENU: [&str; 4] = ["Restore", "Delete…", "-", "Empty Trash…"];

    #[test]
    fn restore_moves_what_is_selected_back_to_where_it_came_from() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        select(&mut f.app, "a.jpg");
        press(&mut f.app, &[KeyCode::Delete, KeyCode::Backspace]);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);

        open_place(&mut f.app, "Trash");
        assert_eq!(names(&f.app), ["a.jpg", "notes.txt"]);
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), TRASH_MENU);
        assert!(dim(&f.app).is_empty());
        press(&mut f.app, &[KeyCode::Esc]);

        f.app.handle_key(ctrl(KeyCode::Char('a')));
        pick(&mut f.app, "Restore");
        assert_eq!(said(&f.app), ("Restored 2 items.", false));
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(read(pictures.join("a.jpg")), "jpg");
        assert!(on_disk(&f.trash).is_empty());
        assert!(names(&f.app).is_empty(), "the trash was listed again");
        assert_eq!(f.app.trashed(&f.trash.join("a.jpg")), None);

        // Undone, they are in the trash again, and still known there.
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid restoring 2 items.", false));
        assert_eq!(on_disk(&f.trash), ["a.jpg", "notes.txt"]);
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
        assert_eq!(on_disk(&pictures), ["Trip"]);
        let known = f.app.trashed(&f.trash.join("a.jpg"));
        assert_eq!(
            known.map(|record| &record.from),
            Some(&pictures.join("a.jpg"))
        );

        // One at a time works the same.
        select(&mut f.app, "a.jpg");
        pick(&mut f.app, "Restore");
        assert_eq!(said(&f.app), ("Restored 1 item.", false));
        assert_eq!(on_disk(&pictures), ["Trip", "a.jpg"]);
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert_eq!(on_disk(&f.trash), ["notes.txt"]);
    }

    #[test]
    fn a_restore_that_cannot_be_done_says_why_and_leaves_the_item_in_the_trash() {
        let mut f = fixture();
        // Its name was taken since.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        fs::write(f.home.join("notes.txt"), "written since").unwrap();
        // The folder it was in is gone.
        let trip = f.home.join("Pictures").join("Trip");
        fs::write(trip.join("day.jpg"), "day").unwrap();
        f.app.navigate(&trip);
        press(&mut f.app, &[KeyCode::Delete]);
        f.app.navigate(&f.home.clone());
        fs::remove_dir(&trip).unwrap();
        // Something else put it in the trash.
        fs::write(f.trash.join("stray.txt"), "stray").unwrap();

        open_place(&mut f.app, "Trash");
        let in_trash = ["day.jpg", "notes.txt", "stray.txt"];
        assert_eq!(names(&f.app), in_trash);
        let home = f.home.file_name().unwrap().to_string_lossy().into_owned();
        let taken = format!("Couldn't restore notes.txt: notes.txt already exists in {home}.");
        for (item, why) in [
            ("notes.txt", taken.as_str()),
            (
                "day.jpg",
                "Couldn't restore day.jpg: Trip isn't there any more.",
            ),
            (
                "stray.txt",
                "stray.txt has no record of where it came from.",
            ),
        ] {
            select(&mut f.app, item);
            pick(&mut f.app, "Restore");
            assert_eq!(said(&f.app), (why, true));
            assert_eq!(on_disk(&f.trash), in_trash, "{item} stays");
            assert_eq!(names(&f.app), in_trash);
        }
        assert_eq!(read(f.home.join("notes.txt")), "written since");
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert!(!trip.exists(), "the folder is not made again");
        // Nothing was moved, so the last thing to undo is still the trashing.
        assert!(f.app.trashed(&f.trash.join("day.jpg")).is_some());
    }

    #[test]
    fn empty_trash_asks_first_and_deletes_only_what_the_trash_holds() {
        let f = fixture();
        let (mut app, state) = remembering(&f);
        let kept = f.home.join("Documents").join("kept.txt");
        fs::write(&kept, "keep").unwrap();
        select(&mut app, "notes.txt");
        press(&mut app, &[KeyCode::Delete]);
        select(&mut app, "Pictures");
        press(&mut app, &[KeyCode::Delete]);
        // Links someone put there, to things that have to outlive them.
        std::os::unix::fs::symlink(f.home.join("Documents"), f.trash.join("to folder")).unwrap();
        std::os::unix::fs::symlink(&kept, f.trash.join("to file")).unwrap();
        fs::write(f.trash.join(".DS_Store"), "Finder's").unwrap();
        let held = ["Pictures", "notes.txt", "to file", "to folder"];
        assert_eq!(on_disk(&f.trash), held);
        assert_eq!(state::load_trashed(&state).len(), 2);

        open_place(&mut app, "Trash");
        pick(&mut app, "Empty Trash…");
        let Some(Dialog::EmptyTrash { paths, armed }) = &app.dialog else {
            panic!("it didn't ask");
        };
        assert_eq!((paths.len(), *armed), (4, false));
        // Cancel is what Enter takes, and Escape is a cancel too.
        press(&mut app, &[KeyCode::Enter]);
        assert!(app.dialog.is_none());
        assert_eq!(on_disk(&f.trash), held);
        pick(&mut app, "Empty Trash…");
        press(&mut app, &[KeyCode::Right, KeyCode::Left, KeyCode::Esc]);
        assert_eq!(on_disk(&f.trash), held);
        assert_eq!(app.message, None);

        pick(&mut app, "Empty Trash…");
        press(&mut app, &[KeyCode::Right, KeyCode::Enter]);
        assert_eq!(said(&app), ("Emptied the trash: 4 items deleted.", false));
        assert!(on_disk(&f.trash).is_empty());
        assert!(f.trash.is_dir(), "the trash itself stays");
        assert!(names(&app).is_empty(), "and is listed again");
        // What the links led to is untouched, and so is everything else.
        assert_eq!(read(kept), "keep");
        assert_eq!(on_disk(&f.home.join("Documents")), ["kept.txt"]);
        assert_eq!(on_disk(&f.home), ["Documents"]);
        assert_eq!(read(f.trash.join(".DS_Store")), "Finder's");
        // The records went with it.
        assert_eq!(read(state.join("trashed")), "");
        assert_eq!(app.trashed(&f.trash.join("notes.txt")), None);

        // There is no taking it back.
        app.handle_key(ctrl(KeyCode::Char('z')));
        let (text, is_error) = said(&app);
        assert!(is_error && text.starts_with("Can't undo that:"), "{text}");
        assert_eq!(on_disk(&f.home), ["Documents"]);
        assert!(on_disk(&f.trash).is_empty());
    }

    #[test]
    fn emptying_the_trash_from_the_sidebar_says_one_item_for_one() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        go_to_place(&mut f.app, "Trash");
        pick(&mut f.app, "Empty Trash…");
        assert!(matches!(f.app.dialog, Some(Dialog::EmptyTrash { .. })));
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Enter]);
        assert_eq!(said(&f.app), ("Emptied the trash: 1 item deleted.", false));
        assert!(on_disk(&f.trash).is_empty());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn an_empty_trash_and_one_that_cannot_be_listed_are_not_asked_about() {
        let mut f = fixture();
        let already = ("The trash is already empty.", false);
        // One nothing was ever moved to.
        go_to_place(&mut f.app, "Trash");
        pick(&mut f.app, "Empty Trash…");
        assert!(f.app.dialog.is_none());
        assert_eq!(said(&f.app), already);
        // One that holds nothing but Finder's notes about it.
        fs::create_dir(&f.trash).unwrap();
        fs::write(f.trash.join(".DS_Store"), "").unwrap();
        f.app.message = None;
        pick(&mut f.app, "Empty Trash…");
        assert!(f.app.dialog.is_none());
        assert_eq!(said(&f.app), already);
        // And from inside it, where there is nothing to open a menu on.
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.trash);
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), ["Empty Trash…"]);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(said(&f.app), already);
        assert!(f.trash.join(".DS_Store").exists());

        // One macOS keeps the terminal out of: nothing is deleted, and it
        // says what the sidebar says.
        fs::write(f.trash.join("notes.txt"), "still here").unwrap();
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o000)).unwrap();
        pick(&mut f.app, "Empty Trash…");
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(f.app.dialog.is_none());
        let refused = "Trash: Permission was refused. Give your terminal \
                       Full Disk Access in System Settings to see it.";
        assert_eq!(said(&f.app), (refused, true));
        assert_eq!(read(f.trash.join("notes.txt")), "still here");
    }

    #[test]
    fn alt_enter_in_the_search_field_opens_the_menu_for_what_was_found() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "notes");
        finish_search(&mut f.app);
        assert!(typing_a_search(&f.app));
        f.app.handle_key(alt(KeyCode::Enter));
        assert_eq!(menu(&f.app), FILE_MENU);
        // The keyboard moved on to what was found, which is still listed.
        assert!(!typing_a_search(&f.app));
        assert_eq!(names(&f.app), ["notes.txt"]);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), [f.home.join("notes.txt")]);
        assert_eq!(search_text(&f.app), "notes");
    }

    #[test]
    fn the_keys_the_menu_shares_code_with_do_what_they_did() {
        let mut f = fixture();
        let (pictures, documents) = (f.home.join("Pictures"), f.home.join("Documents"));
        // Enter opens, and Alt with an arrow walks the history.
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, pictures);
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home);
        f.app.handle_key(alt(KeyCode::Right));
        assert_eq!(f.app.pane().cwd, pictures);
        f.app.handle_key(alt(KeyCode::Char('b')));
        assert_eq!(f.app.pane().cwd, f.home);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), [f.home.join("notes.txt")]);
        assert!(f.app.menu.is_none());

        // Ctrl+V pastes into the folder that is showing, whatever is under
        // the cursor; Ctrl+P pins that folder.
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        f.app.navigate(&pictures);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&pictures), ["Trip", "a.jpg", "notes.txt"]);
        assert!(on_disk(&pictures.join("Trip")).is_empty());
        f.app.navigate(&pictures.join("Trip"));
        pin(&mut f.app);
        assert_eq!(sections(&f.app)[1], ("Pinned", vec!["Trip"]));

        // Enter in the sidebar goes to the place, and Delete there unpins.
        open_place(&mut f.app, "Documents");
        assert_eq!(
            (f.app.pane().cwd.clone(), f.app.focus),
            (documents, Focus::Files)
        );
        go_to_place(&mut f.app, "Trip");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(sections(&f.app), [home_places(), views()]);

        // Delete moves to the trash, and asks in the trash itself.
        open_place(&mut f.app, "Pictures");
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(on_disk(&f.trash), ["notes.txt"]);
        assert_eq!(said(&f.app), ("Moved 1 item to the trash.", false));
        // Escape dismisses what was said, then has nothing left to do.
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
        open_place(&mut f.app, "Trash");
        press(&mut f.app, &[KeyCode::Delete]);
        assert!(matches!(f.app.dialog, Some(Dialog::Delete { .. })));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(on_disk(&f.trash), ["notes.txt"]);
    }
}

/// The mouse: every click lands on what the last frame drew under it, and
/// does what the key for the same thing does.
#[cfg(test)]
mod mouse_tests {
    use super::testing::*;
    use super::*;
    use crate::ui::testing::{buffer, find};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use std::cell::Cell;
    use std::fs;
    use std::ops::Range;
    use std::rc::Rc;
    use std::time::Instant;

    fn draw(app: &mut App) -> Buffer {
        buffer(app, 120, 40)
    }

    /// Draws the window and says where a piece of text starts in it.
    fn spot(app: &mut App, text: &str, columns: Range<u16>) -> (u16, u16) {
        let drawn = draw(app);
        find(&drawn, text, columns).unwrap_or_else(|| panic!("{text} isn't drawn"))
    }

    fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16), held: KeyModifiers) {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: held,
        });
    }

    fn click(app: &mut App, at: (u16, u16)) {
        click_with(app, at, KeyModifiers::NONE);
    }

    fn click_with(app: &mut App, at: (u16, u16), held: KeyModifiers) {
        mouse(app, MouseEventKind::Down(MouseButton::Left), at, held);
    }

    fn right_click(app: &mut App, at: (u16, u16)) {
        mouse(
            app,
            MouseEventKind::Down(MouseButton::Right),
            at,
            KeyModifiers::NONE,
        );
    }

    fn wheel(app: &mut App, at: (u16, u16), down: bool) {
        let kind = if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        mouse(app, kind, at, KeyModifiers::NONE);
    }

    /// A clock the test moves on by hand.
    fn clock(app: &mut App) -> Rc<Cell<Instant>> {
        let now = Rc::new(Cell::new(Instant::now()));
        let read = now.clone();
        app.clock = Box::new(move || read.get());
        now
    }

    fn wait(clock: &Cell<Instant>, millis: u64) {
        clock.set(clock.get() + Duration::from_millis(millis));
    }

    fn cursor_name(app: &App) -> &str {
        app.selected().map_or("", |entry| entry.name.as_str())
    }

    fn selection(app: &App) -> Vec<&str> {
        let selected = app.pane().selection();
        selected.map(|entry| entry.name.as_str()).collect()
    }

    fn labels(app: &App) -> Vec<&str> {
        let rows = &app.menu.as_ref().expect("no menu is open").rows;
        let line = |row: &MenuRow| match row {
            MenuRow::Item { label, .. } => *label,
            MenuRow::Separator => "-",
        };
        rows.iter().map(line).collect()
    }

    const VIEWS: [char; 3] = ['2', '1', '3'];

    #[test]
    fn a_click_on_an_item_puts_the_keyboard_and_the_cursor_on_it_alone_in_every_view() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            f.app.handle_key(shift(KeyCode::Down));
            press(&mut f.app, &[KeyCode::BackTab]);
            assert_eq!(f.app.focus, Focus::Sidebar);
            let (x, y) = spot(&mut f.app, "c.txt", 24..86);
            // A tile is its picture as much as its name.
            let at = if view == '1' { (x, y - 3) } else { (x, y) };
            click(&mut f.app, at);
            assert_eq!(f.app.focus, Focus::Files, "view {view}");
            assert_eq!(f.app.active, 0);
            assert_eq!(cursor_name(&f.app), "c.txt", "view {view}");
            assert_eq!(selection(&f.app), ["c.txt"], "view {view}");
            assert_eq!(f.app.pane().cwd, f.home);
            assert!(f.app.pane().back.is_empty(), "nothing was opened");
        }
    }

    #[test]
    fn in_split_a_click_makes_its_pane_the_active_one_and_empty_space_leaves_the_cursor() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            split(&mut f.app);
            press(&mut f.app, &[KeyCode::Down]);
            assert_eq!(f.app.active, 1);
            let at = spot(&mut f.app, "a.txt", 24..55);
            click(&mut f.app, at);
            assert_eq!(
                (f.app.active, f.app.focus),
                (0, Focus::Files),
                "view {view}"
            );
            assert_eq!(cursor_name(&f.app), "a.txt", "view {view}");
            assert_eq!(f.app.panes[1].cursor, 1, "the other pane's cursor stays");

            // Empty space in the right pane: active, its cursor where it was.
            press(&mut f.app, &[KeyCode::BackTab]);
            assert_eq!(f.app.focus, Focus::Sidebar);
            draw(&mut f.app);
            click(&mut f.app, (57, 36));
            assert_eq!(
                (f.app.active, f.app.focus),
                (1, Focus::Files),
                "view {view}"
            );
            assert_eq!(f.app.panes[1].cursor, 1, "view {view}");
            assert_eq!(f.app.panes[0].cursor, 2, "view {view}");
        }
    }

    #[test]
    fn a_click_on_empty_space_makes_the_pane_active_and_leaves_the_selection_alone() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            f.app.handle_key(shift(KeyCode::Down));
            press(&mut f.app, &[KeyCode::Tab]);
            assert_eq!(f.app.focus, Focus::Details);
            draw(&mut f.app);
            click(&mut f.app, (25, 37));
            assert_eq!(f.app.focus, Focus::Files, "view {view}");
            assert_eq!(selection(&f.app), ["Documents", "Pictures"], "view {view}");
            assert_eq!(cursor_name(&f.app), "Pictures", "view {view}");
        }
    }

    #[test]
    fn in_columns_a_click_above_goes_to_that_folder_and_one_in_the_last_column_selects() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        let trip = spot(&mut f.app, "Trip", 51..86);
        let jpg = spot(&mut f.app, "a.jpg", 51..86);
        click(&mut f.app, jpg);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(cursor_name(&f.app), "a.jpg");
        click(&mut f.app, trip);
        assert_eq!(cursor_name(&f.app), "Trip");

        // The home column, left of it: there, with the row clicked selected.
        let notes = spot(&mut f.app, "notes.txt", 24..51);
        click(&mut f.app, notes);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(cursor_name(&f.app), "notes.txt");
        assert_eq!(f.app.focus, Focus::Files);
        // Ordinary navigation: Back returns.
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
    }

    #[test]
    fn two_clicks_on_one_item_within_400_ms_open_it_like_enter_in_every_view() {
        for view in VIEWS {
            let mut f = fixture();
            let now = clock(&mut f.app);
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let pictures = spot(&mut f.app, "Pictures", 24..86);
            click(&mut f.app, pictures);
            wait(&now, 400);
            draw(&mut f.app);
            click(&mut f.app, pictures);
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"), "view {view}");
            assert_eq!(f.app.pane().back.len(), 1, "in the history");

            // A file opens with its app.
            let jpg = spot(&mut f.app, "a.jpg", 24..86);
            click(&mut f.app, jpg);
            wait(&now, 150);
            draw(&mut f.app);
            click(&mut f.app, jpg);
            let opened = vec![f.home.join("Pictures").join("a.jpg")];
            assert_eq!(*f.system.opened.borrow(), opened, "view {view}");

            // Too slow: two clicks.
            wait(&now, 1000);
            click(&mut f.app, jpg);
            wait(&now, 401);
            draw(&mut f.app);
            click(&mut f.app, jpg);
            assert_eq!(*f.system.opened.borrow(), opened, "view {view}");

            // Quick, on two different items: two clicks.
            let trip = spot(&mut f.app, "Trip", 24..86);
            wait(&now, 1000);
            click(&mut f.app, trip);
            wait(&now, 100);
            draw(&mut f.app);
            click(&mut f.app, jpg);
            assert_eq!(*f.system.opened.borrow(), opened, "view {view}");
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"), "view {view}");
            assert_eq!(cursor_name(&f.app), "a.jpg");
        }
    }

    #[test]
    fn shift_click_selects_the_range_from_where_it_started() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let a = spot(&mut f.app, "a.txt", 24..86);
            let b = spot(&mut f.app, "b.txt", 24..86);
            let d = spot(&mut f.app, "d.txt", 24..86);
            click(&mut f.app, a);
            click_with(&mut f.app, d, KeyModifiers::SHIFT);
            assert_eq!(selection(&f.app), ["a.txt", "b.txt", "c.txt", "d.txt"]);
            assert_eq!(cursor_name(&f.app), "d.txt", "view {view}");
            // The range still starts at a.txt.
            click_with(&mut f.app, b, KeyModifiers::SHIFT);
            assert_eq!(selection(&f.app), ["a.txt", "b.txt"], "view {view}");
            // And Shift+arrows go on from there.
            let next = if view == '1' {
                KeyCode::Right
            } else {
                KeyCode::Down
            };
            f.app.handle_key(shift(next));
            assert_eq!(
                selection(&f.app),
                ["a.txt", "b.txt", "c.txt"],
                "view {view}"
            );
        }
    }

    #[test]
    fn alt_click_adds_or_removes_one_item_and_cmd_click_does_the_same() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let at = |f: &mut Fixture, name| spot(&mut f.app, name, 24..86);
            let (a, c, e) = (
                at(&mut f, "a.txt"),
                at(&mut f, "c.txt"),
                at(&mut f, "e.txt"),
            );
            click(&mut f.app, a);
            click_with(&mut f.app, c, KeyModifiers::ALT);
            assert_eq!(selection(&f.app), ["a.txt", "c.txt"], "view {view}");
            assert_eq!(cursor_name(&f.app), "c.txt");
            click_with(&mut f.app, e, KeyModifiers::SUPER);
            assert_eq!(selection(&f.app), ["a.txt", "c.txt", "e.txt"]);
            click_with(&mut f.app, a, KeyModifiers::ALT);
            assert_eq!(selection(&f.app), ["c.txt", "e.txt"], "view {view}");
            click_with(&mut f.app, e, KeyModifiers::ALT);
            assert_eq!(selection(&f.app), ["c.txt"], "view {view}");
            assert_eq!(cursor_name(&f.app), "c.txt");
            // The last one selected stays: something always is.
            click_with(&mut f.app, c, KeyModifiers::ALT);
            assert_eq!(selection(&f.app), ["c.txt"], "view {view}");
        }
    }

    #[test]
    fn right_click_selects_what_is_outside_the_selection_and_opens_its_menu_at_the_click() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let at = |f: &mut Fixture, name| spot(&mut f.app, name, 24..86);
            let (a, b, c, e) = (
                at(&mut f, "a.txt"),
                at(&mut f, "b.txt"),
                at(&mut f, "c.txt"),
                at(&mut f, "e.txt"),
            );
            click(&mut f.app, a);
            click_with(&mut f.app, c, KeyModifiers::SHIFT);
            press(&mut f.app, &[KeyCode::BackTab]);

            let at = (e.0 + 2, e.1);
            right_click(&mut f.app, at);
            assert_eq!(selection(&f.app), ["e.txt"], "view {view}");
            assert_eq!(f.app.focus, Focus::Files);
            assert_eq!(&labels(&f.app)[..4], ["Open", "Preview", "-", "Rename"]);
            // Right under the click, or right above it without room below.
            let drawn = draw(&mut f.app);
            let below = drawn[(at.0, at.1 + 1)].symbol() == "┌";
            let above = drawn[(at.0, at.1 - 1)].symbol() == "└";
            assert!(below || above, "view {view}");

            // Inside the selection it stays as it is, for all of it.
            press(&mut f.app, &[KeyCode::Esc]);
            draw(&mut f.app);
            click(&mut f.app, a);
            click_with(&mut f.app, c, KeyModifiers::SHIFT);
            right_click(&mut f.app, b);
            assert_eq!(
                selection(&f.app),
                ["a.txt", "b.txt", "c.txt"],
                "view {view}"
            );
            assert_eq!(cursor_name(&f.app), "b.txt");
            assert!(labels(&f.app).contains(&"Compress"));
            let rows = &f.app.menu.as_ref().unwrap().rows;
            let rename = MenuRow::Item {
                label: "Rename",
                action: Action::Rename,
                enabled: false,
            };
            assert!(rows.contains(&rename), "one name for several items");
        }

        // Alt+Enter after it opens next to the item again, not at the click.
        let mut f = several();
        let e = spot(&mut f.app, "e.txt", 24..86);
        right_click(&mut f.app, (e.0 + 10, e.1));
        press(&mut f.app, &[KeyCode::Esc]);
        f.app.handle_key(alt(KeyCode::Enter));
        let drawn = draw(&mut f.app);
        assert_eq!(drawn[(e.0, e.1 + 1)].symbol(), "┌");
        assert_eq!(find(&drawn, "Open", 24..86), Some((e.0 + 2, e.1 + 2)));
    }

    #[test]
    fn right_click_on_empty_space_opens_the_folders_menu_whose_terminal_opens_there() {
        let mut f = several();
        draw(&mut f.app);
        right_click(&mut f.app, (40, 30));
        assert_eq!(
            labels(&f.app),
            ["Paste", "New Folder", "Open Terminal Here"]
        );
        assert_eq!(cursor_name(&f.app), "Documents", "nothing selected anew");

        // Paste is dim with nothing copied: a click on it does nothing.
        let paste = spot(&mut f.app, "Paste", 24..86);
        click(&mut f.app, paste);
        assert!(f.app.menu.is_some() && f.app.message.is_none());
        // The terminal opens in the folder showing, not the one under the cursor.
        let terminal = spot(&mut f.app, "Open Terminal Here", 24..86);
        click(&mut f.app, terminal);
        assert!(f.app.menu.is_none());
        assert_eq!(*f.system.terminals.borrow(), [f.home.clone()]);

        draw(&mut f.app);
        right_click(&mut f.app, (40, 30));
        let new = spot(&mut f.app, "New Folder", 24..86);
        click(&mut f.app, new);
        assert!(matches!(f.app.dialog, Some(Dialog::NewFolder { .. })));
    }

    #[test]
    fn a_click_on_a_menu_line_runs_it_and_one_outside_only_closes_the_menu() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(alt(KeyCode::Enter));
        let documents = spot(&mut f.app, "Documents", 24..86);
        click(&mut f.app, documents);
        assert!(f.app.menu.is_none());
        assert_eq!(
            cursor_name(&f.app),
            "notes.txt",
            "nothing reached the files"
        );

        f.app.handle_key(alt(KeyCode::Enter));
        let copy_path = spot(&mut f.app, "Copy Path", 24..86);
        click(&mut f.app, copy_path);
        assert!(f.app.menu.is_none());
        let path = f.home.join("notes.txt").display().to_string();
        assert_eq!(*f.system.copied.borrow(), [path]);

        // A right click on another item while it is open only closes it too.
        f.app.handle_key(alt(KeyCode::Enter));
        draw(&mut f.app);
        right_click(&mut f.app, documents);
        assert!(f.app.menu.is_none());
        assert_eq!(cursor_name(&f.app), "notes.txt");
    }

    #[test]
    fn right_click_on_a_place_opens_its_menu_and_a_click_on_one_opens_it() {
        let mut f = fixture();
        let place = spot(&mut f.app, "Pictures", 0..24);
        right_click(&mut f.app, place);
        assert_eq!(f.app.focus, Focus::Sidebar);
        assert_eq!(labels(&f.app), ["Open"]);
        assert_eq!(f.app.pane().cwd, f.home, "not opened yet");
        let open = spot(&mut f.app, "Open", 0..86);
        assert_eq!(open, (place.0 + 2, place.1 + 2), "at the click");
        click(&mut f.app, open);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(f.app.focus, Focus::Files);

        // A plain click opens a place, the way Enter in the sidebar does.
        let documents = spot(&mut f.app, "Documents", 0..24);
        click(&mut f.app, documents);
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));
        assert_eq!(f.app.focus, Focus::Files);
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
    }

    #[test]
    fn a_click_on_a_breadcrumb_goes_there_and_back_returns() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        let crumb = spot(&mut f.app, "Pictures", 0..86);
        assert_eq!(crumb.1, 0, "in the top bar");
        click(&mut f.app, crumb);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        f.app.handle_key(alt(KeyCode::Left));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        let home = spot(&mut f.app, "~", 0..10);
        click(&mut f.app, home);
        assert_eq!(f.app.pane().cwd, f.home);

        // Split, each pane's own path above it.
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        let crumb = spot(&mut f.app, "~", 56..86);
        assert_eq!(crumb.1, 2);
        click(&mut f.app, crumb);
        assert_eq!(f.app.panes[1].cwd, f.home);
        assert_eq!(f.app.active, 1);
    }

    #[test]
    fn the_top_bar_toggles_do_what_their_keys_do() {
        let mut f = fixture();
        for (now, next) in [
            ("≡", ViewMode::Columns),
            ("▥", ViewMode::Grid),
            ("▦", ViewMode::List),
        ] {
            let toggle = spot(&mut f.app, now, 86..120);
            click(&mut f.app, toggle);
            assert_eq!(f.app.view, next);
        }
        let sort = spot(&mut f.app, "⇅", 86..120);
        click(&mut f.app, sort);
        assert_eq!(f.app.sort, SortMode::Name.step(1));
        assert_eq!(said(&f.app), Some(SortMode::Name.step(1).tip()));
        let hidden = spot(&mut f.app, "◉", 86..120);
        click(&mut f.app, hidden);
        assert!(f.app.show_hidden && names(&f.app).contains(&".hidden".to_string()));
        let details = spot(&mut f.app, "◨", 86..120);
        click(&mut f.app, details);
        assert!(!f.app.details_visible);
    }

    fn said(app: &App) -> Option<&str> {
        app.message.as_ref().map(|message| message.text.as_str())
    }

    #[test]
    fn the_open_button_of_the_details_opens_the_item_under_the_cursor() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        let open = spot(&mut f.app, "Open", 86..120);
        click(&mut f.app, open);
        assert_eq!(*f.system.opened.borrow(), [f.home.join("notes.txt")]);
    }

    #[test]
    fn the_wheel_scrolls_a_list_by_three_rows_keeping_the_cursor_in_sight() {
        let mut f = fixture();
        for n in 10..70 {
            fs::write(f.home.join(format!("{n}.txt")), "").unwrap();
        }
        press(&mut f.app, &[KeyCode::F(5)]);
        // 63 items, 37 rows.
        let top = spot(&mut f.app, "Documents", 24..86);
        wheel(&mut f.app, top, true);
        assert_eq!(f.app.pane().offset, 3);
        assert_eq!(f.app.pane().cursor, 3, "moved down into sight");
        let drawn = draw(&mut f.app);
        assert_eq!(
            find(&drawn, "11.txt", 24..86),
            Some((28, 3)),
            "the fourth item on top"
        );
        for _ in 0..20 {
            wheel(&mut f.app, top, true);
        }
        assert_eq!(f.app.pane().offset, 26, "as far as it goes");
        assert_eq!(f.app.pane().cursor, 26);
        wheel(&mut f.app, top, false);
        assert_eq!(f.app.pane().offset, 23);
        assert_eq!(f.app.pane().cursor, 26, "still in sight: left alone");
        draw(&mut f.app);
        press(&mut f.app, &[KeyCode::Down]);
        draw(&mut f.app);
        assert_eq!((f.app.pane().offset, f.app.pane().cursor), (23, 27));
    }

    #[test]
    fn the_wheel_scrolls_the_grid_by_a_row_of_tiles() {
        let mut f = grid_fixture();
        // Ten tiles, three across: four rows, two showing.
        let drawn = buffer(&mut f.app, 120, 24);
        let first = find(&drawn, "Documents", 24..86).unwrap();
        wheel(&mut f.app, first, true);
        assert_eq!((f.app.pane().offset, f.app.pane().cursor), (1, 3));
        buffer(&mut f.app, 120, 24);
        wheel(&mut f.app, first, true);
        wheel(&mut f.app, first, true);
        assert_eq!((f.app.pane().offset, f.app.pane().cursor), (2, 6));
        wheel(&mut f.app, first, false);
        assert_eq!((f.app.pane().offset, f.app.pane().cursor), (1, 6));
    }

    #[test]
    fn the_wheel_over_the_sidebar_scrolls_it() {
        let mut f = fixture();
        for n in 0..20 {
            fs::create_dir(f.home.join(format!("Folder {n:02}"))).unwrap();
        }
        press(&mut f.app, &[KeyCode::F(5)]);
        let row = |drawn: &Buffer, text: &str| find(drawn, text, 0..23).map(|(_, y)| y);
        let drawn = buffer(&mut f.app, 120, 14);
        assert_eq!(row(&drawn, "HOME"), Some(3));
        wheel(&mut f.app, (5, 8), true);
        let drawn = buffer(&mut f.app, 120, 14);
        assert_eq!(row(&drawn, "HOME"), None);
        assert_eq!(row(&drawn, "Folder 00"), Some(2));
        assert_eq!(f.app.focus, Focus::Files, "the keyboard stays");
        wheel(&mut f.app, (5, 8), false);
        let drawn = buffer(&mut f.app, 120, 14);
        assert_eq!(row(&drawn, "HOME"), Some(3));
    }

    #[test]
    fn the_wheel_over_the_shortcuts_scrolls_them_and_clicks_reach_nothing_under_them() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let top = crate::ui::testing::screen(&mut f.app, 120, 24);
        wheel(&mut f.app, (60, 12), true);
        let wheeled = crate::ui::testing::screen(&mut f.app, 120, 24);
        assert_ne!(wheeled, top);
        press(&mut f.app, &[KeyCode::Up, KeyCode::Up, KeyCode::Up]);
        assert_eq!(crate::ui::testing::screen(&mut f.app, 120, 24), top);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Down, KeyCode::Down]);
        assert_eq!(crate::ui::testing::screen(&mut f.app, 120, 24), wheeled);

        // Nothing underneath hears a click, wherever it is.
        for at in [(60, 12), (1, 1), (5, 5), (100, 23)] {
            click(&mut f.app, at);
            right_click(&mut f.app, at);
            wheel(&mut f.app, (1, 23), true);
        }
        assert!(f.app.help_visible && f.app.menu.is_none());
        assert_eq!(crate::ui::testing::screen(&mut f.app, 120, 24), wheeled);
        assert_eq!((f.app.pane().cursor, f.app.focus), (0, Focus::Files));
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn a_click_on_a_dialogs_button_chooses_it_and_one_outside_does_nothing() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(shift(KeyCode::Delete));
        let documents = spot(&mut f.app, "Documents", 24..86);
        click(&mut f.app, documents);
        right_click(&mut f.app, documents);
        assert!(matches!(
            f.app.dialog,
            Some(Dialog::Delete { armed: false, .. })
        ));
        assert!(f.app.menu.is_none());
        assert_eq!(cursor_name(&f.app), "notes.txt");

        let cancel = spot(&mut f.app, "Cancel", 24..96);
        click(&mut f.app, cancel);
        assert!(f.app.dialog.is_none());
        assert!(f.home.join("notes.txt").exists());

        f.app.handle_key(shift(KeyCode::Delete));
        let delete = spot(&mut f.app, "Delete  ", 24..96);
        click(&mut f.app, delete);
        assert!(f.app.dialog.is_none());
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(said(&f.app), Some("Deleted 1 item."));

        // A name: Apply and Cancel are what Enter and Escape are.
        press(&mut f.app, &[KeyCode::Home, KeyCode::F(2)]);
        let cancel = spot(&mut f.app, "Esc Cancel", 24..96);
        click(&mut f.app, cancel);
        assert!(f.app.dialog.is_none());
        press(&mut f.app, &[KeyCode::F(2), KeyCode::Char('s')]);
        let apply = spot(&mut f.app, "⏎ Apply", 24..96);
        click(&mut f.app, apply);
        assert!(f.app.dialog.is_none());
        assert!(f.home.join("Documentss").is_dir());
    }

    #[test]
    fn a_click_in_the_files_closes_the_path_field_or_hands_the_search_over_then_acts() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        let notes = spot(&mut f.app, "notes.txt", 24..86);
        click(&mut f.app, notes);
        assert!(f.app.field.is_none());
        assert_eq!(cursor_name(&f.app), "notes.txt");

        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "jpg");
        finish_search(&mut f.app);
        let found = spot(&mut f.app, "a.jpg", 24..86);
        click(&mut f.app, found);
        assert!(f.app.field.is_none(), "the keyboard is with what was found");
        assert!(f.app.pane().results().is_some(), "still showing it");
        assert_eq!(cursor_name(&f.app), "a.jpg");
        // A click on the field hands the keyboard back to it.
        let field = spot(&mut f.app, "/jpg", 0..86);
        click(&mut f.app, field);
        assert!(matches!(f.app.field, Some(Field::Search)));
    }

    #[test]
    fn drags_moves_releases_and_the_middle_button_do_nothing() {
        let mut f = fixture();
        let notes = spot(&mut f.app, "notes.txt", 24..86);
        for kind in [
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
            MouseEventKind::ScrollLeft,
            MouseEventKind::Down(MouseButton::Middle),
        ] {
            mouse(&mut f.app, kind, notes, KeyModifiers::NONE);
        }
        assert_eq!((f.app.pane().cursor, f.app.focus), (0, Focus::Files));
        assert!(f.app.menu.is_none() && f.app.field.is_none());
    }

    #[test]
    fn without_a_frame_drawn_a_click_lands_nowhere() {
        let mut f = fixture();
        click(&mut f.app, (28, 5));
        right_click(&mut f.app, (28, 5));
        assert_eq!(f.app.pane().cursor, 0);
        assert!(f.app.menu.is_none());
    }
}
