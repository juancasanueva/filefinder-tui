//! Application state and what each key does to it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position as Point, Rect};

use crate::format::DateStyle;
use crate::input::Input;
use crate::keys::{self, Chord, Command, Keymap};
use crate::matcher::{self, Query, Tier};
use crate::model::{self, Entry, SortKey, SortMode};
use crate::ops::{self, Change, History, Kind, Report, Step};
use crate::ops::{Collide, Format};
use crate::places::{self, Section};
use crate::search::{Hit, Looking, Reach, Walk};
use crate::settings::{self, Kind as RowKind, SECTIONS, Setting, Settings};
use crate::state::{self, Saved, Trashed};
use crate::syntax::Highlighter;
use crate::system::{Grep, System, Volume};
use crate::theme::Depth;
use crate::thumbs::{Facts, Thumbs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
    Columns,
    /// Two lists side by side: Ctrl+4.
    Dual,
}

/// The most tabs there can be: `1` to `9` reach each of them.
pub const MOST_TABS: usize = 9;

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

/// Where the keyboard is: in the places panel, in a pane's list, or in the
/// preview pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Places,
    List,
    Preview,
}

/// What the list's right-hand column shows: `I` goes round them.
/// Where Space shows the item under the cursor: the card in the terminal,
/// or macOS's Quick Look window. A setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpaceLook {
    #[default]
    Terminal,
    QuickLook,
}

impl SpaceLook {
    /// As the settings say it.
    pub fn label(self) -> &'static str {
        match self {
            SpaceLook::Terminal => "The terminal",
            SpaceLook::QuickLook => "macOS Quick Look",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Size,
    Modified,
    /// The type and permission bits, the way `ls -l` writes them.
    Mode,
    Owner,
}

impl Detail {
    /// Size → Modified → Mode → Owner → Size.
    pub fn next(self) -> Self {
        match self {
            Detail::Size => Detail::Modified,
            Detail::Modified => Detail::Mode,
            Detail::Mode => Detail::Owner,
            Detail::Owner => Detail::Size,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Detail::Size => "Size",
            Detail::Modified => "Modified",
            Detail::Mode => "Mode",
            Detail::Owner => "Owner",
        }
    }
}

/// What the footer says for a while.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub is_error: bool,
    /// After a change undo can take back: the footer says so.
    pub undoable: bool,
    /// When it was said, and how long it stays.
    pub at: Instant,
    pub lasts: Duration,
}

/// How long a notice stays in the footer.
const NOTICE_LASTS: Duration = Duration::from_secs(4);
/// How long an error, and a receipt for what undo can take back, stay.
const RECEIPT_LASTS: Duration = Duration::from_secs(8);
/// How often the caption of an empty folder changes.
const CAPTION_EVERY: Duration = Duration::from_millis(2800);
/// How many captions an empty folder goes round.
pub const CAPTIONS: usize = 3;

/// How long the first `d` waits for the second.
const TRASH_ARM: Duration = Duration::from_millis(1500);

/// A card in front of the window, asking before something that can't be
/// taken back. While one is open every key goes to it.
pub enum Dialog {
    /// Whether to delete `paths` for good. `armed` is whether the keyboard
    /// is on Delete rather than on Cancel, the default.
    Delete { paths: Vec<PathBuf>, armed: bool },
    /// Whether to empty the trash: to delete for good the `paths` it held
    /// when the question was asked, and nothing that got there since;
    /// `bytes` is what they took up then.
    EmptyTrash {
        paths: Vec<PathBuf>,
        bytes: u64,
        armed: bool,
    },
}

impl Dialog {
    /// Whether the keyboard is on the button that does what it asks.
    pub fn armed(&self) -> bool {
        match self {
            Dialog::Delete { armed, .. } | Dialog::EmptyTrash { armed, .. } => *armed,
        }
    }

    fn armed_mut(&mut self) -> &mut bool {
        match self {
            Dialog::Delete { armed, .. } | Dialog::EmptyTrash { armed, .. } => armed,
        }
    }
}

/// What a line of the menu does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    /// Open With ›: the apps, in a submenu.
    OpenWith,
    /// Open in Terminal: a terminal in the folder, or the item's folder.
    OpenTerminal,
    /// Open With › Default app.
    DefaultApp,
    /// Open With › Quick Look: macOS's own Quick Look window.
    QuickLook,
    /// Compress ›: the formats, in a submenu.
    Compress,
    /// One archive of the selection in a format.
    CompressAs(Format),
    /// Extract, for an archive.
    Extract,
    /// Tags ›: the seven, in a submenu.
    Tags,
    /// One of the seven tags, by its place: on or off over the selection.
    Tag(usize),
    /// Remove All Tags: none of the seven on the selection.
    RemoveTags,
    CopyPath,
    /// Pin or Unpin, for the one folder under the cursor.
    Pin,
    Rename,
    Duplicate,
    Trash,
    Delete,
    ToggleHidden,
    OpenTrash,
    Restore,
    EmptyTrash,
}

impl Action {
    /// The submenu a line opens, for the lines with a `›`.
    fn opens(self) -> Option<Sub> {
        match self {
            Action::OpenWith => Some(Sub::OpenWith),
            Action::Compress => Some(Sub::Compress),
            Action::Tags => Some(Sub::Tags),
            _ => None,
        }
    }
}

/// Which menu is showing: the item's own, or a submenu of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sub {
    Root,
    OpenWith,
    Compress,
    Tags,
}

impl Sub {
    /// The muted capitals over a submenu.
    pub fn title(self) -> Option<&'static str> {
        match self {
            Sub::Root => None,
            Sub::OpenWith => Some("OPEN WITH"),
            Sub::Compress => Some("COMPRESS"),
            Sub::Tags => Some("TAGS"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Item {
        label: &'static str,
        action: Action,
        /// Whether it can be used right now; one that can't is shown dim.
        enabled: bool,
        /// The tag whose colour square goes in place of the glyph.
        tag: Option<usize>,
    },
    /// The rule between two groups.
    Separator,
}

/// The menu for the selection, or for the folder. While it is open every
/// key goes to it.
pub struct Menu {
    pub sub: Sub,
    pub rows: Vec<MenuRow>,
    /// The line the keyboard is on: never a rule.
    pub cursor: usize,
    /// The menu a submenu was opened from, which `h` goes back to.
    pub parent: Option<Box<Menu>>,
}

impl Menu {
    /// Groups of lines with a rule between each two; the groups with
    /// nothing in them are left out.
    fn new(sub: Sub, groups: Vec<Vec<MenuRow>>) -> Self {
        let mut rows = Vec::new();
        for group in groups.into_iter().filter(|group| !group.is_empty()) {
            if !rows.is_empty() {
                rows.push(MenuRow::Separator);
            }
            rows.extend(group);
        }
        Self {
            sub,
            rows,
            cursor: 0,
            parent: None,
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
    pub fn chosen(&self) -> Option<Action> {
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
        tag: None,
    }
}

/// A format as Compress › lists it: its ending, with the dot.
fn dotted(format: Format) -> &'static str {
    match format {
        Format::Zip => ".zip",
        Format::Tar => ".tar",
        Format::TarGz => ".tar.gz",
        Format::TarBz2 => ".tar.bz2",
        Format::TarXz => ".tar.xz",
        Format::SevenZ => ".7z",
    }
}

/// A field on the bottom row that narrows or looks through what the
/// active pane lists; the pane holds its text. While one has the keyboard
/// every key goes to it.
pub enum Field {
    /// `f` and Ctrl+F: Find.
    Search,
    /// `/`: the filter.
    Filter,
}

/// What a prompt on the bottom row asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asking {
    /// A new name for the item at `path`.
    Rename(PathBuf),
    /// The name of a folder to make in this folder.
    NewFolder(PathBuf),
    /// `:` and Ctrl+L: a place to go to.
    GoTo,
}

/// A prompt on the bottom row, in the footer's place. While one is open
/// every key goes to it.
pub struct Prompt {
    pub asking: Asking,
    pub input: Input,
    /// Why Enter didn't take what was typed; typing again clears it.
    pub error: Option<String>,
    /// What Tab would add to a place being typed, shown after it.
    pub ghost: String,
}

impl Prompt {
    /// What the prompt is called on the row.
    pub fn label(&self) -> &'static str {
        match self.asking {
            Asking::Rename(_) => "Rename",
            Asking::NewFolder(_) => "Folder name",
            Asking::GoTo => "Go to",
        }
    }
}

/// A paste that would meet names already taken, waiting for the answer to
/// how to treat them. While it is asked every key goes to it.
pub struct PasteQuestion {
    pub paths: Vec<PathBuf>,
    pub folder: PathBuf,
    /// Whether the items move rather than copy.
    pub moving: bool,
    /// The names in `folder` the paste would meet, in the order of `paths`.
    pub clashes: Vec<String>,
    /// Whether it is F5 or F6 into the other pane rather than a paste.
    pub across: bool,
}

impl PasteQuestion {
    /// The question.
    pub fn text(&self) -> String {
        let folder = match self.folder.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => self.folder.display().to_string(),
        };
        match self.clashes.as_slice() {
            [one] => format!("{one} already exists in {folder}."),
            many => format!("{} items already exist in {folder}.", many.len()),
        }
    }
}

/// What the paste question takes for an answer.
pub const PASTE_KEYS: &str = "enter or b keeps both · r replaces · s skips · esc cancels";

/// What the rename prompt says on the right.
pub const RENAME_HINT: &str = "enter saves · esc cancels · ctrl-a selects the whole name";

/// The commands of the function key bar along the bottom of the panes
/// while two show, in the bar's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnKey {
    /// F2: the rename prompt, as everywhere.
    Rename,
    /// F5: the selection copied into the other pane's folder.
    Copy,
    /// F6: the selection moved into the other pane's folder.
    Move,
    /// F7: the folder name prompt, as `N`.
    NewFolder,
    /// F8: the selection moved to the Trash, as Delete.
    Trash,
    /// F9: an empty `new.md`, then the rename prompt on it.
    NewMarkdown,
    /// F10: an empty `new.txt`, then the rename prompt on it.
    NewText,
}

impl FnKey {
    /// Every command, in the bar's order.
    pub const ALL: [FnKey; 7] = [
        FnKey::Rename,
        FnKey::Copy,
        FnKey::Move,
        FnKey::NewFolder,
        FnKey::Trash,
        FnKey::NewMarkdown,
        FnKey::NewText,
    ];

    /// The number of its function key.
    pub fn number(self) -> u8 {
        match self {
            FnKey::Rename => 2,
            FnKey::Copy => 5,
            FnKey::Move => 6,
            FnKey::NewFolder => 7,
            FnKey::Trash => 8,
            FnKey::NewMarkdown => 9,
            FnKey::NewText => 10,
        }
    }

    /// What the bar says it does.
    pub fn label(self) -> &'static str {
        match self {
            FnKey::Rename => "Rename",
            FnKey::Copy => "Copy",
            FnKey::Move => "Move",
            FnKey::NewFolder => "Mkdir",
            FnKey::Trash => "Delete",
            FnKey::NewMarkdown => "New MD",
            FnKey::NewText => "New TXT",
        }
    }
}

/// What a part of the window stands for, as the last frame drew it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spot {
    /// A pane's files, wherever no item is.
    Pane(usize),
    /// An item of a pane: a row of the list or of its own column, or a tile.
    Item { pane: usize, index: usize },
    /// A row of a folder above the pane's own in the columns view: item
    /// `index` of `ancestors[depth]`.
    Above {
        pane: usize,
        depth: usize,
        index: usize,
    },
    /// A breadcrumb, and the folder it stands for.
    Crumb { pane: usize, path: PathBuf },
    /// A tab's label in the header's strip, by its place among the tabs.
    Tab(usize),
    /// A view's button in the header.
    View(ViewMode),
    /// The settings button in the header, after the views'.
    Gear,
    /// The settings card, wherever nothing else of it is.
    SettingsCard,
    /// A section in the settings' rail.
    SettingsSection(crate::settings::Section),
    /// A row of the settings, by its place among the section's items.
    SettingsRow(usize),
    /// A title in a list's header that sorts: by its key, or the other way
    /// round when the items already are.
    Sort(SortKey),
    /// The places panel, wherever no place is.
    Places,
    /// A place in the panel, by its place among them all.
    Place(usize),
    /// The eject button on a volume's row in the panel, by its place.
    Eject(usize),
    /// The preview pane.
    Preview,
    /// The menu, wherever no line is.
    Menu,
    /// A line of the menu, by its number among its rows.
    MenuRow(usize),
    /// A card that asks, wherever no button is.
    Dialog,
    /// A card's button: the one that does what it asks, or Cancel.
    Choice(bool),
    /// The keyboard shortcuts' card.
    Help,
    /// The Quick Look card, wherever its `×` isn't.
    Look,
    /// The Quick Look card's `×`.
    LookClose,
    /// An item of the function key bar.
    FnKey(FnKey),
}

/// Where the last frame drew what a click can land on. Drawing writes it;
/// the mouse reads it and nothing else, so a click lands on what was on
/// screen under it.
#[derive(Clone, Debug, Default)]
pub struct HitMap {
    /// In the order drawn: a later one is on top of an earlier one.
    spots: Vec<(Rect, Spot)>,
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

/// The in-terminal Quick Look card: Space. It is about the item under the
/// cursor, and follows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// `+` and `-` steps from fitting the picture to the card; `0` fits.
    pub zoom: i8,
    /// How many rows of a text or a folder are scrolled away; drawing
    /// keeps it in range.
    pub scroll: usize,
    /// Rows of the body shown last time, for Page Up and Page Down.
    pub rows: usize,
    /// The item it was last about, to start another at the top.
    about: Option<PathBuf>,
}

/// How far `+` and `-` go each way.
pub const MOST_ZOOM: i8 = 3;

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
    /// The items of the folder at `cwd` the filter leaves standing.
    Filter(Filter),
    /// What Find found at and below a folder, or what was there before
    /// while nothing is typed.
    Search(Search),
}

/// Where Find looks; Tab goes round them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The folder it was started in.
    Folder,
    /// The home folder, from inside it; elsewhere the folder again.
    Home,
    /// Everything from `/` on the system's and home's volumes.
    Everywhere,
}

impl Scope {
    fn next(self) -> Self {
        match self {
            Scope::Folder => Scope::Home,
            Scope::Home => Scope::Everywhere,
            Scope::Everywhere => Scope::Folder,
        }
    }
}

/// What the filter narrows; Tab switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterScope {
    /// The folder's own items.
    Folder,
    /// Those, and every item in the folders below.
    Subfolders,
}

/// The filter in a pane: `/` and what was typed after it.
pub struct Filter {
    pub input: Input,
    pub scope: FilterScope,
    /// Every item it narrows: the folder's own, then what the walk found below.
    all: Vec<Entry>,
    /// How many of `all` are the folder's own.
    own: usize,
    /// The walk listing the folders below, until it has no more to give.
    walk: Option<Walk>,
    /// How many of the items shown are close matches.
    pub close: usize,
}

impl Filter {
    /// How many items it narrows.
    pub fn total(&self) -> usize {
        self.all.len()
    }

    /// Whether items from below may still arrive.
    pub fn is_running(&self) -> bool {
        self.walk.is_some()
    }
}

/// Find in a pane: `f` and what was typed after it.
pub struct Search {
    pub input: Input,
    /// What the pane showed before: where Escape and Back return to.
    origin: Visit,
    /// The walk looking for matches, until it has no more to give.
    walk: Option<Walk>,
    /// Where the cursor belongs once the walk finds one of them; the
    /// earlier in the list, the better.
    wanted: Vec<PathBuf>,
    pub scope: Scope,
    /// Ctrl+G: whether it looks in what files hold rather than at names.
    pub contents: bool,
    /// The folder the walk looks in; what is found is named below it.
    pub root: PathBuf,
    /// Everything the walk found, close matches included, with how well.
    hits: Vec<Hit>,
    /// How many items the walk has looked at.
    pub scanned: usize,
    /// Whether the cursor stays on its item as better ones arrive, rather
    /// than on the best: once the user has put it somewhere.
    held: bool,
    /// Why Ctrl+G was refused, said after the field.
    pub error: Option<String>,
}

impl Search {
    /// Whether matches may still arrive.
    pub fn is_running(&self) -> bool {
        self.walk.is_some()
    }
}

/// What is said when Ctrl+G finds no ripgrep.
pub const NO_RIPGREP: &str = "ripgrep is unavailable.";

/// What Ctrl+C or Ctrl+X took, waiting for Ctrl+V.
struct Clipboard {
    paths: Vec<PathBuf>,
    cut: bool,
}

/// File work under way on a thread of its own.
struct Work {
    /// What the footer says meanwhile.
    label: String,
    cancel: Arc<AtomicBool>,
    results: Receiver<Report>,
    /// The pane F5 or F6 sends to, where what lands is to be selected.
    landing: Option<Landing>,
}

/// The pane F5 or F6 sends to, as it was when the work started: the tab it
/// is in, which of that tab's panes, and the folder it showed.
struct Landing {
    tab: u64,
    pane: usize,
    folder: PathBuf,
}

impl Drop for Work {
    /// Work nobody waits for any more stops at the next file.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What is said when macOS keeps the terminal out of the trash.
const TRASH_REFUSED: &str = "Trash: Permission was refused. Give your terminal \
                             Full Disk Access in System Settings to see it.";

/// A folder in the history, with what was selected there.
#[derive(Clone, Debug)]
struct Visit {
    path: PathBuf,
    selected: Option<String>,
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
    /// The items marked with `v`, a range or Ctrl+A. With none, what a
    /// command acts on is the item under the cursor.
    marks: BTreeSet<usize>,
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

    /// Whether an item is marked.
    pub fn is_marked(&self, index: usize) -> bool {
        self.marks.contains(&index)
    }

    /// Whether an item is selected: what a command acts on. That is the
    /// marked items, or, with none marked, the item under the cursor.
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

    /// `v`: marks the item under the cursor, or unmarks it. The cursor stays.
    fn toggle_mark(&mut self) {
        if self.cursor < self.entries.len() && !self.marks.remove(&self.cursor) {
            self.marks.insert(self.cursor);
        }
    }

    /// `J`, `K`, Shift and the arrows: moves the cursor and marks every item
    /// from where it was to where it lands. What was marked before stays.
    fn extend(&mut self, step: impl FnOnce(&mut Pane)) {
        if self.entries.is_empty() {
            return;
        }
        let from = self.cursor;
        step(self);
        let (first, last) = (from.min(self.cursor), from.max(self.cursor));
        self.marks.extend(first..=last);
    }

    /// `V`: marks what isn't marked and unmarks what is.
    fn invert(&mut self) {
        let all = 0..self.entries.len();
        self.marks = all.filter(|index| !self.marks.contains(index)).collect();
    }

    /// Ctrl+A: marks every item, wherever the cursor is.
    fn select_all(&mut self) {
        self.marks = (0..self.entries.len()).collect();
    }

    /// Unmarks everything: the item under the cursor is what is selected.
    fn collapse(&mut self) {
        self.marks.clear();
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
        self.marks = marks;
    }

    /// Whether the pane lists a folder's own items, filtered or not. Only
    /// there can things be pasted or made.
    pub fn is_folder(&self) -> bool {
        matches!(self.listing, Listing::Folder | Listing::Filter(_))
    }

    /// The filter that is on in the pane.
    pub fn filter(&self) -> Option<&Filter> {
        match &self.listing {
            Listing::Filter(filter) => Some(filter),
            _ => None,
        }
    }

    /// What the items listed match, for drawing it bold: the filter's text
    /// or what Find looks for.
    pub fn matching(&self) -> String {
        match &self.listing {
            Listing::Filter(filter) => filter.input.text(),
            // What Find asks for besides `tag:` terms.
            Listing::Search(search) => find_terms(&search.input.text()).1,
            Listing::Folder => String::new(),
        }
    }

    /// The folder an item is in, below the one looked in, for what Find
    /// and the filter's subfolders list; `None` for one right there.
    pub fn place_of(&self, entry: &Entry) -> Option<String> {
        let root = match &self.listing {
            Listing::Search(search) if !search.input.chars().is_empty() => &search.root,
            Listing::Filter(filter) if filter.scope == FilterScope::Subfolders => &self.cwd,
            _ => return None,
        };
        let parent = entry.path.parent()?;
        let below = parent.strip_prefix(root).ok()?;
        let place = if root == Path::new("/") {
            parent.display().to_string()
        } else {
            below.display().to_string()
        };
        (!below.as_os_str().is_empty()).then_some(place)
    }

    /// Ends the filter and hands it over; the pane lists what it showed.
    fn take_filter(&mut self) -> Option<Filter> {
        self.filter()?;
        match std::mem::replace(&mut self.listing, Listing::Folder) {
            Listing::Filter(filter) => Some(filter),
            _ => None,
        }
    }

    /// Ends the filter: the folder's own items again, the cursor and the
    /// marks on the items they were on.
    fn clear_filter(&mut self) {
        if let Some(filter) = self.take_filter() {
            let mut all = filter.all;
            all.truncate(filter.own);
            self.relisted(all);
        }
    }

    /// Lists these items in place of the ones listed, the cursor on the
    /// item it was on (or the first) and the marks on theirs.
    fn relisted(&mut self, entries: Vec<Entry>) {
        let under = self.selected().map(|entry| entry.path.clone());
        let marked: HashSet<PathBuf> = (self.marks.iter())
            .filter_map(|index| self.entries.get(*index))
            .map(|entry| entry.path.clone())
            .collect();
        self.entries = entries;
        let found = |path: &PathBuf| self.entries.iter().position(|entry| entry.path == *path);
        self.cursor = under.as_ref().and_then(found).unwrap_or(0);
        self.marks = (self.entries.iter().enumerate())
            .filter(|(_, entry)| marked.contains(&entry.path))
            .map(|(index, _)| index)
            .collect();
    }

    /// Lists what the filter leaves standing: the exact matches, or with
    /// none of them the close ones, best first.
    fn narrow(&mut self) {
        let Listing::Filter(filter) = &mut self.listing else {
            return;
        };
        let names = filter.all.iter().map(|entry| entry.name.as_str());
        let (shown, close) = match matcher::narrow(&filter.input.text(), names) {
            None => ((0..filter.all.len()).collect(), 0),
            Some(narrowed) if !narrowed.exact.is_empty() => (narrowed.exact, 0),
            Some(narrowed) => {
                let close = narrowed.close.len();
                (narrowed.close, close)
            }
        };
        filter.close = close;
        let entries = shown
            .iter()
            .map(|index| filter.all[*index].clone())
            .collect();
        self.relisted(entries);
    }

    /// Lists what Find found, best first, and only the close matches when
    /// nothing matches at the start of a word or better. The cursor stays
    /// on its item once the user has put it somewhere, and otherwise on
    /// the best; one wanted goes first.
    fn rank(&mut self) {
        let Listing::Search(search) = &mut self.listing else {
            return;
        };
        let strong = (search.hits.iter()).any(|hit| hit.score.tier >= Tier::WordStart);
        let mut shown: Vec<&Hit> = (search.hits.iter())
            .filter(|hit| !strong || hit.score.tier != Tier::Typo)
            .collect();
        shown.sort_by(|a, b| matcher::rank(&a.score, &a.path, &b.score, &b.path));
        let entries: Vec<Entry> = shown.iter().map(|hit| hit.entry.clone()).collect();
        let held = search.held;
        let wanted = (search.wanted.iter().enumerate())
            .find_map(|(rank, path)| Some((rank, entries.iter().position(|e| e.path == *path)?)));
        if let Some((rank, _)) = wanted {
            // Only one wanted more than this one moves the cursor again.
            search.wanted.truncate(rank);
            search.held = true;
        }
        if !held && wanted.is_none() {
            self.cursor = 0;
            self.offset = 0;
            self.entries.clear();
        }
        self.relisted(entries);
        if let Some((_, index)) = wanted {
            self.cursor = index;
            self.marks.clear();
        }
    }

    /// The user takes over: the cursor stays on its item from now on.
    fn hold(&mut self) {
        if let Listing::Search(search) = &mut self.listing {
            search.held = true;
        }
    }

    /// Has every copy of an item this pane holds carry these tags, as they
    /// are now on disk: in its list, its parent's, the folders above and
    /// what a filter or a search keeps aside.
    fn retagged(&mut self, path: &Path, tags: u8) {
        let kept: Vec<&mut Entry> = match &mut self.listing {
            Listing::Filter(filter) => filter.all.iter_mut().collect(),
            Listing::Search(search) => search.hits.iter_mut().map(|hit| &mut hit.entry).collect(),
            Listing::Folder => Vec::new(),
        };
        let above = self
            .ancestors
            .iter_mut()
            .flat_map(|above| &mut above.entries);
        let copies = (self.entries.iter_mut()).chain(above).chain(kept);
        copies
            .filter(|entry| entry.path == path)
            .for_each(|entry| entry.tags = tags);
        // Find asked for tags this item no longer carries: it isn't found.
        if let Listing::Search(search) = &mut self.listing {
            let (wanted, _) = find_terms(&search.input.text());
            if !wanted.is_empty() && !crate::tags::carries(&crate::tags::labels(path), &wanted) {
                search.hits.retain(|hit| hit.entry.path != path);
                self.rank();
            }
        }
    }

    /// Whether the filter or Find has something to look for, `tag:`
    /// terms alone included.
    fn asks(&self) -> bool {
        match &self.listing {
            Listing::Filter(filter) => !filter.input.text().is_empty(),
            Listing::Search(search) => !search.input.text().is_empty(),
            Listing::Folder => false,
        }
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
            // A filtered folder comes back whole, on the item the cursor
            // was on when that is the folder's own.
            Listing::Folder | Listing::Filter(_) => Visit {
                path: self.cwd.clone(),
                selected: (self.selected())
                    .filter(|entry| entry.path.parent() == Some(self.cwd.as_path()))
                    .map(|entry| entry.name.clone()),
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

    /// Takes in what Find, or the filter's walk below, found since the
    /// last turn.
    fn collect_found(&mut self) {
        match &mut self.listing {
            Listing::Search(search) => {
                let Some(walk) = &mut search.walk else { return };
                let (found, finished) = walk.collect();
                search.scanned = walk.scanned();
                if finished {
                    search.walk = None;
                }
                if !found.is_empty() || search.hits.is_empty() {
                    search.hits.extend(found);
                    self.rank();
                }
            }
            Listing::Filter(filter) => {
                let Some(walk) = &mut filter.walk else { return };
                let (found, finished) = walk.collect();
                if finished {
                    filter.walk = None;
                }
                if !found.is_empty() {
                    filter.all.extend(found.into_iter().map(|hit| hit.entry));
                    self.narrow();
                }
            }
            Listing::Folder => {}
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

    /// Ctrl+click or Cmd+click: the cursor on an item, and the item marked
    /// or unmarked.
    fn toggle(&mut self, index: usize) {
        if index < self.entries.len() {
            self.cursor = index;
            self.toggle_mark();
        }
    }

    /// Shift+click: the items from the cursor to this one are what is
    /// selected, and the cursor goes there.
    fn mark_range(&mut self, index: usize) {
        if index < self.entries.len() {
            let from = self.cursor;
            self.cursor = index;
            self.marks = (from.min(index)..=from.max(index)).collect();
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

/// One tab: a whole way of browsing, with its view and its pane or two,
/// each with its folder, history, cursor, marks and listing.
pub struct Tab {
    /// Which tab this is, whatever its place in the strip.
    id: u64,
    pub panes: Vec<Pane>,
    /// The pane that has the keyboard.
    pub active: usize,
    pub view: ViewMode,
    /// The view two panes were opened from, where Ctrl+4 returns.
    before: ViewMode,
    /// What F5 or F6 brought into one of its panes while it wasn't
    /// showing, to be selected once it shows and has listed it.
    landed: Option<(Landing, Vec<PathBuf>)>,
}

impl Tab {
    fn new(cwd: PathBuf, view: ViewMode) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            panes: vec![Pane::new(cwd, Vec::new())],
            active: 0,
            view,
            before: view,
            landed: None,
        }
    }

    /// The view with one pane: the view itself, or the one two panes were
    /// opened from.
    pub fn single_view(&self) -> ViewMode {
        match self.view {
            ViewMode::Dual => self.before,
            view => view,
        }
    }

    /// The pane that has the keyboard.
    pub fn pane(&self) -> &Pane {
        &self.panes[self.active]
    }
}

pub struct Config {
    pub home: PathBuf,
    /// A folder to open, or a file to select in its folder.
    pub start: PathBuf,
    pub date_style: DateStyle,
    /// Where the view, the tile size and the preview are remembered;
    /// `None` remembers nothing.
    pub state_dir: Option<PathBuf>,
    /// What makes the thumbnails; `None` leaves every file its stand-in.
    pub thumbs: Option<Thumbs>,
    /// What colours code in the preview; `None` shows it plain.
    pub highlighter: Option<Highlighter>,
    /// Where Delete moves things: the trash folder.
    pub trash_dir: PathBuf,
    /// Where the mounted volumes are, which the panel lists under DEVICES.
    pub volumes_dir: PathBuf,
    /// The applications folder, first under PLACES when it exists.
    pub applications_dir: PathBuf,
    /// How many colours the terminal shows.
    pub depth: Depth,
}

pub struct App {
    pub home: PathBuf,
    /// The tabs, in the order the strip shows them; never none.
    pub tabs: Vec<Tab>,
    /// Which of them is showing.
    pub current: usize,
    pub focus: Focus,
    pub tile: TileSize,
    pub sort: SortMode,
    /// What the columns view's right-hand column shows: `I`. The list and
    /// the two panes show every column under a header.
    pub detail: Detail,
    pub show_hidden: bool,
    /// Whether the preview pane shows beside the list: Alt+P.
    pub preview_visible: bool,
    /// What the places panel lists, as last read.
    pub places: Vec<Section>,
    /// The colour tags some file carries, as Spotlight said last.
    tags_in_use: u8,
    /// Whether the places panel shows left of the files: Ctrl+B.
    pub places_visible: bool,
    /// The place the panel's cursor is on, among them all.
    pub places_cursor: usize,
    /// The first line of the panel shown; drawing keeps it in range.
    pub places_offset: usize,
    /// How many rows of the preview's body are scrolled away.
    pub preview_scroll: usize,
    /// Rows of the preview's body shown last time, for paging it.
    pub preview_rows: usize,
    /// The item the preview was last about, to scroll back to the top for another.
    previewed: Option<PathBuf>,
    /// How many colours the terminal shows.
    pub depth: Depth,
    pub help_visible: bool,
    /// The header button the pointer rests on (a view's or the settings'),
    /// which says what it does under it until the pointer leaves, a key or
    /// a click.
    pub tip: Option<Spot>,
    /// The settings card, while it is open: `,` or the gear.
    pub settings: Option<crate::settings::Settings>,
    /// Where Space shows the item under the cursor: a setting.
    pub space_look: SpaceLook,
    /// Which of the list's columns show, in their order (Size, User,
    /// Permissions, Kind, Modified): a setting.
    pub columns: [bool; 5],
    /// Every pinned folder, in the order pinned, the ones that are gone
    /// included: they keep their line in the file and are only left out of
    /// the panel.
    pub pins: Vec<PathBuf>,
    /// How many lines the shortcuts are scrolled down; drawing keeps it in range.
    pub help_scroll: usize,
    /// Lines of shortcuts shown last time, for Page Up and Page Down.
    pub help_rows: usize,
    /// What the shortcuts are filtered by: `/` on the sheet types it.
    pub help_filter: Input,
    /// Whether keys on the sheet go into its filter.
    pub help_typing: bool,
    pub dialog: Option<Dialog>,
    /// The menu that is open: `m`.
    pub menu: Option<Menu>,
    /// The Quick Look card, while it is open: Space.
    pub look: Option<Look>,
    /// The field in the header that has the keyboard.
    pub field: Option<Field>,
    /// The prompt on the bottom row that has the keyboard.
    pub prompt: Option<Prompt>,
    /// The paste waiting for the answer to its question on the bottom row.
    pub question: Option<PasteQuestion>,
    /// When the first `d` was pressed, while the second may still follow.
    trash_armed: Option<Instant>,
    /// The browsing commands' keys, from the keys file.
    pub keymap: Keymap,
    /// Whether the terminal was handed to another program, so that the
    /// next frame has to be drawn whole.
    redraw: bool,
    pub message: Option<Message>,
    /// Which caption an empty folder shows; `tick` moves it on.
    pub caption: usize,
    /// Since when the active pane has shown an empty folder.
    empty_since: Option<Instant>,
    /// The volume each folder the footer named is on, asked once.
    volumes: HashMap<PathBuf, Option<Volume>>,
    /// Whether each mounted volume the panel listed is on the Mac's own
    /// disk, asked once until Ctrl+R.
    internal_volumes: HashMap<PathBuf, bool>,
    pub should_quit: bool,
    pub date_style: DateStyle,
    /// What colours code in the preview.
    pub highlighter: Option<Highlighter>,
    pub thumbs: Option<Thumbs>,
    /// What was read about the selected item for the preview pane.
    pub facts: Option<Facts>,
    clipboard: Option<Clipboard>,
    history: History,
    work: Option<Work>,
    /// Where what this app moved to the trash came from, and when: one
    /// record for each item, as long as it is there.
    trashed: Vec<Trashed>,
    trash_dir: PathBuf,
    volumes_dir: PathBuf,
    applications_dir: PathBuf,
    state_dir: Option<PathBuf>,
    system: Box<dyn System>,
    /// Where the last frame drew what a click can land on.
    pub hits: HitMap,
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
        let trashed = config
            .state_dir
            .as_deref()
            .map(state::load_trashed)
            .unwrap_or_default();
        let (keymap, key_problems) = config
            .state_dir
            .as_deref()
            .map(keys::load)
            .unwrap_or_default();
        let mut app = Self {
            // Every launch opens one pane in the list, whatever view was used last.
            tabs: vec![Tab::new(config.home.clone(), ViewMode::List)],
            current: 0,
            home: config.home,
            focus: Focus::List,
            tile: saved.tile,
            sort: saved.sort,
            detail: saved.detail,
            show_hidden: saved.hidden,
            preview_visible: saved.preview,
            places: Vec::new(),
            tags_in_use: 0,
            places_visible: saved.places,
            places_cursor: 0,
            places_offset: 0,
            preview_scroll: 0,
            preview_rows: 1,
            previewed: None,
            depth: config.depth,
            help_visible: false,
            tip: None,
            settings: None,
            space_look: saved.space,
            columns: saved.columns,
            pins: config
                .state_dir
                .as_deref()
                .map(state::load_pins)
                .unwrap_or_default(),
            help_scroll: 0,
            help_rows: 1,
            help_filter: Input::default(),
            help_typing: false,
            dialog: None,
            menu: None,
            look: None,
            field: None,
            prompt: None,
            question: None,
            trash_armed: None,
            keymap,
            redraw: false,
            message: None,
            caption: 0,
            empty_since: None,
            volumes: HashMap::new(),
            internal_volumes: HashMap::new(),
            should_quit: false,
            date_style: config.date_style,
            thumbs: config.thumbs,
            highlighter: config.highlighter,
            facts: None,
            clipboard: None,
            history: History::default(),
            work: None,
            trashed,
            trash_dir: config.trash_dir,
            volumes_dir: config.volumes_dir,
            applications_dir: config.applications_dir,
            state_dir: config.state_dir,
            system,
            hits: HitMap::default(),
            last_click: None,
            clock: Box::new(Instant::now),
        };
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
        app.read_tags_in_use();
        app.read_places();
        if let Some(first) = key_problems.first() {
            let more = key_problems.len() - 1;
            app.fail(match more {
                0 => first.clone(),
                _ => format!("{first} (and {more} more)"),
            });
        }
        app
    }

    /// The tab that is showing: what every key and click acts on.
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.current]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.current]
    }

    /// The panes of the tab that is showing.
    pub fn panes(&self) -> &[Pane] {
        &self.tab().panes
    }

    /// Which of them has the keyboard.
    pub fn active(&self) -> usize {
        self.tab().active
    }

    /// The view of the tab that is showing.
    pub fn view(&self) -> ViewMode {
        self.tab().view
    }

    pub fn pane(&self) -> &Pane {
        self.tab().pane()
    }

    fn pane_mut(&mut self) -> &mut Pane {
        let tab = self.tab_mut();
        &mut tab.panes[tab.active]
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.pane().selected()
    }

    /// What the file work under way is doing, in the footer's words.
    pub fn working(&self) -> Option<&str> {
        self.work.as_ref().map(|work| work.label.as_str())
    }

    /// Whether the preview pane shows: beside a single pane in every view,
    /// never beside two.
    pub fn shows_preview(&self) -> bool {
        self.preview_visible && self.view() != ViewMode::Dual
    }

    /// Whether the function key bar shows along the bottom of the panes:
    /// while two panes do.
    pub fn shows_fn_bar(&self) -> bool {
        self.view() == ViewMode::Dual
    }

    /// Whether a command of the function key bar can run now: never while
    /// a prompt, the paste question or a field has the keyboard, nor while
    /// a menu, a card, the key sheet, the settings or Quick Look is open;
    /// Rename for one item, Copy and Move for a selection and a folder in
    /// the other pane, Delete for a selection outside the Trash, and the
    /// new folder and the new files in a folder.
    pub fn can_run(&self, command: FnKey) -> bool {
        let typing = self.prompt.is_some() || self.question.is_some() || self.field.is_some();
        let over = self.menu.is_some()
            || self.dialog.is_some()
            || self.help_visible
            || self.settings.is_some()
            || self.look.is_some();
        if typing || over || !self.shows_fn_bar() {
            return false;
        }
        let selected = self.pane().selection().count();
        match command {
            FnKey::Rename => selected == 1,
            FnKey::Copy | FnKey::Move => {
                selected > 0 && self.other_pane().is_some_and(Pane::is_folder)
            }
            // Never a delete for good: in the Trash there is nothing to do.
            FnKey::Trash => selected > 0 && !self.in_trash(),
            FnKey::NewFolder | FnKey::NewMarkdown | FnKey::NewText => self.pane().is_folder(),
        }
    }

    /// The pane without the keyboard, while two show.
    fn other_pane(&self) -> Option<&Pane> {
        let active = self.active();
        let others = self.panes().iter().enumerate();
        others
            .filter(|(index, _)| *index != active)
            .map(|(_, pane)| pane)
            .next()
    }

    /// F5 to F10 in two panes, and a click on an item of the bar: its
    /// command, when it can run.
    fn run_fn_key(&mut self, command: FnKey) {
        if !self.can_run(command) {
            return;
        }
        match command {
            FnKey::Rename => self.ask_rename(),
            FnKey::Copy => self.send_to_other_pane(false),
            FnKey::Move => self.send_to_other_pane(true),
            FnKey::NewFolder => self.ask_new_folder(),
            FnKey::Trash => self.trash(),
            FnKey::NewMarkdown => self.new_file("md"),
            FnKey::NewText => self.new_file("txt"),
        }
    }

    /// The volume the active pane's folder is on, as `tick` last asked.
    pub fn volume(&self) -> Option<&Volume> {
        self.volumes.get(&self.pane().cwd)?.as_ref()
    }

    /// What every turn of the event loop does besides keys: takes in the file
    /// work that finished, what a search found and the thumbnails made since
    /// the last one, reads what the preview shows of a newly selected item
    /// and the volume of a newly shown folder, lets what the footer says go
    /// when its time is up and moves an empty folder's caption on, so that
    /// drawing never has to.
    pub fn tick(&mut self) {
        let now = (self.clock)();
        self.collect_work();
        // A search goes on in a tab that isn't showing.
        for pane in self.tabs.iter_mut().flat_map(|tab| &mut tab.panes) {
            pane.collect_found();
        }
        if let Some(thumbs) = &mut self.thumbs {
            thumbs.collect();
        }
        if let Some(highlighter) = &mut self.highlighter {
            highlighter.collect();
        }
        let (hidden, sort) = (self.show_hidden, self.sort);
        let selected = self.tabs[self.current].pane().selected();
        let current = match (&self.facts, selected) {
            (Some(facts), Some(entry)) => facts.describes(entry, hidden, sort),
            (None, None) => true,
            _ => false,
        };
        if !current {
            self.facts = selected.map(|entry| Facts::read(entry, hidden, sort));
        }
        // Another item: its preview starts at the top.
        let under_cursor = selected.map(|entry| entry.path.clone());
        if let Some(look) = &mut self.look
            && look.about != under_cursor
        {
            look.about = under_cursor.clone();
            look.scroll = 0;
        }
        if under_cursor != self.previewed {
            self.previewed = under_cursor;
            self.preview_scroll = 0;
        }
        // The folders the preview shows on screen are counted here, not
        // while drawing.
        if let Some(Some(Ok(children))) = self.facts.as_ref().map(|facts| facts.children.as_ref()) {
            let shown = children
                .iter()
                .skip(self.preview_scroll)
                .take(self.preview_rows);
            shown.for_each(|child| {
                child.item_count();
            });
        }
        let folder = self.pane().cwd.clone();
        if !self.volumes.contains_key(&folder) {
            let volume = self.system.volume(&folder);
            self.volumes.insert(folder, volume);
        }
        if let Some(message) = &self.message
            && now.saturating_duration_since(message.at) >= message.lasts
        {
            self.message = None;
        }
        let empty = self.pane().entries.is_empty() && self.pane().filter().is_none();
        let empty = empty && self.pane().is_folder();
        match (empty, self.empty_since) {
            (false, _) => {
                self.empty_since = None;
                self.caption = 0;
            }
            (true, None) => {
                self.empty_since = Some(now);
                self.caption = 0;
            }
            (true, Some(since)) => {
                let beats =
                    now.saturating_duration_since(since).as_millis() / CAPTION_EVERY.as_millis();
                self.caption = (beats % CAPTIONS as u128) as usize;
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.tip = None;
        // A key is the user taking over: the cursor is no longer moved to
        // what a search under way was asked to find again, and one in the
        // files stays on its item as better ones arrive.
        self.pane_mut().want(Vec::new());
        if self.field.is_none() {
            self.pane_mut().hold();
        }
        // Whatever this key is, a first `d` waits for no other.
        let armed = self.trash_armed.take();
        if let Some(menu) = self.menu.take() {
            self.menu_key(menu, key);
            return;
        }
        if self.help_visible {
            // The shortcuts keep every key: these scroll, filter or close
            // them, and no other reaches the files underneath.
            let page = self.help_rows.max(1);
            if self.help_typing {
                // Typing: letters go into the filter; Enter keeps it,
                // Escape clears it, the arrows and page keys still scroll.
                match key.code {
                    KeyCode::Enter => self.help_typing = false,
                    KeyCode::Esc => self.clear_help_filter(),
                    KeyCode::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                    KeyCode::Down => self.help_scroll = self.help_scroll.saturating_add(1),
                    KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(page),
                    KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(page),
                    _ => {
                        if self.help_filter.handle_key(key) {
                            self.help_scroll = 0;
                        }
                    }
                }
                return;
            }
            match key.code {
                KeyCode::Char('/') => self.help_typing = true,
                KeyCode::Esc if !self.help_filter.chars().is_empty() => self.clear_help_filter(),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?' | 'q') => {
                    self.help_visible = false
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(page),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(page),
                _ => {}
            }
            return;
        }
        if self.settings.is_some() {
            return self.settings_key(key);
        }
        if let Some(dialog) = self.dialog.take() {
            self.dialog_key(dialog, key);
            return;
        }
        if self.look.is_some() {
            self.look_key(key);
            return;
        }
        if let Some(prompt) = self.prompt.take() {
            self.prompt_key(prompt, key);
            return;
        }
        if let Some(question) = self.question.take() {
            self.question_key(question, key);
            return;
        }
        match self.field.take() {
            Some(Field::Search) => return self.search_key(key),
            Some(Field::Filter) => return self.filter_key(key),
            None => {}
        }
        // The settings open wherever the keyboard is.
        let held = key.modifiers;
        if key.code == KeyCode::Char(',')
            && !held.intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return self.open_settings();
        }
        if self.focus == Focus::Preview && self.preview_key(key) {
            return;
        }
        if self.focus == Focus::Places {
            return self.places_key(key);
        }
        self.command_key(key, armed);
    }

    /// The command keys, over the files of the active pane: the commands
    /// the keys file gives the key, tried in the file's order until one can
    /// run here; a key with Shift that runs none runs those of the key
    /// without it, unless it is a plain letter's capital.
    fn command_key(&mut self, key: KeyEvent, armed: Option<Instant>) {
        let Some(chord) = Chord::of(key) else {
            return;
        };
        if !self.run_chord(chord, armed, false)
            && let Some(unshifted) = chord.unshifted()
        {
            self.run_chord(unshifted, armed, true);
        }
    }

    /// Runs the first command on a chord that can run here; false if none.
    /// Run for a key whose Shift ran nothing, the two panes' commands are
    /// left out: Shift+F8 is not F8.
    fn run_chord(&mut self, chord: Chord, armed: Option<Instant>, unshifted: bool) -> bool {
        let commands: Vec<Command> = self
            .keymap
            .commands(chord)
            .filter(|command| !(unshifted && command.two_panes()))
            .collect();
        commands
            .into_iter()
            .any(|command| self.run_command(command, armed))
    }

    /// Runs a browsing command, or says it can't run here with false: the
    /// function key bar's only while it shows, the grid's only in the grid.
    fn run_command(&mut self, command: Command, armed: Option<Instant>) -> bool {
        let grid = self.view() == ViewMode::Grid;
        let bar = self.shows_fn_bar();
        match command {
            // Changing files.
            Command::PaneCopy if bar => self.run_fn_key(FnKey::Copy),
            Command::PaneMove if bar => self.run_fn_key(FnKey::Move),
            Command::PaneNewFolder if bar => self.run_fn_key(FnKey::NewFolder),
            Command::PaneTrash if bar => self.run_fn_key(FnKey::Trash),
            Command::PaneNewMarkdown if bar => self.run_fn_key(FnKey::NewMarkdown),
            Command::PaneNewText if bar => self.run_fn_key(FnKey::NewText),
            Command::PaneCopy
            | Command::PaneMove
            | Command::PaneNewFolder
            | Command::PaneTrash
            | Command::PaneNewMarkdown
            | Command::PaneNewText => return false,
            Command::Trash => self.trash_key(armed),
            Command::TrashNow => self.delete_key(),
            Command::DeleteForever => self.ask_delete(),
            Command::Copy => self.take(false),
            Command::Cut => self.take(true),
            Command::Paste => self.paste(false),
            Command::MoveHere => self.paste(true),
            Command::Duplicate => self.duplicate(),
            Command::CopyFolderPath => self.copy_folder_path(),
            Command::CopyPath => self.copy_path(),
            Command::Rename => self.ask_rename(),
            Command::NewFolder => self.ask_new_folder(),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Edit => self.edit_file(),
            Command::Tags => self.open_tags(),
            Command::Pin => self.toggle_pin(),
            Command::Terminal => self.open_terminal(),
            // Selecting.
            Command::Select => self.pane_mut().toggle_mark(),
            Command::InvertSelection => self.pane_mut().invert(),
            Command::SelectAll => self.pane_mut().select_all(),
            Command::ExtendNext => self.extend(1),
            Command::ExtendPrevious => self.extend(-1),
            Command::ExtendDown => self.extend_row(true),
            Command::ExtendUp => self.extend_row(false),
            Command::ExtendRight if grid => self.extend(1),
            Command::ExtendLeft if grid => self.extend(-1),
            // Moving.
            Command::GridLeft if grid => self.move_by(-1),
            Command::GridRight if grid => self.move_by(1),
            Command::ExtendRight | Command::ExtendLeft | Command::GridLeft | Command::GridRight => {
                return false;
            }
            Command::Down => self.step(1),
            Command::Up => self.step(-1),
            Command::Parent => self.go_up(),
            Command::BrowseIn => self.browse_in(),
            Command::Open => self.open_selected(),
            Command::Back => self.go_back(),
            Command::Forward => self.go_forward(),
            Command::First => self.move_by(isize::MIN / 2),
            Command::Last => self.move_by(isize::MAX / 2),
            Command::PageDown => self.page(1),
            Command::PageUp => self.page(-1),
            Command::FocusNext => self.move_focus(1),
            Command::FocusPrevious => self.move_focus(-1),
            Command::Escape => self.escape(),
            // Looking.
            Command::Help => {
                self.open_help();
                self.help_scroll = 0;
            }
            // The card in the terminal, or macOS's window: a setting.
            Command::QuickLook => match self.space_look {
                SpaceLook::Terminal => self.open_look(),
                SpaceLook::QuickLook => self.preview_selected(),
            },
            Command::Hidden => self.toggle_hidden(),
            Command::Detail => self.next_detail(),
            Command::Filter => self.open_filter(),
            Command::Find => self.search(),
            Command::Reveal => self.reveal(),
            Command::Sort => self.set_sort(self.sort.next()),
            Command::ReverseSort => self.set_sort(self.sort.reverse()),
            Command::GoTo => self.ask_go_to(),
            Command::Refresh => self.refresh(),
            Command::Preview => self.toggle_preview(),
            Command::Places => self.toggle_places(),
            Command::Menu => self.open_menu(),
            // Views, panes and the window.
            Command::ListView => self.set_view(ViewMode::List),
            Command::ColumnsView => self.set_view(ViewMode::Columns),
            Command::GridView => self.set_view(ViewMode::Grid),
            Command::DualPane => self.toggle_split(),
            Command::ZoomIn => self.set_tile(self.tile.bigger()),
            Command::ZoomOut => self.set_tile(self.tile.smaller()),
            // Tabs.
            Command::NewTab => self.new_tab(),
            Command::OpenInNewTab => self.open_in_new_tab(),
            Command::CloseTab => self.close_tab(),
            Command::NextTab => self.step_tab(1),
            Command::PreviousTab => self.step_tab(-1),
            Command::MoveTabRight => self.move_tab(1),
            Command::MoveTabLeft => self.move_tab(-1),
            Command::Tab1 => self.go_to_tab(0),
            Command::Tab2 => self.go_to_tab(1),
            Command::Tab3 => self.go_to_tab(2),
            Command::Tab4 => self.go_to_tab(3),
            Command::Tab5 => self.go_to_tab(4),
            Command::Tab6 => self.go_to_tab(5),
            Command::Tab7 => self.go_to_tab(6),
            Command::Tab8 => self.go_to_tab(7),
            Command::Tab9 => self.go_to_tab(8),
            Command::Quit => self.quit(),
        }
        true
    }

    /// The mouse twin of `handle_key`: a click, a double
    /// click, a right click or a turn of the wheel lands on what the last
    /// frame drew under it and does what the key for the same thing does.
    /// While a menu or a card is open, moving over its lines and buttons
    /// puts the keyboard there. Drags, releases and the buttons crossterm
    /// has no name for do nothing.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let spot = self.hits.at(mouse.column, mouse.row).cloned();
        match mouse.kind {
            MouseEventKind::Moved => return self.hover(spot),
            MouseEventKind::Down(MouseButton::Middle) => return,
            MouseEventKind::Down(_) | MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {}
            _ => return,
        }
        // As with a key, the user takes over from a search that was asked
        // to put the cursor somewhere.
        for pane in &mut self.tab_mut().panes {
            pane.want(Vec::new());
            pane.hold();
        }
        match mouse.kind {
            // A click or a turn of the wheel is the user doing something
            // else, and may move the cursor: a first `d` waits for no second.
            MouseEventKind::ScrollDown => {
                self.trash_armed = None;
                self.wheel(spot, true);
            }
            MouseEventKind::ScrollUp => {
                self.trash_armed = None;
                self.wheel(spot, false);
            }
            MouseEventKind::Down(button) => {
                self.trash_armed = None;
                self.tip = None;
                self.click(button, spot, mouse.modifiers);
            }
            _ => {}
        }
    }

    /// Whether moving the mouse is worth hearing about: while a menu or a
    /// card with buttons is open, whose lines and buttons follow it, and
    /// while the header's view buttons show, which say what they do when the
    /// pointer rests on them. Asked of the last frame drawn.
    pub fn wants_motion(&self) -> bool {
        let buttons = self.hits.rect_of(&Spot::View(ViewMode::List)).is_some();
        self.menu.is_some() || self.dialog.is_some() || buttons
    }

    /// The mouse moved over a menu's line or a card's button: the keyboard
    /// goes there, as the arrows would take it.
    fn hover(&mut self, spot: Option<Spot>) {
        self.tip = spot
            .clone()
            .filter(|spot| matches!(spot, Spot::View(_) | Spot::Gear));
        match (spot, &mut self.menu, &mut self.dialog) {
            (Some(Spot::MenuRow(row)), Some(menu), _) => {
                if matches!(menu.rows.get(row), Some(MenuRow::Item { .. })) {
                    menu.cursor = row;
                }
            }
            (Some(Spot::Choice(yes)), _, Some(dialog)) => *dialog.armed_mut() = yes,
            _ => {}
        }
    }

    fn click(&mut self, button: MouseButton, spot: Option<Spot>, held: KeyModifiers) {
        let last = self.last_click.take();
        let left = button == MouseButton::Left;
        // The menu and the cards keep the clicks to themselves, and one
        // outside them closes them.
        if self.menu.is_some() {
            match spot {
                Some(Spot::MenuRow(row)) if left => self.choose_row(row),
                Some(Spot::Menu | Spot::MenuRow(_)) => {}
                _ => self.menu = None,
            }
            return;
        }
        if let Some(mut open) = self.settings {
            match spot {
                // A section: its rows, the keyboard in the rail.
                Some(Spot::SettingsSection(section)) if left => {
                    open.section = section;
                    open.in_rail = true;
                    open.cursor = 0;
                    self.settings = Some(open);
                }
                // A row: the keyboard there, and what Enter does.
                Some(Spot::SettingsRow(index)) if left => {
                    open.cursor = index;
                    open.in_rail = false;
                    self.settings = Some(open);
                    let items = settings::items(&settings::rows(self, open.section));
                    if let Some(setting) = items.get(index) {
                        self.change_setting(*setting, 1);
                    }
                }
                Some(Spot::SettingsCard | Spot::SettingsSection(_) | Spot::SettingsRow(_)) => {}
                _ => self.settings = None,
            }
            return;
        }
        if self.help_visible {
            if left && spot != Some(Spot::Help) {
                self.help_visible = false;
            }
            return;
        }
        if self.dialog.is_some() {
            match spot {
                Some(Spot::Choice(yes)) if left => self.choose(yes),
                Some(Spot::Dialog | Spot::Choice(_)) => {}
                // Outside the card, as Escape: nothing done.
                _ if left => self.dialog = None,
                _ => {}
            }
            return;
        }
        if self.look.is_some() {
            // Its `×`, or anywhere outside it.
            if left && spot != Some(Spot::Look) {
                self.look = None;
            }
            return;
        }
        // A prompt or the paste question on the bottom row waits for keys.
        if self.prompt.is_some() || self.question.is_some() {
            return;
        }
        let Some(spot) = spot else { return };
        // The function key bar, before a field gives the keyboard back:
        // nothing in it runs while one is open.
        if let Spot::FnKey(command) = spot {
            if left {
                self.run_fn_key(command);
            }
            return;
        }
        self.leave_field();
        match (left, spot) {
            (true, Spot::Item { pane, index }) => self.click_item(pane, index, held, last),
            (true, Spot::Above { pane, depth, index }) => {
                self.click_above(pane, depth, index);
            }
            (true, Spot::Pane(pane)) => self.enter_pane(pane),
            (true, Spot::Place(index)) => self.open_place(index),
            (true, Spot::Eject(index)) => self.eject_place(index),
            (true, Spot::Places) => self.enter_places(),
            (true, Spot::View(view)) => self.choose_view(view),
            (true, Spot::Gear) => self.open_settings(),
            (true, Spot::Sort(key)) => self.sort_by(key),
            (true, Spot::Preview) => self.focus_preview(),
            (true, Spot::Crumb { pane, path }) => {
                let tab = self.tab_mut();
                tab.active = pane.min(tab.panes.len() - 1);
                self.navigate(&path);
            }
            (true, Spot::Tab(index)) => self.switch_tab(index),
            (false, Spot::Item { pane, index }) => {
                self.enter_pane(pane);
                let pane = self.pane_mut();
                if index < pane.entries.len() {
                    // Inside the selection it is about all of it.
                    if !pane.is_selected(index) {
                        pane.collapse();
                    }
                    pane.point_at(index);
                    self.open_menu();
                }
            }
            (false, Spot::Above { pane, depth, index }) if self.click_above(pane, depth, index) => {
                self.open_menu();
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

    /// A click on a card's button: Enter on the button that does what it
    /// asks, or Escape.
    fn choose(&mut self, yes: bool) {
        if !yes {
            return self.handle_key(KeyEvent::from(KeyCode::Esc));
        }
        if let Some(dialog) = &mut self.dialog {
            *dialog.armed_mut() = true;
        }
        self.handle_key(KeyEvent::from(KeyCode::Enter));
    }

    /// A click in the files while a field has the keyboard: the keyboard
    /// goes to what it lists, as with Enter.
    fn leave_field(&mut self) {
        if let Some(field) = self.field.take() {
            self.commit(field);
        }
    }

    /// Enter in a field: the keyboard goes to what the pane lists, which
    /// stays as it is. With nothing typed there is nothing to keep.
    fn commit(&mut self, field: Field) {
        self.field = None;
        if self.pane().asks() {
            self.pane_mut().hold();
            return;
        }
        match field {
            Field::Search => self.leave_search(),
            Field::Filter => self.pane_mut().clear_filter(),
        }
    }

    /// The keyboard goes to a pane's files.
    fn enter_pane(&mut self, pane: usize) {
        self.focus = Focus::List;
        let tab = self.tab_mut();
        tab.active = pane.min(tab.panes.len() - 1);
    }

    /// The keyboard goes to the preview pane, when it shows.
    fn focus_preview(&mut self) {
        if self.shows_preview() {
            self.focus = Focus::Preview;
        }
    }

    /// The keyboard goes to the places panel, read again, with its cursor
    /// on the place holding the active pane's folder.
    fn enter_places(&mut self) {
        if !self.places_visible {
            return;
        }
        self.read_places();
        self.focus = Focus::Places;
        if let Some(current) = self.current_place() {
            self.places_cursor = current;
        }
    }

    /// Reads what the panel lists again, keeping its cursor among them.
    fn read_places(&mut self) {
        let (system, asked) = (&self.system, &mut self.internal_volumes);
        let mut internal = |volume: &Path| {
            let known = asked.get(volume).copied();
            known.unwrap_or_else(|| {
                let internal = system.internal(volume);
                asked.insert(volume.to_path_buf(), internal);
                internal
            })
        };
        self.places = places::read(
            &self.applications_dir,
            &self.home,
            &self.trash_dir,
            &self.volumes_dir,
            self.tags_in_use,
            &self.pins,
            &mut internal,
        );
        let last = places::flat(&self.places).count().saturating_sub(1);
        self.places_cursor = self.places_cursor.min(last);
    }

    /// Ctrl+P: pins the folder showing to the panel, or unpins it.
    fn toggle_pin(&mut self) {
        if !self.pane().is_folder() || self.pane().search().is_some() {
            self.note("Only folders can be pinned.");
            return;
        }
        let folder = self.pane().cwd.clone();
        self.toggle_pin_of(folder);
    }

    /// Pin or Unpin, in the menu: the folder under the cursor.
    fn pin_selected(&mut self) {
        let folder = self.selected().filter(|entry| entry.is_dir);
        if let Some(folder) = folder.map(|entry| entry.path.clone()) {
            self.toggle_pin_of(folder);
        }
    }

    /// Ctrl+P on a pinned folder in the panel: unpins it.
    fn unpin_place(&mut self, index: usize) {
        let place = places::flat(&self.places).nth(index);
        let Some(place) = place.filter(|place| place.kind == places::Kind::Pinned) else {
            return;
        };
        let folder = place.path.clone();
        self.toggle_pin_of(folder);
    }

    /// The name the panel lists a folder under, in PLACES or DEVICES,
    /// where it needs no pin; none when it isn't there.
    fn listed_as(&self, folder: &Path) -> Option<String> {
        places::flat(&self.places)
            .find(|place| {
                !matches!(place.kind, places::Kind::Pinned | places::Kind::Tag(_))
                    && place.path == folder
            })
            .map(|place| place.name.clone())
    }

    /// Pins a folder, or unpins it when it is pinned. One the panel lists
    /// already, under PLACES or DEVICES, isn't pinned again.
    fn toggle_pin_of(&mut self, folder: PathBuf) {
        let name = places::label(&folder);
        let mut pins = self.pins.clone();
        let listed = self.listed_as(&folder);
        let note = if let Some(index) = pins.iter().position(|pin| *pin == folder) {
            pins.remove(index);
            format!("Unpinned {name}")
        } else if let Some(listed) = listed {
            self.note(&format!("{listed} is already in the panel."));
            return;
        } else {
            pins.push(folder);
            format!("Pinned {name}")
        };
        let saved = match &self.state_dir {
            Some(dir) => state::save_pins(dir, &pins),
            None => Ok(()),
        };
        match saved {
            Ok(()) => {
                self.pins = pins;
                self.read_places();
                self.note(&note);
            }
            Err(_) => self.fail(format!("Couldn't save the pin to {name}.")),
        }
    }

    /// The place the panel lifts: the deepest one holding the active
    /// pane's folder.
    /// A tag's row while Find lists what carries that tag alone.
    pub fn current_place(&self) -> Option<usize> {
        let asked = self.pane().search().map(|search| search.input.text());
        let tag_row = asked.and_then(|asked| {
            places::flat(&self.places).position(|place| {
                matches!(place.kind, places::Kind::Tag(_))
                    && asked.trim().eq_ignore_ascii_case(&tag_query(&place.name))
            })
        });
        tag_row.or_else(|| places::current(&self.places, &self.pane().cwd))
    }

    /// Asks Spotlight which colour tags some file carries, for the panel;
    /// at start, on Ctrl+R and after this app changes tags, never per frame.
    fn read_tags_in_use(&mut self) {
        self.tags_in_use = self.system.tags_in_use().unwrap_or(0);
    }

    /// Enter, `l` or a click on a place: the active pane goes there, as a
    /// new place in its history, and the keyboard to its list. The trash
    /// says so when it can't be listed, and the keyboard stays.
    fn open_place(&mut self, index: usize) {
        let Some(place) = places::flat(&self.places).nth(index) else {
            return;
        };
        self.places_cursor = index;
        if let places::Kind::Tag(_) = place.kind {
            // What carries the tag, anywhere, with the keyboard in the results.
            let query = tag_query(&place.name);
            self.search();
            if let Listing::Search(search) = &mut self.pane_mut().listing {
                search.input = Input::new(&query);
                search.scope = Scope::Everywhere;
            }
            self.run_search();
            self.field = None;
            self.focus = Focus::List;
            return;
        }
        let path = place.path.clone();
        if self.go_to(&path, None) {
            self.focus = Focus::List;
        }
    }

    /// Keys while the places panel has the keyboard: these move its cursor
    /// and open a place; the window's own keys (views, tabs, panes, the
    /// shortcuts, quitting) do what they do anywhere; the keys that act on
    /// files do nothing.
    fn places_key(&mut self, key: KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let last = places::flat(&self.places).count().saturating_sub(1);
        let cursor = self.places_cursor;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down if plain => {
                self.places_cursor = (cursor + 1).min(last)
            }
            KeyCode::Char('k') | KeyCode::Up if plain => {
                self.places_cursor = cursor.saturating_sub(1)
            }
            KeyCode::Char('g') | KeyCode::Home if plain => self.places_cursor = 0,
            KeyCode::Char('G') | KeyCode::End if plain => self.places_cursor = last,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if plain => {
                self.open_place(cursor)
            }
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                self.unpin_place(cursor)
            }
            KeyCode::Char('e') if plain => self.eject_place(cursor),
            _ if self.window_key(key) => self.command_key(key, None),
            _ => {}
        }
    }

    /// `e` in the panel or a click on a volume's `⏏`: ejects it, as Finder
    /// does. Panes showing a folder on it go home; the panel lists it no
    /// more. Anything else under the cursor is left alone.
    fn eject_place(&mut self, index: usize) {
        let place = places::flat(&self.places).nth(index);
        let Some(place) = place.filter(|place| place.ejectable) else {
            return;
        };
        let (name, volume) = (place.name.clone(), place.path.clone());
        if self.system.eject(&volume).is_err() {
            self.fail(format!("Couldn't eject {name}. Something may be using it."));
            return;
        }
        let (current, home) = (self.current, self.home.clone());
        for tab in 0..self.tabs.len() {
            self.current = tab;
            for pane in 0..self.panes().len() {
                if self.panes()[pane].cwd.starts_with(&volume) {
                    self.in_pane(pane, |app| app.navigate(&home));
                }
            }
        }
        self.current = current;
        self.volumes.clear();
        self.read_places();
        self.note(&format!("Ejected {name}"));
    }

    /// Ctrl+B: hides or shows the places panel, and remembers it. Hidden,
    /// it gives the keyboard to the list.
    fn toggle_places(&mut self) {
        self.places_visible = !self.places_visible;
        if !self.places_visible && self.focus == Focus::Places {
            self.focus = Focus::List;
        }
        self.remember();
    }

    /// A click on a view's button in the header: what Ctrl+1 to Ctrl+4 do.
    fn choose_view(&mut self, view: ViewMode) {
        match view {
            ViewMode::Dual => self.toggle_split(),
            view => self.set_view(view),
        }
    }

    /// A left click on an item: the cursor there and the
    /// marks cleared; with Shift the range from the cursor to it is what is
    /// selected; with Ctrl (or Cmd) it alone is marked or unmarked. A second
    /// plain click on it within 400 ms opens it like Enter, or among what
    /// Find found shows it in its folder like `o`.
    fn click_item(&mut self, pane: usize, index: usize, held: KeyModifiers, last: Option<Click>) {
        let now = (self.clock)();
        self.enter_pane(pane);
        let pane = self.pane_mut();
        let Some(path) = pane.entries.get(index).map(|entry| entry.path.clone()) else {
            return;
        };
        if held.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) {
            // With nothing marked yet, the item a plain click chose just
            // before is marked too: it was the selection.
            let before = pane.cursor;
            let chose = |click: &Click| {
                let under = pane.entries.get(before);
                under.is_some_and(|entry| entry.path == click.path)
            };
            if pane.marks.is_empty() && before != index && last.as_ref().is_some_and(chose) {
                pane.marks.insert(before);
            }
            return pane.toggle(index);
        }
        if held.contains(KeyModifiers::SHIFT) {
            return pane.mark_range(index);
        }
        pane.point_at(index);
        pane.collapse();
        let active = self.active();
        let again = |click: &Click| {
            click.pane == active
                && click.path == path
                && now.saturating_duration_since(click.at) <= DOUBLE_CLICK
        };
        if last.as_ref().is_some_and(again) {
            if self.pane().results().is_some() {
                self.reveal();
            } else {
                self.open_selected();
            }
        } else {
            self.last_click = Some(Click {
                pane: active,
                path,
                at: now,
            });
        }
    }

    /// A click on a row of a folder above the pane's own in the columns
    /// view: goes to that folder with the row selected. False when it couldn't.
    fn click_above(&mut self, pane: usize, depth: usize, index: usize) -> bool {
        let now = (self.clock)();
        let above = self
            .panes()
            .get(pane)
            .and_then(|it| it.ancestors.get(depth));
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
            let pane = self.active();
            self.last_click = Some(Click {
                pane,
                path,
                at: now,
            });
        }
        arrived
    }

    /// Shows the shortcuts, every one of them: no filter, not typing.
    fn open_help(&mut self) {
        self.help_visible = true;
        self.clear_help_filter();
    }

    /// Takes the filter off the shortcuts and stops typing into it.
    fn clear_help_filter(&mut self) {
        self.help_filter = Input::default();
        self.help_typing = false;
        self.help_scroll = 0;
    }

    /// A turn of the wheel: over a pane, its cursor one row further, as
    /// `j` and `k`; over the preview, the shortcuts or the Quick Look card,
    /// they scroll a row. Under the menu or a card that asks nothing does.
    fn wheel(&mut self, spot: Option<Spot>, down: bool) {
        let rows: isize = if down { 1 } else { -1 };
        if self.menu.is_some() || self.dialog.is_some() {
            return;
        }
        if self.help_visible {
            if spot == Some(Spot::Help) {
                self.help_scroll = self.help_scroll.saturating_add_signed(rows);
            }
            return;
        }
        if let Some(look) = &mut self.look {
            if matches!(spot, Some(Spot::Look | Spot::LookClose)) {
                look.scroll = look.scroll.saturating_add_signed(rows);
            }
            return;
        }
        match spot {
            Some(Spot::Item { pane, .. } | Spot::Above { pane, .. } | Spot::Pane(pane)) => {
                self.enter_pane(pane);
                self.step(rows);
            }
            Some(Spot::Preview) => self.scroll_preview(rows),
            // The panel scrolls a line; drawing keeps it in range.
            Some(Spot::Places | Spot::Place(_) | Spot::Eject(_)) => {
                self.places_offset = self.places_offset.saturating_add_signed(rows)
            }
            _ => {}
        }
    }

    /// The first `d` says what the second does; a second one soon after
    /// moves the selection to the trash. An empty folder has nothing to
    /// arm for.
    fn trash_key(&mut self, armed: Option<Instant>) {
        let now = (self.clock)();
        if armed.is_some_and(|at| now.saturating_duration_since(at) < TRASH_ARM) {
            return self.delete_key();
        }
        if self.pane().entries.is_empty() {
            return;
        }
        self.trash_armed = Some(now);
        let what = if self.selection().len() > 1 {
            "them"
        } else {
            "it"
        };
        // The keys file's keys, where it changes them.
        let again = self.keymap.first(Command::Trash, "d");
        let alone = match self.keymap.first(Command::TrashNow, "Delete") {
            key if key.is_empty() => String::new(),
            key => format!(", or press {key} on its own"),
        };
        let said = if self.in_trash() {
            format!("Press {again} again to review permanent deletion{alone}.")
        } else {
            format!("Press {again} again to move {what} to Trash{alone}.")
        };
        self.note(&said);
    }

    /// `m`, Menu and Shift+F10, and a right click: the menu for the
    /// selection, under the item at the cursor.
    fn open_menu(&mut self) {
        let menu = Menu::new(Sub::Root, self.root_menu());
        if !menu.rows.is_empty() {
            self.menu = Some(menu);
        }
    }

    /// The menu's lines for what is selected in the active pane, in its
    /// groups and order. What can't apply to it is left out, or dimmed.
    fn root_menu(&self) -> Vec<Vec<MenuRow>> {
        let hidden = if self.show_hidden {
            "Hide Hidden Files"
        } else {
            "Show Hidden Files"
        };
        let mut view = vec![item(hidden, Action::ToggleHidden)];
        let selection = self.selection();
        if self.in_trash() {
            let items = match selection.is_empty() {
                true => Vec::new(),
                false => vec![
                    item("Restore", Action::Restore),
                    item("Delete Permanently…", Action::Delete),
                ],
            };
            let empty = vec![item("Empty Trash…", Action::EmptyTrash)];
            return vec![items, empty, view];
        }
        view.push(item("Open Trash", Action::OpenTrash));
        let Some(entry) = self.selected() else {
            // An empty folder: a terminal in it.
            return vec![vec![item("Open in Terminal", Action::OpenTerminal)], view];
        };
        let one = selection.len() == 1;
        // An archive goes beside its items: they have to be in one folder.
        let together = selection
            .iter()
            .all(|path| path.parent() == selection[0].parent());
        let archive = one && !entry.is_dir && self.unpacks(&entry.path);
        let archives = [
            Some(item_if(together, "Compress", Action::Compress)),
            archive.then(|| item("Extract", Action::Extract)),
        ];
        vec![
            vec![
                item("Open", Action::Open),
                item_if(one, "Open With", Action::OpenWith),
                item("Open in Terminal", Action::OpenTerminal),
            ],
            archives.into_iter().flatten().collect(),
            [
                Some(item("Tags", Action::Tags)),
                Some(item("Copy Path", Action::CopyPath)),
                // For one folder the panel doesn't list already.
                (one && entry.is_dir && self.listed_as(&entry.path).is_none()).then(|| match self
                    .pins
                    .contains(&entry.path)
                {
                    true => item("Unpin", Action::Pin),
                    false => item("Pin", Action::Pin),
                }),
            ]
            .into_iter()
            .flatten()
            .collect(),
            vec![
                item("Rename", Action::Rename),
                item("Duplicate", Action::Duplicate),
            ],
            vec![
                item("Move to Trash", Action::Trash),
                item("Delete Permanently…", Action::Delete),
            ],
            view,
        ]
    }

    /// The lines of a submenu.
    fn submenu(&self, sub: Sub) -> Vec<Vec<MenuRow>> {
        match sub {
            Sub::Root => self.root_menu(),
            Sub::OpenWith => vec![vec![
                item("Default app", Action::DefaultApp),
                item("Quick Look", Action::QuickLook),
            ]],
            Sub::Compress => {
                let formats = self.system.formats().into_iter();
                let rows = formats.map(|format| item(dotted(format), Action::CompressAs(format)));
                vec![rows.collect()]
            }
            Sub::Tags => self.tag_rows(),
        }
    }

    /// The seven tags, each with its colour, and Remove All Tags while one
    /// of them is on any of the selection.
    fn tag_rows(&self) -> Vec<Vec<MenuRow>> {
        let paths = self.selection();
        let tagged = paths
            .iter()
            .any(|path| crate::tags::mask(&crate::tags::labels(path)) != 0);
        let seven = (0..crate::tags::NAMES.len()).map(|index| MenuRow::Item {
            label: crate::tags::NAMES[index],
            action: Action::Tag(index),
            enabled: true,
            tag: Some(index),
        });
        let mut groups = vec![seven.collect()];
        if tagged {
            groups.push(vec![item("Remove All Tags", Action::RemoveTags)]);
        }
        groups
    }

    /// A key pressed while the menu is open: it goes nowhere else. `j`,
    /// `k` and the arrows move, `l`, Right and Enter choose or open a
    /// submenu, `h` and Left go back out of one, Escape closes them all.
    fn menu_key(&mut self, mut menu: Menu, key: KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            // Closed, with nothing done.
            KeyCode::Esc => return,
            KeyCode::Up => menu.step(false),
            KeyCode::Down => menu.step(true),
            KeyCode::Char('k') if plain => menu.step(false),
            KeyCode::Char('j') if plain => menu.step(true),
            KeyCode::Enter | KeyCode::Right => return self.choose_line(menu),
            KeyCode::Char('l') if plain => return self.choose_line(menu),
            KeyCode::Left => return self.menu_back(menu),
            KeyCode::Char('h') if plain => return self.menu_back(menu),
            // In the tags' menu a digit is the tag of that place.
            KeyCode::Char(digit @ '1'..='7') if menu.sub == Sub::Tags => {
                return self.run(Action::Tag(digit as usize - '1' as usize));
            }
            _ => {}
        }
        self.menu = Some(menu);
    }

    /// Enter on a line: its submenu opens over it, or it is done. A dim
    /// line does nothing.
    fn choose_line(&mut self, menu: Menu) {
        let Some(action) = menu.chosen() else {
            self.menu = Some(menu);
            return;
        };
        match action.opens() {
            Some(sub) => {
                let mut opened = Menu::new(sub, self.submenu(sub));
                opened.parent = Some(Box::new(menu));
                self.menu = Some(opened);
            }
            None => self.run(action),
        }
    }

    /// `h` and Left: out of a submenu, back to the menu it opened from; one
    /// opened on its own, like `T`'s, closes.
    fn menu_back(&mut self, mut menu: Menu) {
        self.menu = match menu.parent.take() {
            Some(parent) => Some(*parent),
            None if menu.sub != Sub::Root => None,
            None => Some(menu),
        };
    }

    /// Does what a line of the menu says, with the code its key runs.
    fn run(&mut self, action: Action) {
        match action {
            Action::Open => self.open_selected(),
            Action::DefaultApp => self.open_in_default_app(),
            Action::QuickLook => self.preview_selected(),
            Action::CompressAs(format) => self.compress(format),
            Action::Extract => self.extract(),
            Action::Tag(index) => self.retag(ops::Tagging::Toggle(index)),
            Action::RemoveTags => self.retag(ops::Tagging::RemoveAll),
            Action::CopyPath => self.copy_path(),
            Action::Pin => self.pin_selected(),
            Action::Rename => self.ask_rename(),
            Action::Duplicate => self.duplicate(),
            Action::Trash => self.trash(),
            Action::Delete => self.ask_delete(),
            Action::ToggleHidden => self.toggle_hidden(),
            Action::OpenTerminal => self.open_terminal(),
            Action::OpenTrash => {
                let trash = self.trash_dir.clone();
                self.navigate(&trash);
            }
            Action::Restore => self.restore(),
            Action::EmptyTrash => self.ask_empty_trash(),
            // The lines with a `›` open their submenu instead.
            Action::OpenWith | Action::Compress | Action::Tags => {}
        }
    }

    /// Open With › Default app: the item under the cursor in the app the
    /// system opens it with, a folder too.
    fn open_in_default_app(&mut self) {
        let Some(entry) = self.selected() else { return };
        let (path, name) = (entry.path.clone(), entry.name.clone());
        if self.system.open(&path).is_err() {
            self.fail(format!("No app is set up to open {name}."));
        }
    }

    /// `T`: the seven tags for the selection, each with its colour, and
    /// Remove All Tags while one of them is on any of it.
    fn open_tags(&mut self) {
        if self.selection().is_empty() || self.in_trash() {
            return;
        }
        self.menu = Some(Menu::new(Sub::Tags, self.tag_rows()));
    }

    /// Changes the tags of the selection and says what it did.
    fn retag(&mut self, tagging: ops::Tagging) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        match ops::retag(&paths, tagging) {
            Ok((what, change)) => {
                let items = ops::count(change.steps.len());
                let said = match what {
                    ops::Retagged::Added(index) => {
                        format!("Tagged {items} {}", crate::tags::NAMES[index])
                    }
                    ops::Retagged::Removed(index) => {
                        format!("Removed {} from {items}", crate::tags::NAMES[index])
                    }
                    ops::Retagged::Cleared => format!("Removed all tags from {items}"),
                };
                let undoable = !change.steps.is_empty();
                // Shown as they are now, and the marks stay where they are.
                for path in change.landed() {
                    let tags = crate::tags::mask(&crate::tags::labels(&path));
                    let panes = self.tabs.iter_mut().flat_map(|tab| &mut tab.panes);
                    panes.for_each(|pane| pane.retagged(&path, tags));
                }
                self.history.record(change);
                self.read_tags_in_use();
                self.read_places();
                self.receipt(said, undoable);
            }
            Err(error) => self.fail(error.to_string()),
        }
    }

    /// A key pressed while a card asks: it goes nowhere else. Left and Right
    /// go to Cancel and to the other button, Tab between them; Enter or
    /// Space presses the one the keyboard is on, Escape cancels.
    fn dialog_key(&mut self, mut dialog: Dialog, key: KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let armed = dialog.armed_mut();
        match key.code {
            // Closed, with nothing done.
            KeyCode::Esc => return,
            KeyCode::Enter | KeyCode::Char(' ') if plain => {
                self.answer(dialog);
                return;
            }
            KeyCode::Left => *armed = false,
            KeyCode::Right => *armed = true,
            KeyCode::Char('h') if plain => *armed = false,
            KeyCode::Char('l') if plain => *armed = true,
            KeyCode::Tab | KeyCode::BackTab => *armed = !*armed,
            _ => {}
        }
        self.dialog = Some(dialog);
    }

    /// `:` and Ctrl+L: asks on the bottom row for a place to go to.
    fn ask_go_to(&mut self) {
        let mut prompt = Prompt {
            asking: Asking::GoTo,
            input: Input::default(),
            error: None,
            ghost: String::new(),
        };
        self.suggest(&mut prompt);
        self.prompt = Some(prompt);
    }

    /// A path the way the header spells it: `~` for the home folder.
    pub fn shown_path(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }

    /// A key pressed while a prompt is open: it goes nowhere else.
    fn prompt_key(&mut self, mut prompt: Prompt, key: KeyEvent) {
        let command = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
        match key.code {
            // Closed, with nothing done.
            KeyCode::Esc => return,
            KeyCode::Enter => return self.answer_prompt(prompt),
            KeyCode::Tab if prompt.asking == Asking::GoTo => {
                if let Some(filled) = self.completed(&prompt.input.text()) {
                    prompt.input = Input::new(&filled);
                    prompt.error = None;
                }
            }
            KeyCode::Char('a' | 'A') if command => prompt.input.select_all(),
            _ => {
                if prompt.input.handle_key(key) {
                    prompt.error = None;
                }
            }
        }
        self.suggest(&mut prompt);
        self.prompt = Some(prompt);
    }

    /// What Tab would add to the place being typed, shown after it.
    fn suggest(&self, prompt: &mut Prompt) {
        prompt.ghost = match prompt.asking {
            Asking::GoTo => {
                let typed = prompt.input.text();
                let filled = self.completed(&typed).unwrap_or_default();
                filled.strip_prefix(&typed).unwrap_or_default().to_string()
            }
            _ => String::new(),
        };
    }

    /// Enter in a prompt: does what it asked for. When that can't be done
    /// the prompt stays, saying why.
    fn answer_prompt(&mut self, mut prompt: Prompt) {
        let text = prompt.input.text();
        let refused = match &prompt.asking {
            Asking::GoTo => self.go_to_typed(&text).err(),
            Asking::Rename(path) => {
                let path = path.clone();
                name_refusal(&text).or_else(|| match ops::rename(&path, &text) {
                    Ok(Some(change)) => {
                        self.changed(change, "Renamed".to_string());
                        None
                    }
                    Ok(None) => None,
                    Err(error) => Some(error.to_string()),
                })
            }
            Asking::NewFolder(folder) => {
                let folder = folder.clone();
                name_refusal(&text).or_else(|| match ops::new_folder(&folder, &text) {
                    Ok(Some(change)) => {
                        self.changed(change, "Folder created".to_string());
                        None
                    }
                    Ok(None) => None,
                    Err(error) => Some(error.to_string()),
                })
            }
        };
        if let Some(why) = refused {
            prompt.error = Some(why);
            self.prompt = Some(prompt);
        }
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

    /// Enter in Go to: goes to the folder typed, or to a file's folder with
    /// the file selected. Why not, when it can't.
    fn go_to_typed(&mut self, typed: &str) -> Result<(), String> {
        let typed = typed.trim();
        if typed.is_empty() {
            return Ok(());
        }
        let target = self.resolve(typed);
        let arrived = match Entry::from_path(target.clone()) {
            Ok(entry) if entry.is_dir => self.go_to(&target, None),
            Ok(entry) => {
                let folder = target.parent().unwrap_or(&target);
                self.go_to(folder, Some(&entry.name))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(format!("{typed} does not exist or cannot be reached."));
            }
            Err(error) => return Err(read_error(&target, &error)),
        };
        if arrived {
            return Ok(());
        }
        // What went wrong was said in the footer: the prompt says it instead.
        let said = self.message.take().filter(|message| message.is_error);
        Err(said.map_or_else(|| format!("{typed} couldn't be opened."), |said| said.text))
    }

    /// What Tab makes of a place being typed: the folder name being typed
    /// filled in as far as the folders that start like it agree. `None`
    /// when nothing would change. The folder is listed once.
    fn completed(&self, text: &str) -> Option<String> {
        if text == "~" {
            return Some("~/".to_string());
        }
        let (before, typed) = text.split_at(text.rfind('/').map_or(0, |slash| slash + 1));
        let hidden = self.show_hidden || typed.starts_with('.');
        let entries = model::list_dir(&self.resolve(before), hidden, SortMode::Name).ok()?;
        let start = typed.to_lowercase();
        let folders: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.is_dir && entry.name.to_lowercase().starts_with(&start))
            .map(|entry| entry.name.as_str())
            .collect();
        let filled = match folders.as_slice() {
            [] => return None,
            [only] => format!("{only}/"),
            [first, others @ ..] => common_start(first, others),
        };
        let filled = format!("{before}{filled}");
        (filled != text).then_some(filled)
    }

    /// `/`: opens the filter on the bottom row, or hands the keyboard back
    /// to the one that is on. What Find found is not filtered.
    fn open_filter(&mut self) {
        if self.pane().search().is_some() {
            return;
        }
        if self.pane().filter().is_none() {
            let pane = self.pane_mut();
            let all = pane.entries.clone();
            pane.listing = Listing::Filter(Filter {
                input: Input::default(),
                scope: FilterScope::Folder,
                own: all.len(),
                all,
                walk: None,
                close: 0,
            });
        }
        self.focus = Focus::List;
        self.field = Some(Field::Filter);
    }

    /// A key pressed while the filter's field has the keyboard: what isn't
    /// typed into it narrows the folder, keeps it or clears it.
    fn filter_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => return self.pane_mut().clear_filter(),
            KeyCode::Enter | KeyCode::Down if plain => return self.commit(Field::Filter),
            // Everything shown marked, and the keyboard on to it.
            KeyCode::Char('a') if ctrl => {
                self.commit(Field::Filter);
                return self.pane_mut().select_all();
            }
            KeyCode::Menu | KeyCode::F(10)
                if key.code == KeyCode::Menu || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.commit(Field::Filter);
                return self.open_menu();
            }
            KeyCode::Tab => self.switch_filter_scope(),
            _ => {
                if let Listing::Filter(filter) = &mut self.pane_mut().listing
                    && filter.input.handle_key(key)
                {
                    self.pane_mut().narrow();
                }
            }
        }
        self.field = Some(Field::Filter);
    }

    /// Tab in the filter: this folder, or this folder and every folder below.
    fn switch_filter_scope(&mut self) {
        let hidden = self.show_hidden;
        let pane = self.pane_mut();
        let cwd = pane.cwd.clone();
        let Listing::Filter(filter) = &mut pane.listing else {
            return;
        };
        match filter.scope {
            FilterScope::Folder => {
                filter.scope = FilterScope::Subfolders;
                let reach = Reach::volume(cwd);
                filter.walk = Some(Walk::start(reach, Looking::Below, hidden));
            }
            FilterScope::Subfolders => {
                filter.scope = FilterScope::Folder;
                filter.walk = None;
                filter.all.truncate(filter.own);
            }
        }
        pane.narrow();
    }

    /// `f` and Ctrl+F: opens Find on the bottom row, or hands the keyboard
    /// back to the one that is open.
    fn search(&mut self) {
        if self.pane().search().is_none() {
            let origin = self.pane().visit();
            // A filter goes as Find comes: its folder is found in whole.
            if self.pane().filter().is_some() {
                self.revisit(&origin);
            }
            let root = origin.path.clone();
            self.pane_mut().listing = Listing::Search(Search {
                input: Input::default(),
                origin,
                walk: None,
                wanted: Vec::new(),
                scope: Scope::Folder,
                contents: false,
                root,
                hits: Vec::new(),
                scanned: 0,
                held: false,
                error: None,
            });
        }
        self.focus = Focus::List;
        self.field = Some(Field::Search);
    }

    /// A key pressed while Find's field has the keyboard: what isn't typed
    /// into it changes where or how it looks, keeps what it found or leaves.
    fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => {
                // A walk under way stops first; then Find is left.
                if !self.stop_search() {
                    return self.leave_search();
                }
            }
            KeyCode::Enter | KeyCode::Down if plain => return self.commit(Field::Search),
            // The menu is for what was found: the keyboard moves on to
            // that, and the menu opens.
            KeyCode::Menu | KeyCode::F(10)
                if key.code == KeyCode::Menu || key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                self.commit(Field::Search);
                return self.open_menu();
            }
            KeyCode::Tab => self.next_scope(),
            KeyCode::Char('g') if ctrl => self.toggle_contents(),
            _ => {
                let mut retyped = false;
                if let Listing::Search(search) = &mut self.pane_mut().listing {
                    let before = search.input.text();
                    search.input.handle_key(key);
                    retyped = search.input.text() != before;
                    if retyped {
                        search.error = None;
                    }
                }
                if retyped {
                    self.run_search();
                }
            }
        }
        if self.pane().search().is_some() {
            self.field = Some(Field::Search);
        }
    }

    /// Stops the walk under way, keeping what it found; false when none was.
    fn stop_search(&mut self) -> bool {
        match &mut self.pane_mut().listing {
            Listing::Search(search) if search.is_running() => {
                search.walk = None;
                true
            }
            _ => false,
        }
    }

    /// The folder a scope of Find looks in, from the folder it was started
    /// in; `None` for everywhere.
    fn scope_root(&self, folder: &Path, scope: Scope) -> Option<PathBuf> {
        match scope {
            Scope::Folder => Some(folder.to_path_buf()),
            Scope::Home if folder.starts_with(&self.home) => Some(self.home.clone()),
            Scope::Home => Some(folder.to_path_buf()),
            Scope::Everywhere => None,
        }
    }

    /// Tab in Find: the next scope that looks somewhere else.
    fn next_scope(&mut self) {
        let Some(search) = self.pane().search() else {
            return;
        };
        let folder = search.origin.path.clone();
        let here = self.scope_root(&folder, search.scope);
        let mut scope = search.scope.next();
        if self.scope_root(&folder, scope) == here {
            scope = scope.next();
        }
        if let Listing::Search(search) = &mut self.pane_mut().listing {
            search.scope = scope;
        }
        self.run_search();
    }

    /// Ctrl+G in Find: contents rather than names, or back; with no
    /// ripgrep, names it stays, and it says so.
    fn toggle_contents(&mut self) {
        let can = self.system.can_grep();
        if let Listing::Search(search) = &mut self.pane_mut().listing {
            if search.contents || can {
                search.contents = !search.contents;
                search.error = None;
            } else {
                search.error = Some(NO_RIPGREP.to_string());
                return;
            }
        }
        self.run_search();
    }

    /// Where Find looks: `in ~/x`, `everywhere`, with
    /// `contents` in front when it looks in what files hold.
    pub fn scope_label(&self, search: &Search) -> String {
        let root = self.scope_root(&search.origin.path, search.scope);
        let place = match root {
            Some(root) => format!("in {}", self.shown_path(&root)),
            None => "everywhere".to_string(),
        };
        if search.contents {
            format!("contents {place}")
        } else {
            place
        }
    }

    /// Lists what Find's field asks for: what was there before while it
    /// is empty, and otherwise what a walk started afresh finds.
    fn run_search(&mut self) {
        let Some(mut search) = self.pane_mut().take_search() else {
            return;
        };
        let text = search.input.text();
        search.wanted.clear();
        // The walk under way looked for something else: dropped, it stops.
        search.walk = None;
        search.hits.clear();
        search.scanned = 0;
        search.held = false;
        // `tag:red` asks for a tag; the rest is looked for as it is.
        let (tags, words) = find_terms(&text);
        let query = Query::new(&words);
        if query.is_none() && tags.is_empty() {
            self.revisit(&search.origin);
            self.pane_mut().listing = Listing::Search(search);
            return;
        }
        let folder = search.origin.path.clone();
        let reach = match self.scope_root(&folder, search.scope) {
            Some(root) => Reach::volume(root),
            None => Reach::everywhere(&self.home),
        };
        let grep = if query.is_none() && search.scope == Scope::Everywhere {
            // Only tags, anywhere: Spotlight knows where they are. A folder
            // or home is walked, exact where Spotlight doesn't index.
            let spotlight = self.system.spotlight(&tags, None);
            spotlight.map(|grep| shown_only(grep, None, self.show_hidden))
        } else if search.contents {
            self.system.grep(&reach.roots(), &words, self.show_hidden)
        } else {
            None
        };
        let looking = match grep {
            Some(grep) => Looking::Contents { grep, tags },
            None => Looking::Names { query, tags },
        };
        search.root = reach.root.clone();
        search.walk = Some(Walk::start(reach, looking, self.show_hidden));
        let pane = self.pane_mut();
        pane.show(folder, Vec::new(), None);
        pane.listing = Listing::Search(search);
        self.refresh_ancestors();
        self.message = None;
    }

    /// Ends Find and shows what was there before it, the cursor where it was.
    fn leave_search(&mut self) {
        self.field = None;
        if let Some(search) = self.pane_mut().take_search() {
            self.revisit(&search.origin);
        }
    }

    /// `o`: what Find found under the cursor, shown in its folder, as a new
    /// place in the history.
    fn reveal(&mut self) {
        if self.pane().results().is_none() {
            return;
        }
        let Some(entry) = self.selected() else { return };
        let (path, name) = (entry.path.clone(), entry.name.clone());
        if let Some(folder) = path.parent() {
            self.go_to(folder, Some(&name));
        }
    }

    /// Enter in a dialog: does what it asked about.
    fn answer(&mut self, dialog: Dialog) {
        match dialog {
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
            Dialog::EmptyTrash {
                paths, armed: true, ..
            } => self.empty_trash(paths),
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
                // What the card says they take up, counted once now.
                let bytes = paths.iter().map(|path| bytes_of(path)).sum();
                self.dialog = Some(Dialog::EmptyTrash {
                    paths,
                    bytes,
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

    /// One archive of the selection beside it, in a format.
    fn compress(&mut self, format: Format) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        if let Err(error) = ops::archive_name(&paths, format) {
            return self.fail(error.to_string());
        }
        let what = match paths.as_slice() {
            [one] => name_of(one),
            many => ops::count(many.len()),
        };
        let write = self.system.archiver(&paths, format);
        let label = format!("Compressing {what} to .{}", format.extension());
        self.start(label, move |cancel| {
            ops::compress(&paths, format, cancel, |archive| write(archive, cancel))
        });
    }

    /// Whether Extract can unpack an item: an archive by its name, of a kind
    /// something installed reads.
    fn unpacks(&self, path: &Path) -> bool {
        let name = name_of(path);
        Format::of_archive(&name)
            .is_some_and(|(format, _)| self.system.extractor(path, format).is_some())
    }

    /// Extract, in the menu: the archive under the cursor into a new folder
    /// beside it.
    fn extract(&mut self) {
        if self.busy() {
            return;
        }
        let Some(entry) = self.selected() else { return };
        let (archive, name) = (entry.path.clone(), entry.name.clone());
        let Some((format, _)) = Format::of_archive(&name) else {
            return;
        };
        let Some((list, unpack)) = self.system.extractor(&archive, format) else {
            return;
        };
        self.start(format!("Extracting {name}"), move |cancel| {
            ops::extract(
                &archive,
                cancel,
                || list(cancel),
                |into| unpack(into, cancel),
            )
        });
    }

    /// Ctrl+T: a terminal in the folder under the cursor, in the folder of
    /// the file under it, or else in the folder that is showing.
    fn open_terminal(&mut self) {
        let folder = match self.selected() {
            Some(entry) if entry.is_dir => Some(entry.path.as_path()),
            Some(entry) => entry.path.parent(),
            None => Some(self.pane().cwd.as_path()),
        };
        let folder = folder.map(Path::to_path_buf);
        self.terminal_in(folder);
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

    /// `y` and `x`: remembers the selection for `p`.
    fn take(&mut self, cut: bool) {
        let paths = self.selection();
        if paths.is_empty() {
            return;
        }
        let what = if cut { "move" } else { "paste" };
        self.note(&format!("{} ready to {what}.", ops::count(paths.len())));
        self.clipboard = Some(Clipboard { paths, cut });
    }

    /// `p` copies what was taken into the pane's folder, or moves it there
    /// when it was cut; `M` moves it there whichever it was.
    fn paste(&mut self, moving: bool) {
        if self.busy() {
            return;
        }
        if !self.pane().is_folder() {
            self.note("Files can't be pasted here.");
            return;
        }
        let folder = self.pane().cwd.clone();
        self.paste_into(folder, moving);
    }

    /// Copies what was taken into a folder, or moves it there. When names
    /// there are taken it asks first, with the cursor on the first of them,
    /// and nothing is written until it is answered.
    fn paste_into(&mut self, folder: PathBuf, moving: bool) {
        let Some(clipboard) = &self.clipboard else {
            self.note("Nothing to paste");
            return;
        };
        let (paths, moving) = (clipboard.paths.clone(), clipboard.cut || moving);
        self.send(paths, folder, moving, false);
    }

    /// F5 and F6 in two panes: the active pane's selection copied, or
    /// moved, into the other pane's folder, as a paste there would; the
    /// clipboard stays as it was.
    fn send_to_other_pane(&mut self, moving: bool) {
        if self.busy() {
            return;
        }
        let paths = self.selection();
        let folder = self.other_pane().filter(|pane| pane.is_folder());
        let Some(folder) = folder.map(|pane| pane.cwd.clone()) else {
            return;
        };
        if !paths.is_empty() {
            self.send(paths, folder, moving, true);
        }
    }

    /// Copies items into a folder, or moves them there: a paste, or F5 and
    /// F6 into the other pane (`across`). When names there are taken it
    /// asks first, with the cursor on the first of them in the pane that
    /// shows the folder, and nothing is written until it is answered.
    fn send(&mut self, paths: Vec<PathBuf>, folder: PathBuf, moving: bool, across: bool) {
        let clashes = ops::clashes(&paths, &folder, moving);
        if clashes.is_empty() {
            return self.transfer(paths, folder, moving, Collide::KeepBoth, across);
        }
        // A paste goes into the active pane's folder, F5 and F6 into the
        // other one's.
        let active = self.active();
        let index = match across {
            true => (0..self.panes().len()).find(|index| *index != active),
            false => Some(active),
        };
        let shows = |pane: &&mut Pane| pane.is_folder() && pane.cwd == folder;
        let pane = index.and_then(|index| self.tab_mut().panes.get_mut(index));
        if let Some(pane) = pane.filter(shows) {
            let row = pane
                .entries
                .iter()
                .position(|entry| entry.name == clashes[0]);
            if let Some(row) = row {
                pane.point_at(row);
            }
        }
        self.question = Some(PasteQuestion {
            paths,
            folder,
            moving,
            clashes,
            across,
        });
    }

    /// A key while the paste question is asked: Enter or `b` keeps both,
    /// `r` replaces, `s` skips, Escape cancels the paste with nothing done.
    /// Any other key leaves the question standing.
    fn question_key(&mut self, question: PasteQuestion, key: KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let collide = match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter | KeyCode::Char('b') if plain => Collide::KeepBoth,
            KeyCode::Char('r') if plain => Collide::Replace,
            KeyCode::Char('s') if plain => Collide::Skip,
            _ => {
                self.question = Some(question);
                return;
            }
        };
        let PasteQuestion {
            paths,
            folder,
            moving,
            across,
            ..
        } = question;
        self.transfer(paths, folder, moving, collide, across);
    }

    /// Copies or moves items into a folder on the worker, treating the
    /// names already taken there as `collide` says. A paste that moves
    /// empties the clipboard; F5 and F6 (`across`) leave it, and what
    /// arrives is selected in the other pane.
    fn transfer(
        &mut self,
        paths: Vec<PathBuf>,
        folder: PathBuf,
        moving: bool,
        collide: Collide,
        across: bool,
    ) {
        let what = ops::count(paths.len());
        let trash = self.trash_dir.clone();
        let into = folder.clone();
        // F5 and F6 send to the pane without the keyboard now; it may have
        // it, or not show, by the time the work is done.
        let active = self.active();
        let landing = (0..self.panes().len())
            .find(|index| across && *index != active)
            .map(|pane| Landing {
                tab: self.tab().id,
                pane,
                folder,
            });
        if moving {
            if !across {
                // What moves is no longer where it was taken from.
                self.clipboard = None;
            }
            self.start(format!("Moving {what}…"), move |cancel| {
                ops::transfer(&paths, &into, true, collide, &trash, cancel)
            });
        } else {
            self.start(format!("Copying {what}…"), move |cancel| {
                ops::transfer(&paths, &into, false, collide, &trash, cancel)
            });
        }
        if let Some(work) = &mut self.work {
            work.landing = landing;
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

    /// Delete: moves the selection to the trash, except in the trash
    /// itself, where all that is left to do with it is to delete it for good.
    fn delete_key(&mut self) {
        if self.in_trash() {
            self.ask_delete();
        } else {
            self.trash();
        }
    }

    /// Whether the pane shows the trash, or a folder that is in it.
    fn in_trash(&self) -> bool {
        self.pane().cwd.starts_with(&self.trash_dir)
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

    /// `r` and F2: asks on the bottom row for the selected item's new name,
    /// the name without its extension selected.
    fn ask_rename(&mut self) {
        if self.busy() {
            return;
        }
        // Several are renamed together, in the editor.
        let selection = self.selection();
        if selection.len() > 1 {
            return self.bulk_rename(selection);
        }
        let [path] = selection.as_slice() else { return };
        let entries = &self.pane().entries;
        let Some(entry) = entries.iter().find(|entry| entry.path == *path) else {
            return;
        };
        let (stem, _) = ops::split_name(&entry.name, entry.is_dir);
        let mut input = Input::new(&entry.name);
        input.select(0, stem.chars().count());
        self.prompt = Some(Prompt {
            asking: Asking::Rename(entry.path.clone()),
            input,
            error: None,
            ghost: String::new(),
        });
    }

    /// `r` with several selected: their names in `$EDITOR`, a line each, and
    /// what comes back renamed as one change, or nothing renamed at all.
    fn bulk_rename(&mut self, items: Vec<PathBuf>) {
        let mut names = Vec::with_capacity(items.len());
        for item in &items {
            match item.file_name().and_then(|name| name.to_str()) {
                Some(name) => names.push(name.to_string()),
                None => {
                    let why = "Bulk rename cannot handle a name that is not valid text";
                    return self.fail(why.to_string());
                }
            }
        }
        let system = &self.system;
        let edited = ops::edit_names(&names, |file| system.edit(file));
        // The terminal was the editor's meanwhile.
        self.redraw = true;
        let lines = match edited {
            Ok(Some(lines)) => lines,
            Ok(None) => {
                let why = "The editor quit with an error; nothing was renamed.";
                return self.fail(why.to_string());
            }
            Err(error) => return self.fail(error.to_string()),
        };
        let moves = match ops::plan_renames(&items, &lines) {
            Ok(moves) => moves,
            Err(error) => return self.fail(error.to_string()),
        };
        if moves.is_empty() {
            return self.note(&format!("0 renamed · {} unchanged", items.len()));
        }
        match ops::bulk_rename(&moves) {
            Ok(change) => {
                let said = format!("Renamed {} · Undo reverts one", ops::count(moves.len()));
                self.changed(change, said);
            }
            Err(error) => {
                self.relist(&[], None);
                self.fail(error.to_string());
            }
        }
    }

    /// `N` and Ctrl+Shift+N: asks on the bottom row for a new folder's
    /// name, the suggested one selected so that typing replaces it.
    fn ask_new_folder(&mut self) {
        if self.busy() {
            return;
        }
        if !self.pane().is_folder() {
            self.note("Folders can't be made here.");
            return;
        }
        let mut input = Input::new("New Folder");
        input.select_all();
        self.prompt = Some(Prompt {
            asking: Asking::NewFolder(self.pane().cwd.clone()),
            input,
            error: None,
            ghost: String::new(),
        });
    }

    /// F9 and F10 in two panes: an empty `new.<extension>` in the active
    /// pane's folder, or the next free `new N.<extension>`, selected with
    /// the rename prompt open on it. Undo moves it to the trash.
    fn new_file(&mut self, extension: &str) {
        if self.busy() {
            return;
        }
        if !self.pane().is_folder() {
            self.note("Files can't be made here.");
            return;
        }
        match ops::new_file(&self.pane().cwd, "new", extension) {
            Ok(change) => {
                let made = change.landed();
                self.changed(change, "File created".to_string());
                // Only on the new file: a filter may leave it out.
                let on_it = self
                    .selected()
                    .is_some_and(|entry| made.contains(&entry.path));
                if on_it {
                    self.ask_rename();
                }
            }
            Err(error) => self.fail(error.to_string()),
        }
    }

    /// `Y`: the path of the folder that is showing, as text for other apps.
    fn copy_folder_path(&mut self) {
        if !self.pane().is_folder() {
            return;
        }
        let path = self.pane().cwd.display().to_string();
        match self.system.copy_text(&path) {
            Ok(()) => self.note("Copied the path."),
            Err(_) => self.fail("The path couldn't be copied.".to_string()),
        }
    }

    /// `E`: the file under the cursor in `$EDITOR`, in this terminal. The
    /// window is drawn whole again afterwards and the folder listed again.
    fn edit_file(&mut self) {
        let Some(entry) = self.selected() else { return };
        if entry.is_dir || !entry.path.is_file() {
            self.fail("Only a regular file can be edited".to_string());
            return;
        }
        let path = entry.path.clone();
        let edited = self.system.edit(&path);
        self.redraw = true;
        self.relist(&[], None);
        if edited.is_err() {
            self.fail("Could not start the editor".to_string());
        }
    }

    /// Whether the terminal was handed to another program since the last
    /// time this was asked, so that the next frame has to be drawn whole.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    /// `I`: the next detail in the list's right-hand column, remembered.
    fn next_detail(&mut self) {
        self.detail = self.detail.next();
        self.note(&format!("Detail column · {}", self.detail.label()));
        self.remember();
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
                self.receipt(format!("Undid {}", undone(&change)), false);
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
                    landing: None,
                });
            }
            Err(_) => self.fail("The file work couldn't be started.".to_string()),
        }
    }

    /// Takes in the file work that finished since the last turn.
    fn collect_work(&mut self) {
        let Some(work) = &self.work else { return };
        let had_menu = self.menu.is_some();
        match work.results.try_recv() {
            Ok(report) => {
                let landing = self.work.take().and_then(|mut work| work.landing.take());
                // The items listed again: a first `d` armed before is about
                // what may no longer be under the cursor.
                self.trash_armed = None;
                let landed = report.change.landed();
                match landing {
                    // Selected where it was sent, and nowhere else.
                    Some(landing) => {
                        self.settle_wanting(report, &[], None);
                        self.select_landed(landing, landed);
                    }
                    None => self.settle(report, None),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.work = None;
                self.trash_armed = None;
                self.relist(&[], None);
                self.fail("The file work stopped before it was done.".to_string());
            }
        }
        // Said last, so that what the work did doesn't hide it.
        if had_menu && self.menu.is_none() {
            self.fail("What the menu was for changed, so it closed.".to_string());
        }
    }

    /// Takes in what a command over several items did: logs it for undo,
    /// lists the folder again and says how it went.
    fn settle(&mut self, report: Report, neighbour: Option<PathBuf>) {
        let landed = report.change.landed();
        self.settle_wanting(report, &landed, neighbour);
    }

    /// As `settle`, the active pane's cursor put on the first of `wanted`
    /// it lists, else where it was, else on `neighbour`.
    fn settle_wanting(&mut self, report: Report, wanted: &[PathBuf], neighbour: Option<PathBuf>) {
        let (text, is_error) = outcome(&report);
        let undoable = !is_error && !report.change.steps.is_empty();
        self.keep_trash_records(Some(&report.change));
        self.history.record(report.change);
        self.relist(wanted, neighbour);
        if is_error {
            self.fail(text);
        } else {
            self.receipt(text, undoable);
        }
    }

    /// Takes in a change made to one item.
    fn changed(&mut self, change: Change, said: String) {
        let landed = change.landed();
        self.history.record(change);
        self.relist(&landed, None);
        self.receipt(said, true);
    }

    /// Lists what the pane shows again and puts the cursor on the first of
    /// these that is in it: one of `paths`, what was selected before, or
    /// `neighbour`. The other pane's folder is listed again too, since what
    /// changed may show there, and that pane stays where it was.
    fn relist(&mut self, paths: &[PathBuf], neighbour: Option<PathBuf>) {
        // The space left may have changed with the files.
        self.volumes.clear();
        let selected = self.selected().map(|entry| entry.path.clone());
        // A menu acts on the selection when one of its lines is chosen, so
        // while it is open the selection must stay what it was opened for.
        let menu_for = self.menu.is_some().then(|| self.pane().position());
        if self.reload() {
            match menu_for {
                Some(was) => self.keep_menu_selection(was),
                None => {
                    let wanted = paths.iter().cloned().chain(selected).chain(neighbour);
                    self.pane_mut().want(wanted.collect());
                }
            }
        }
        let active = self.active();
        for index in 0..self.panes().len() {
            if index != active {
                self.relist_pane(index);
            }
        }
    }

    /// After listing again under an open menu: the cursor and the marks back
    /// on the items the menu was opened for, or, when one of them is gone,
    /// the menu closed, so that no line can act on something else.
    fn keep_menu_selection(&mut self, was: Position) {
        let listed = |path: &PathBuf| self.pane().entries.iter().any(|entry| entry.path == *path);
        let all_there = was.under_cursor.iter().all(listed) && was.selected.iter().all(listed);
        if all_there {
            self.pane_mut().put_back(was);
        } else {
            self.menu = None;
        }
    }

    /// After F5 or F6: the cursor of the pane it was sent to on the first
    /// of what landed, when its tab is still open. A tab that isn't
    /// showing takes it in when it shows again, having listed it.
    fn select_landed(&mut self, landing: Landing, landed: Vec<PathBuf>) {
        let Some(tab) = self.tabs.iter().position(|tab| tab.id == landing.tab) else {
            return;
        };
        if tab == self.current {
            self.land(landing, landed);
        } else {
            self.tabs[tab].landed = Some((landing, landed));
        }
    }

    /// In the tab showing: the cursor of the pane F5 or F6 sent to on the
    /// first of what landed that it lists, when that pane is still there
    /// and still shows the folder. Under an open menu the active pane
    /// stays on what the menu was opened for.
    fn land(&mut self, landing: Landing, landed: Vec<PathBuf>) {
        if landing.pane == self.active() && self.menu.is_some() {
            return;
        }
        let shows = |pane: &&mut Pane| pane.is_folder() && pane.cwd == landing.folder;
        let pane = self.tab_mut().panes.get_mut(landing.pane);
        if let Some(pane) = pane.filter(shows) {
            pane.want(landed);
        }
    }

    /// Lists another pane's folder again behind its back and puts it back
    /// where it was. The files used lately and what a search found are not
    /// asked for again on another pane's account.
    fn relist_pane(&mut self, index: usize) {
        if self.panes()[index].is_folder() {
            let was = self.panes()[index].position();
            if self.in_pane(index, Self::reload) {
                self.tab_mut().panes[index].put_back(was);
            }
        }
    }

    /// Does to a pane what is written for the active one.
    fn in_pane<T>(&mut self, index: usize, act: impl FnOnce(&mut Self) -> T) -> T {
        let active = std::mem::replace(&mut self.tab_mut().active, index);
        let done = act(self);
        self.tab_mut().active = active;
        done
    }

    /// Lists what every pane shows again: the sort order and the hidden
    /// files are the same in all of them. Other tabs catch up when they
    /// show again.
    fn reload_all(&mut self) {
        for index in 0..self.panes().len() {
            self.in_pane(index, Self::reload);
        }
    }

    /// `t`: a new tab after the others, in the folder showing, in the view
    /// showing.
    fn new_tab(&mut self) {
        let folder = self.pane().cwd.clone();
        self.open_tab(folder);
    }

    /// `O`: the folder under the cursor in a new tab.
    fn open_in_new_tab(&mut self) {
        if self.tabs_full() {
            return;
        }
        match self.selected().filter(|entry| entry.is_dir) {
            Some(entry) => {
                let folder = entry.path.clone();
                self.open_tab(folder);
            }
            None => self.note("Only a folder opens in a new tab"),
        }
    }

    /// Says so when there is no room for another tab.
    fn tabs_full(&mut self) -> bool {
        let full = self.tabs.len() >= MOST_TABS;
        if full {
            self.note("Nine tabs is the most");
        }
        full
    }

    /// Opens a folder in a new tab at the end of the strip and shows it.
    fn open_tab(&mut self, folder: PathBuf) {
        if self.tabs_full() {
            return;
        }
        let was = self.current;
        // A new tab starts in the list, whatever view this one is in.
        self.tabs.push(Tab::new(folder.clone(), ViewMode::List));
        self.current = self.tabs.len() - 1;
        if self.show_folder(&folder, None) {
            self.focus = Focus::List;
            self.last_click = None;
        } else {
            // Why was said; a tab with nothing to show is no use.
            self.tabs.pop();
            self.current = was;
        }
    }

    /// Shows another tab, its folders read again, since what changed while
    /// it wasn't showing may be in them.
    pub fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.current {
            return;
        }
        self.current = index;
        self.arrive();
    }

    /// A tab is showing again: the keyboard in its list and its folders
    /// read again, each where it was.
    fn arrive(&mut self) {
        self.focus = Focus::List;
        self.last_click = None;
        for index in 0..self.panes().len() {
            self.relist_pane(index);
        }
        if let Some((landing, landed)) = self.tab_mut().landed.take() {
            self.land(landing, landed);
        }
    }

    /// `w` and Ctrl+W: closes the tab showing, and the tab that takes its
    /// place shows. Closing the last tab quits.
    fn close_tab(&mut self) {
        if self.tabs.len() == 1 {
            return self.quit();
        }
        self.tabs.remove(self.current);
        self.current = self.current.min(self.tabs.len() - 1);
        self.arrive();
    }

    /// `]` and `[`: the next or the previous tab, round from the last to
    /// the first and back.
    fn step_tab(&mut self, delta: isize) {
        let count = self.tabs.len() as isize;
        let index = (self.current as isize + delta).rem_euclid(count);
        self.switch_tab(index as usize);
    }

    /// `}` and `{`: moves the tab showing one place right or left, as far
    /// as the ends. It stays the one showing.
    fn move_tab(&mut self, delta: isize) {
        let last = self.tabs.len() as isize - 1;
        let to = (self.current as isize + delta).clamp(0, last) as usize;
        if to != self.current {
            let tab = self.tabs.remove(self.current);
            self.tabs.insert(to, tab);
            self.current = to;
        }
    }

    /// `1` to `9`: the tab with that number.
    fn go_to_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.switch_tab(index);
        } else {
            self.note(&format!("No tab {}", index + 1));
        }
    }

    /// A second pane on the right, in the folder of the first, with the
    /// keyboard: the two-pane view. Two are as many as there are.
    fn split(&mut self) {
        if self.panes().len() > 1 {
            return;
        }
        let folder = self.pane().cwd.clone();
        let tab = self.tab_mut();
        tab.before = tab.view;
        tab.view = ViewMode::Dual;
        tab.panes.push(Pane::new(folder.clone(), Vec::new()));
        let shown = self.in_pane(1, |app| app.show_folder(&folder, None));
        if shown {
            self.tab_mut().active = 1;
            self.focus = Focus::List;
            // The folders above go with one pane.
            self.in_pane(0, Self::refresh_ancestors);
        } else {
            // Why was said; a pane with nothing to show is no use.
            let tab = self.tab_mut();
            tab.panes.pop();
            tab.view = tab.before;
        }
    }

    /// Ctrl+4, `|` and Alt+1: two panes, or back to one pane in the view
    /// they were opened from.
    fn toggle_split(&mut self) {
        if self.view() == ViewMode::Dual {
            let before = self.tab().before;
            self.set_view(before);
        } else {
            self.split();
        }
    }

    /// Closes the right pane. The left one has the keyboard from then on.
    fn close_split(&mut self) {
        let tab = self.tab_mut();
        if tab.panes.len() > 1 {
            tab.panes.truncate(1);
            tab.active = 0;
            tab.view = tab.before;
            self.focus = Focus::List;
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

    /// Escape, "Close or go back" in this order: the keyboard back from
    /// the preview to the list; an error dismissed; the filter cleared;
    /// Find's walk stopped, then Find left; the file work under way
    /// stopped; and with none of that, the marks cleared.
    fn escape(&mut self) {
        if self.focus == Focus::Preview {
            self.focus = Focus::List;
        } else if self
            .message
            .as_ref()
            .is_some_and(|message| message.is_error)
        {
            self.message = None;
        } else if self.pane().filter().is_some() {
            self.pane_mut().clear_filter();
        } else if self.pane().search().is_some() {
            // A walk under way stops first; then Find is left.
            if !self.stop_search() {
                self.leave_search();
            }
        } else if self.work.is_some() {
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

    /// `q`, and `w` on the last tab: closes the window, once no file is left
    /// half copied.
    fn quit(&mut self) {
        if !self.busy() {
            self.should_quit = true;
        }
    }

    /// `j` and `k`, Down and Up: one item further, or in the grid one row.
    fn step(&mut self, delta: isize) {
        let grid = self.view() == ViewMode::Grid;
        let pane = self.pane_mut();
        if grid {
            let columns = pane.grid_columns.max(1);
            pane.move_row(delta > 0, columns);
        } else {
            pane.move_cursor(delta);
        }
    }

    /// Moves the cursor this many items, as far as there are.
    fn move_by(&mut self, delta: isize) {
        self.pane_mut().move_cursor(delta);
    }

    /// Ctrl+D and Ctrl+U, Page Down and Page Up: a screenful further.
    fn page(&mut self, pages: isize) {
        let grid = self.view() == ViewMode::Grid;
        let pane = self.pane();
        let columns = if grid { pane.grid_columns.max(1) } else { 1 };
        let page = (pane.page_rows.max(1) * columns) as isize;
        self.move_by(pages * page);
    }

    /// `J` and `K`, and Shift with an arrow in the grid: one item further,
    /// marking the way.
    fn extend(&mut self, delta: isize) {
        self.pane_mut().extend(|pane| pane.move_cursor(delta));
    }

    /// Shift+Down and Shift+Up: one further, or in the grid one row,
    /// marking the way.
    fn extend_row(&mut self, down: bool) {
        let grid = self.view() == ViewMode::Grid;
        let pane = self.pane_mut();
        let columns = pane.grid_columns.max(1);
        pane.extend(|pane| {
            if grid {
                pane.move_row(down, columns);
            } else {
                pane.move_cursor(if down { 1 } else { -1 });
            }
        });
    }

    /// `l` and Right: into the folder under the cursor; a file opens.
    fn browse_in(&mut self) {
        self.open_selected();
    }

    /// Keys while the preview pane has the keyboard: up and down scroll it
    /// and Escape gives the keyboard back to the list. False for any other
    /// key, which acts on the files as it would from the list.
    fn preview_key(&mut self, key: KeyEvent) -> bool {
        let page = self.preview_rows.max(1) as isize;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if plain => self.scroll_preview(-1),
            KeyCode::Down | KeyCode::Char('j') if plain => self.scroll_preview(1),
            KeyCode::PageUp if plain => self.scroll_preview(-page),
            KeyCode::PageDown if plain => self.scroll_preview(page),
            KeyCode::Char('u') if ctrl => self.scroll_preview(-page),
            KeyCode::Char('d') if ctrl => self.scroll_preview(page),
            KeyCode::Esc => self.focus = Focus::List,
            _ => return false,
        }
        true
    }

    /// Scrolls the preview's body; drawing keeps it in range.
    fn scroll_preview(&mut self, rows: isize) {
        self.preview_scroll = self.preview_scroll.saturating_add_signed(rows);
    }

    /// Tab and Shift+Tab: the places panel when it shows, then each pane in
    /// turn, then the preview when it shows.
    fn move_focus(&mut self, delta: isize) {
        let mut stops: Vec<(Focus, Option<usize>)> = Vec::new();
        if self.places_visible {
            stops.push((Focus::Places, None));
        }
        stops.extend((0..self.panes().len()).map(|pane| (Focus::List, Some(pane))));
        if self.shows_preview() {
            stops.push((Focus::Preview, None));
        }
        let here = |(focus, pane): &(Focus, Option<usize>)| {
            *focus == self.focus && pane.is_none_or(|pane| pane == self.active())
        };
        let count = stops.len() as isize;
        // A stop that has gone, like a hidden panel, starts the way round
        // again from its first stop or its last.
        let index = match stops.iter().position(here) {
            Some(index) => index as isize + delta,
            None if delta > 0 => 0,
            None => count - 1,
        };
        let (focus, pane) = stops[index.rem_euclid(count) as usize];
        if focus == Focus::Places {
            return self.enter_places();
        }
        self.focus = focus;
        if let Some(pane) = pane {
            self.tab_mut().active = pane;
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

    /// Space: the Quick Look card for the item under the cursor.
    fn open_look(&mut self) {
        if let Some(entry) = self.selected() {
            let about = Some(entry.path.clone());
            self.look = Some(Look {
                about,
                ..Look::default()
            });
        }
    }

    /// A key while the Quick Look card is open: these act, and no other
    /// reaches what is underneath.
    fn look_key(&mut self, key: KeyEvent) {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let Some(look) = &mut self.look else { return };
        let page = look.rows.max(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') => self.look = None,
            // The list's cursor moves, and the card follows it.
            KeyCode::Char('j') | KeyCode::Down if plain => self.step(1),
            KeyCode::Char('k') | KeyCode::Up if plain => self.step(-1),
            KeyCode::Char('+') => look.zoom = (look.zoom + 1).min(MOST_ZOOM),
            KeyCode::Char('-') => look.zoom = (look.zoom - 1).max(-MOST_ZOOM),
            KeyCode::Char('0') => look.zoom = 0,
            KeyCode::PageDown => look.scroll = look.scroll.saturating_add(page),
            KeyCode::PageUp => look.scroll = look.scroll.saturating_sub(page),
            _ => {}
        }
    }

    /// Shows the selected item in macOS's own Quick Look window, or closes
    /// that window: Open With › Quick Look.
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
        if path != from.path {
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

    fn go_up(&mut self) {
        let from = self.pane().visit();
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

    /// Ctrl+1, Ctrl+2 and Ctrl+3: the tab's view, with one pane.
    fn set_view(&mut self, view: ViewMode) {
        self.close_split();
        self.tab_mut().view = view;
        for index in 0..self.panes().len() {
            self.in_pane(index, Self::refresh_ancestors);
        }
        self.remember();
    }

    fn set_tile(&mut self, tile: TileSize) {
        self.tile = tile;
        self.remember();
    }

    /// Saves the view, the tile size, the preview, the sort, the detail
    /// column and the places panel for the next launch.
    fn remember(&self) {
        if let Some(dir) = &self.state_dir {
            let saved = Saved {
                view: self.tab().single_view(),
                tile: self.tile,
                preview: self.preview_visible,
                sort: self.sort,
                detail: self.detail,
                places: self.places_visible,
                hidden: self.show_hidden,
                space: self.space_look,
                columns: self.columns,
            };
            // A view that isn't remembered is no reason to interrupt.
            let _ = state::save(dir, saved);
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.reload_all();
        self.remember();
    }

    /// `,` or the gear: the settings card, on View with the keyboard in
    /// its rows.
    fn open_settings(&mut self) {
        self.menu = None;
        self.settings = Some(Settings::new());
    }

    /// A key while the settings are open: Tab between the rail and the
    /// rows, `j`/`k` the next or previous of them, `h`/`l` a choice's value,
    /// Space a check, Enter a row (or into the rows from the rail), Esc away.
    fn settings_key(&mut self, key: KeyEvent) {
        let Some(mut open) = self.settings else {
            return;
        };
        let items = settings::items(&settings::rows(self, open.section));
        let last = items.len().saturating_sub(1);
        let section = SECTIONS
            .iter()
            .position(|s| *s == open.section)
            .unwrap_or(0);
        let here = items.get(open.cursor).copied();
        let mut change = None;
        match key.code {
            KeyCode::Esc => {
                self.settings = None;
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => open.in_rail = !open.in_rail,
            KeyCode::Down | KeyCode::Char('j') if open.in_rail => {
                open.section = SECTIONS[(section + 1).min(SECTIONS.len() - 1)];
                open.cursor = 0;
            }
            KeyCode::Up | KeyCode::Char('k') if open.in_rail => {
                open.section = SECTIONS[section.saturating_sub(1)];
                open.cursor = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => open.cursor = (open.cursor + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => open.cursor = open.cursor.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') if open.in_rail => open.in_rail = false,
            KeyCode::Right | KeyCode::Char('l') => change = here.map(|s| (s, 1, RowKind::Choice)),
            KeyCode::Left | KeyCode::Char('h') => change = here.map(|s| (s, -1, RowKind::Choice)),
            KeyCode::Char(' ') => change = here.map(|s| (s, 1, RowKind::Check)),
            KeyCode::Enter => change = here.map(|s| (s, 1, s.kind())),
            _ => {}
        }
        self.settings = Some(open);
        if let Some((setting, step, wanted)) = change
            && setting.kind() == wanted
        {
            self.change_setting(setting, step);
        }
    }

    /// What a row does: a choice stepped by `step`, a check turned over, an
    /// action run. Its own key's way, so it is remembered the same.
    fn change_setting(&mut self, setting: Setting, step: isize) {
        let at = |index: Option<usize>, count| settings::wrap(index.unwrap_or(0), step, count);
        match setting {
            Setting::View => {
                let index = settings::VIEWS.iter().position(|v| *v == self.view());
                self.pick_view(settings::VIEWS[at(index, 4)]);
            }
            Setting::Preview => self.toggle_preview(),
            Setting::Places => self.toggle_places(),
            Setting::Detail => {
                let index = settings::DETAILS.iter().position(|d| *d == self.detail);
                self.detail = settings::DETAILS[at(index, 4)];
                self.remember();
            }
            Setting::SortBy => {
                let index = settings::KEYS.iter().position(|k| *k == self.sort.key);
                let key = settings::KEYS[at(index, 4)];
                self.set_sort(SortMode {
                    key,
                    reversed: self.sort.reversed,
                });
            }
            Setting::Order => self.set_sort(self.sort.reverse()),
            Setting::Hidden => self.toggle_hidden(),
            Setting::Column(index) => {
                self.columns[index] = !self.columns[index];
                self.remember();
            }
            Setting::Space => {
                self.space_look = match self.space_look {
                    SpaceLook::Terminal => SpaceLook::QuickLook,
                    SpaceLook::QuickLook => SpaceLook::Terminal,
                };
                self.remember();
            }
            Setting::Keys => {
                self.settings = None;
                self.open_help();
                return;
            }
            Setting::Issue => {
                if self.system.open(Path::new(settings::ISSUES)).is_err() {
                    self.fail("Couldn't open the issues page.".to_string());
                }
                return;
            }
            Setting::Repo => {
                if self.system.open(Path::new(settings::REPO)).is_err() {
                    self.fail("Couldn't open the repository's page.".to_string());
                }
                return;
            }
            Setting::Version | Setting::License => return,
        }
        if let Some(open) = &mut self.settings {
            open.saved = true;
        }
    }

    /// A view as "Last-used view" picks it: two panes opened or closed the
    /// way `|` does, so the one-pane view to go back to stays right.
    fn pick_view(&mut self, view: ViewMode) {
        let dual = self.view() == ViewMode::Dual;
        if (view == ViewMode::Dual) != dual {
            self.toggle_split();
        }
        if view != ViewMode::Dual {
            self.set_view(view);
        }
    }

    /// Alt+P: hides or shows the preview pane, and remembers it.
    fn toggle_preview(&mut self) {
        self.preview_visible = !self.preview_visible;
        if !self.preview_visible && self.focus == Focus::Preview {
            self.focus = Focus::List;
        }
        self.remember();
    }

    /// `s` and `S`: sorts every pane this way, and remembers it.
    fn set_sort(&mut self, sort: SortMode) {
        self.sort = sort;
        self.reload_all();
        self.remember();
    }

    /// A click on a title in a list's header: sorts by it, A to Z (or
    /// smallest, oldest first), or the other way round when already sorted
    /// by it.
    fn sort_by(&mut self, key: SortKey) {
        let sort = match self.sort.key == key {
            true => self.sort.reverse(),
            false => SortMode {
                key,
                reversed: false,
            },
        };
        self.set_sort(sort);
    }

    /// Ctrl+R: reads what the pane shows, its volume and the places
    /// panel again.
    fn refresh(&mut self) {
        self.volumes.clear();
        self.internal_volumes.clear();
        self.read_tags_in_use();
        self.read_places();
        self.reload();
    }

    /// Lists what the pane shows again, keeping the selection on the same
    /// item. False when it couldn't be listed.
    fn reload(&mut self) -> bool {
        if self.pane().search().is_some() {
            self.run_search();
            return true;
        }
        if self.pane().filter().is_some() {
            return self.reload_filtered();
        }
        let here = self.pane().visit();
        self.revisit(&here)
    }

    /// Lists a filtered folder again, and what is below it when the filter
    /// looks there, and narrows it as before.
    fn reload_filtered(&mut self) -> bool {
        let here = self.pane().visit();
        let under = self.selected().map(|entry| entry.path.clone());
        let Some(mut filter) = self.pane_mut().take_filter() else {
            return false;
        };
        if !self.revisit(&here) {
            return false;
        }
        let pane = self.pane_mut();
        filter.all = pane.entries.clone();
        filter.own = filter.all.len();
        if filter.scope == FilterScope::Subfolders {
            let reach = Reach::volume(pane.cwd.clone());
            filter.walk = Some(Walk::start(reach, Looking::Below, self.show_hidden));
        }
        let pane = self.pane_mut();
        pane.listing = Listing::Filter(filter);
        pane.narrow();
        pane.want(under.into_iter().collect());
        true
    }

    /// Shows the folder a visit was of.
    fn revisit(&mut self, visit: &Visit) -> bool {
        self.show_folder(&visit.path, visit.selected.as_deref())
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

    /// Lists the folders above the active pane's again, when the columns
    /// view shows them. What a search found is a column on its own.
    fn refresh_ancestors(&mut self) {
        let ancestors = if self.view() == ViewMode::Columns && self.pane().results().is_none() {
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

    fn say(&mut self, text: String, is_error: bool, undoable: bool, lasts: Duration) {
        self.message = Some(Message {
            text,
            is_error,
            undoable,
            at: (self.clock)(),
            lasts,
        });
    }

    /// A notice, for four seconds.
    fn note(&mut self, text: &str) {
        self.say(text.to_string(), false, false, NOTICE_LASTS);
    }

    /// What a change did, or what undo took back, for eight seconds; after
    /// a change undo can take back, the footer says so.
    fn receipt(&mut self, text: String, undoable: bool) {
        self.say(text, false, undoable, RECEIPT_LASTS);
    }

    /// What went wrong, for eight seconds.
    fn fail(&mut self, text: String) {
        self.say(text, true, false, RECEIPT_LASTS);
    }

    /// Whether a key is the window's own rather than the files': it does
    /// what it does even while the places panel has the keyboard.
    fn window_key(&self, key: KeyEvent) -> bool {
        let Some(chord) = Chord::of(key) else {
            return false;
        };
        let window = |chord: Chord| self.keymap.commands(chord).any(Command::window);
        window(chord) || chord.unshifted().is_some_and(window)
    }
}

/// Find's text cut into the `tag:` terms it asks for and the rest; with no
/// such term, the rest is the text just as it was typed.
/// What Find asks for to list what carries one tag.
fn tag_query(name: &str) -> String {
    format!("tag:{}", name.to_lowercase())
}

/// What finds paths, keeping back hidden ones below `root` (anywhere,
/// without one) unless hidden items show.
fn shown_only(grep: Grep, root: Option<PathBuf>, show_hidden: bool) -> Grep {
    if show_hidden {
        return grep;
    }
    Box::new(move |found, stop| {
        let mut shown = |path: PathBuf| {
            let below = root
                .as_deref()
                .and_then(|root| path.strip_prefix(root).ok());
            let hidden = below
                .unwrap_or(&path)
                .components()
                .any(|part| part.as_os_str().to_string_lossy().starts_with('.'));
            hidden || found(path)
        };
        grep(&mut shown, stop)
    })
}

fn find_terms(text: &str) -> (Vec<String>, String) {
    match crate::tags::split_query(text) {
        (tags, _) if tags.is_empty() => (tags, text.to_string()),
        split => split,
    }
}

/// Why a name typed in a prompt can't be one.
fn name_refusal(name: &str) -> Option<String> {
    let why = if name.trim().is_empty() {
        "A name cannot be empty."
    } else if matches!(name.trim(), "." | "..") {
        "A name cannot be a single dot or a double dot."
    } else if name.contains('/') {
        "A name cannot contain /."
    } else if name.contains('\0') {
        "A name cannot contain a null character."
    } else {
        return None;
    };
    Some(why.to_string())
}

/// How a command over several items went, for the footer, and whether
/// that is an error.
fn outcome(report: &Report) -> (String, bool) {
    if let Some(error) = &report.error {
        return (error.to_string(), true);
    }
    let done = ops::count(report.done);
    let text = if report.stopped && report.change.kind == Kind::Compress {
        // One archive is made whole or not at all: there is no part to count.
        "Stopped compressing.".to_string()
    } else if report.stopped && report.change.kind == Kind::Extract {
        "Extraction cancelled.".to_string()
    } else if report.stopped {
        let asked = ops::count(report.asked);
        format!("Stopped after {} of {asked}.", report.done)
    } else if report.done == 0 && report.skipped > 0 && report.declined == 0 {
        "It is already in this folder.".to_string()
    } else {
        match report.change.kind {
            Kind::Copy => format!("Copied {done}"),
            Kind::Duplicate => format!("Duplicated {done}."),
            Kind::Trash => format!("Moved {done} to Trash"),
            Kind::Compress => format!("Compressed into {}.", report.change.name()),
            Kind::Extract => "Extracted.".to_string(),
            Kind::Restore => format!("Restored {done}."),
            Kind::Move
            | Kind::Rename
            | Kind::NewFolder
            | Kind::NewFile
            | Kind::Tag
            | Kind::BulkRename => {
                format!("Moved {done}.")
            }
        }
    };
    // What a paste left out because its name was taken.
    let text = match report.declined {
        0 => text,
        declined => format!("{text} · {declined} skipped"),
    };
    (text, false)
}

/// What an undo took back, after "Undid".
fn undone(change: &Change) -> &'static str {
    match change.kind {
        Kind::Copy => "the copy",
        Kind::Move => "the move",
        Kind::Trash => "the move to Trash",
        Kind::Duplicate => "the duplicate",
        Kind::Rename => "the rename",
        Kind::NewFolder => "the new folder",
        Kind::NewFile => "the new file",
        Kind::Compress => "the compress",
        Kind::Restore => "the restore",
        Kind::Tag => "the tag change",
        Kind::BulkRename => "the renames",
        Kind::Extract => "the extract",
    }
}

/// What the footer says after a redo ("Redid").
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
        Kind::NewFolder | Kind::NewFile => format!("{verb} making {name}."),
        Kind::Compress => format!("{verb} compressing into {name}."),
        Kind::Restore => format!("{verb} restoring {items}."),
        Kind::Tag => format!("{verb} the tag change on {items}."),
        Kind::BulkRename => format!("{verb} the renames."),
        Kind::Extract => format!("{verb} extracting into {name}."),
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
/// How many bytes an item takes up: a folder's are those of everything
/// in it, links not followed.
fn bytes_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let Ok(items) = std::fs::read_dir(path) else {
        return 0;
    };
    items.flatten().map(|item| bytes_of(&item.path())).sum()
}

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
    use crate::system::{Archiver, Lister, Unpacker};
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
        /// The volume every folder is on; `None` when it can't be told.
        pub volume: Rc<RefCell<Option<Volume>>>,
        /// How many times a volume was asked about.
        pub volumes_asked: Rc<Cell<usize>>,
        /// Every folder a terminal was opened in.
        pub terminals: Rc<RefCell<Vec<PathBuf>>>,
        /// Whether no terminal can be started.
        pub no_terminal: Rc<Cell<bool>>,
        /// How the stand-ins for the programs that write and read archives behave.
        pub archiving: Rc<Cell<Archiving>>,
        /// Every format an archive was asked of.
        pub formats_asked: Rc<RefCell<Vec<Format>>>,
        /// Whether 7-Zip is installed.
        pub seven_zip: Rc<Cell<bool>>,
        /// Every file handed to the editor.
        pub edited: Rc<RefCell<Vec<PathBuf>>>,
        /// Whether the editor can't be started.
        pub no_editor: Rc<Cell<bool>>,
        /// What the stand-in for the editor does to the file it is given.
        pub editor: Rc<RefCell<Option<Editor>>>,
        /// What the stand-in for ripgrep finds, whatever it is asked;
        /// `None` when ripgrep isn't installed.
        pub grep_finds: Rc<RefCell<Option<Vec<PathBuf>>>>,
        /// Every search of contents asked for: the folders and the text.
        pub grepped: Rc<RefCell<Vec<Grepped>>>,
        /// When set, the next search of contents hands over what comes
        /// through here, as it comes, until the other end goes.
        pub grep_feed: Rc<RefCell<Option<mpsc::Receiver<PathBuf>>>>,
        /// What the stand-in for Spotlight finds, whatever tags it is asked
        /// for; `None` when there is no Spotlight.
        pub spotlight_finds: Rc<RefCell<Option<Vec<PathBuf>>>>,
        /// Every Spotlight search asked for: the tags and where.
        pub spotlit: Rc<RefCell<Vec<(Vec<String>, Option<PathBuf>)>>>,
        /// The tags the stand-in for Spotlight says are in use.
        pub in_use: Rc<Cell<Option<u8>>>,
        /// How many times the tags in use were asked for.
        pub in_use_asked: Rc<Cell<usize>>,
        /// Every volume ejected; each is removed, as if unmounted.
        pub ejected: Rc<RefCell<Vec<PathBuf>>>,
        /// Whether ejecting fails, as when something uses the volume.
        pub eject_fails: Rc<Cell<bool>>,
        /// The volumes on the Mac's own disk; the others are external.
        pub internal: Rc<RefCell<Vec<PathBuf>>>,
        /// How many times a volume was asked whether it is internal.
        pub internal_asked: Rc<Cell<usize>>,
    }

    /// The stand-in for the editor: what it does to the file.
    pub type Editor = Rc<dyn Fn(&Path) -> io::Result<()>>;

    /// A search of contents asked of the stand-in for ripgrep: where, and for what.
    pub type Grepped = (Vec<PathBuf>, String);

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

        fn open_terminal(&self, folder: &Path) -> io::Result<()> {
            if self.no_terminal.get() {
                return Err(io::Error::other("no terminal started"));
            }
            self.terminals.borrow_mut().push(folder.to_path_buf());
            Ok(())
        }

        fn archiver(&self, items: &[PathBuf], format: Format) -> Archiver {
            self.formats_asked.borrow_mut().push(format);
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

        fn formats(&self) -> Vec<Format> {
            let mut formats = Format::WRITTEN.to_vec();
            if self.seven_zip.get() {
                formats.push(Format::SevenZ);
            }
            formats
        }

        /// An archive holds names, a line each; unpacked, each is a file
        /// saying where it came from.
        fn extractor(&self, archive: &Path, format: Format) -> Option<(Lister, Unpacker)> {
            if format == Format::SevenZ && !self.seven_zip.get() {
                return None;
            }
            let (archive, archiving) = (archive.to_path_buf(), self.archiving.get());
            let listed = archive.clone();
            let lister: Lister = Box::new(move |_stop| {
                let text = fs::read_to_string(&listed)?;
                Ok(text.lines().map(str::to_string).collect())
            });
            let unpacker: Unpacker = Box::new(move |into, stop| {
                let text = fs::read_to_string(&archive)?;
                let said = format!("from {}", name_of(&archive));
                for name in text.lines() {
                    let file = into.join(name);
                    fs::create_dir_all(file.parent().unwrap_or(into))?;
                    fs::write(&file, &said)?;
                    if archiving != Archiving::Works {
                        break;
                    }
                }
                while archiving == Archiving::Hangs && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                match archiving {
                    Archiving::Works => Ok(()),
                    Archiving::Hangs => Err(io::ErrorKind::Interrupted.into()),
                    Archiving::Fails => Err(io::Error::other("it couldn't be read")),
                }
            });
            Some((lister, unpacker))
        }

        fn eject(&self, volume: &Path) -> io::Result<()> {
            if self.eject_fails.get() {
                return Err(io::Error::other("it is in use"));
            }
            self.ejected.borrow_mut().push(volume.to_path_buf());
            std::fs::remove_dir_all(volume)
        }

        fn internal(&self, volume: &Path) -> bool {
            self.internal_asked.set(self.internal_asked.get() + 1);
            self.internal.borrow().iter().any(|path| path == volume)
        }

        fn volume(&self, _folder: &Path) -> Option<Volume> {
            self.volumes_asked.set(self.volumes_asked.get() + 1);
            self.volume.borrow().clone()
        }

        fn edit(&self, path: &Path) -> io::Result<()> {
            if self.no_editor.get() {
                return Err(io::Error::other("no editor started"));
            }
            self.edited.borrow_mut().push(path.to_path_buf());
            let editor = self.editor.borrow().clone();
            editor.map_or(Ok(()), |editor| editor(path))
        }

        fn grep(&self, roots: &[PathBuf], text: &str, _show_hidden: bool) -> Option<Grep> {
            let finds = self.grep_finds.borrow().clone()?;
            self.grepped
                .borrow_mut()
                .push((roots.to_vec(), text.to_string()));
            if let Some(feed) = self.grep_feed.borrow_mut().take() {
                return Some(Box::new(move |found, stop| {
                    loop {
                        match feed.recv_timeout(Duration::from_millis(2)) {
                            Ok(path) => {
                                if !found(path) {
                                    return Ok(());
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                            Err(mpsc::RecvTimeoutError::Timeout)
                                if stop.load(Ordering::Relaxed) =>
                            {
                                return Ok(());
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                }));
            }
            Some(Box::new(move |found, _stop| {
                for path in finds {
                    if !found(path) {
                        break;
                    }
                }
                Ok(())
            }))
        }

        fn can_grep(&self) -> bool {
            self.grep_finds.borrow().is_some()
        }

        fn spotlight(&self, tags: &[String], root: Option<&Path>) -> Option<Grep> {
            let finds = self.spotlight_finds.borrow().clone()?;
            let asked = (tags.to_vec(), root.map(Path::to_path_buf));
            self.spotlit.borrow_mut().push(asked);
            Some(Box::new(move |found, _stop| {
                for path in finds {
                    if !found(path) {
                        break;
                    }
                }
                Ok(())
            }))
        }

        fn tags_in_use(&self) -> Option<u8> {
            self.in_use_asked.set(self.in_use_asked.get() + 1);
            self.in_use.get()
        }
    }

    impl FakeSystem {
        /// A system whose volumes are all APFS with 1.2 GB free.
        fn new() -> Self {
            let system = Self::default();
            *system.volume.borrow_mut() = Some(Volume {
                kind: "apfs".to_string(),
                free: 1_200_000_000,
            });
            system
        }
    }

    pub struct Fixture {
        pub app: App,
        pub system: FakeSystem,
        pub home: PathBuf,
        /// The trash the app was given: a folder of the fixture's own.
        pub trash: PathBuf,
        /// Where the app looks for mounted volumes: empty, outside home.
        pub volumes: PathBuf,
        _dir: tempfile::TempDir,
        _volumes: tempfile::TempDir,
    }

    /// A home folder with `Documents/`, `Pictures/{Trip/, a.jpg}`, `notes.txt` and `.hidden`.
    pub fn fixture() -> Fixture {
        fixture_with(None)
    }

    /// The fixture drawing real thumbnails: a fake thumbnailer shown through half blocks.
    pub fn pictures() -> (Fixture, Arc<FakeThumbnailer>) {
        pictures_through(ratatui_image::picker::ProtocolType::Halfblocks)
    }

    /// The fixture drawing the fake thumbnailer's pictures the way a
    /// terminal speaking `protocol` would.
    pub fn pictures_through(
        protocol: ratatui_image::picker::ProtocolType,
    ) -> (Fixture, Arc<FakeThumbnailer>) {
        let thumbnailer = Arc::new(FakeThumbnailer::default());
        let mut picker = crate::thumbs::testing::picker();
        picker.set_protocol_type(protocol);
        let thumbs = Thumbs::new(picker, thumbnailer.clone());
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

    /// Lets Find's walk, or the filter's below the folder, look everywhere
    /// and takes in what it found.
    pub fn finish_search(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.tick();
            let pane = app.pane();
            let searching = pane.search().is_some_and(Search::is_running);
            if !searching && pane.filter().is_none_or(|filter| !filter.is_running()) {
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
            let coloured = app.highlighter.as_ref().is_none_or(Highlighter::is_idle);
            if app.thumbs.as_ref().is_none_or(Thumbs::is_idle) && coloured {
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
        let system = FakeSystem::new();
        let trash = home.join(".Trash");
        // The volumes folder, outside home and empty: tests mount what they need.
        let volumes_dir = tempfile::tempdir().unwrap();
        let volumes = volumes_dir.path().canonicalize().unwrap();
        let config = Config {
            home: home.clone(),
            start: home.clone(),
            date_style: DateStyle::MonthFirst,
            state_dir: None,
            thumbs,
            highlighter: None,
            trash_dir: trash.clone(),
            volumes_dir: volumes.clone(),
            // No applications there: PLACES starts at Home.
            applications_dir: home.join(".no-applications"),
            depth: Depth::Truecolor,
        };
        let mut app = App::new(config, Box::new(system.clone()));
        // The app opens in two panes; the tests written against one list
        // keep it, and the others switch with Ctrl+2, Ctrl+3 or Ctrl+4.
        app.close_split();
        app.tab_mut().view = ViewMode::List;
        Fixture {
            app,
            system,
            home,
            trash,
            volumes,
            _dir: dir,
            _volumes: volumes_dir,
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
            highlighter: None,
            trash_dir: home.join(".Trash"),
            // No volumes there: DEVICES lists nothing.
            volumes_dir: home.join(".no-volumes"),
            // No applications there: PLACES starts at Home.
            applications_dir: home.join(".no-applications"),
            depth: Depth::Truecolor,
        };
        App::new(config, Box::new(FakeSystem::new()))
    }

    /// Starts the app the way `main` does, then closes the second pane it
    /// opens in, without saving anything: for tests about one pane.
    pub fn launch_one_pane(home: &Path, state_dir: Option<&Path>) -> App {
        let mut app = launch(home, state_dir);
        app.close_split();
        app
    }

    /// The fixture as a grid three tiles across and two rows tall, holding ten items:
    /// `Documents`, `Pictures`, `a.txt` to `g.txt` and `notes.txt`.
    pub fn grid_fixture() -> Fixture {
        let mut f = fixture();
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        // The preview and the places panel hidden: the grid takes the whole body.
        f.app.handle_key(alt(KeyCode::Char('p')));
        f.app.handle_key(ctrl(KeyCode::Char('b')));
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        // The body of a window 62 across and 26 down is 62 by 22.
        crate::ui::testing::screen(&mut f.app, 62, 26);
        f
    }

    /// The fixture with `a.txt` to `e.txt` next to what it had: eight items
    /// in a list, `Documents`, `Pictures`, the five and `notes.txt`.
    pub fn several() -> Fixture {
        let mut f = fixture();
        for name in ["a", "b", "c", "d", "e"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        f
    }

    /// `|`: a second pane, which takes the keyboard.
    pub fn split(app: &mut App) {
        app.handle_key(key(KeyCode::Char('|')));
    }

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    pub fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    /// Gives the preview the keyboard the way the user does, Tab passing it
    /// by: a click in it, on a frame drawn 120 by 30.
    pub fn click_preview(app: &mut App) {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        crate::ui::testing::buffer(app, 120, 30);
        let rect = app.hits.rect_of(&Spot::Preview).expect("the preview");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + rect.width / 2,
            row: rect.y + rect.height / 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.focus, Focus::Preview);
    }

    /// Control and Option (sent as Alt) held together.
    pub fn ctrl_alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL | KeyModifiers::ALT)
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

    /// A clock the test moves on by hand, for the app to tell the time by.
    pub fn manual_clock(app: &mut App) -> Rc<Cell<Instant>> {
        let now = Rc::new(Cell::new(Instant::now()));
        let read = now.clone();
        app.clock = Box::new(move || read.get());
        now
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
    use crate::model::SortKey;
    use crate::theme;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::rc::Rc;

    const BY_SIZE: SortMode = SortMode {
        key: SortKey::Size,
        reversed: true,
    };
    const BY_KIND: SortMode = SortMode {
        key: SortKey::Kind,
        reversed: false,
    };

    fn selected_name(app: &App) -> Option<&str> {
        app.selected().map(|entry| entry.name.as_str())
    }

    #[test]
    fn starts_in_the_given_folder_with_folders_first_and_hidden_files_out() {
        let f = fixture();
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(f.app.focus, Focus::List);
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
            highlighter: None,
            trash_dir: f.trash.clone(),
            volumes_dir: f.volumes.clone(),
            applications_dir: f.home.join(".no-applications"),
            depth: Depth::Truecolor,
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
    fn l_and_right_browse_into_a_folder_and_h_left_and_backspace_back_out() {
        let mut f = fixture();
        for (into, out) in [
            (KeyCode::Right, KeyCode::Left),
            (KeyCode::Char('l'), KeyCode::Char('h')),
            (KeyCode::Char('l'), KeyCode::Backspace),
        ] {
            press(&mut f.app, &[KeyCode::Home, KeyCode::Down, into]);
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"), "{into:?}");
            press(&mut f.app, &[out]);
            assert_eq!(f.app.pane().cwd, f.home, "{out:?}");
            assert_eq!(selected_name(&f.app), Some("Pictures"));
        }
        // On a file they open it, as browsing in does.
        press(
            &mut f.app,
            &[KeyCode::End, KeyCode::Right, KeyCode::Char('l')],
        );
        assert_eq!(f.app.pane().cwd, f.home);
        let notes = f.home.join("notes.txt");
        assert_eq!(*f.system.opened.borrow(), [notes.clone(), notes]);
    }

    #[test]
    fn capital_h_and_l_walk_the_history_back_and_forward() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(selected_name(&f.app), Some("Trip"));
        f.app.handle_key(shift(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home);
        f.app.handle_key(key(KeyCode::Char('L')));
        f.app.handle_key(shift(KeyCode::Char('L')));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        // Alt with the arrows, and Option's b and f, are not keys here.
        for chord in [
            alt(KeyCode::Left),
            alt(KeyCode::Char('b')),
            alt(KeyCode::Home),
        ] {
            f.app.handle_key(chord);
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        }
    }

    #[test]
    fn tab_moves_the_keyboard_between_the_panel_the_list_and_the_preview() {
        let mut f = fixture();
        // The preview shows: it is the last stop, after the list.
        for focus in [Focus::Preview, Focus::Places, Focus::List] {
            press(&mut f.app, &[KeyCode::Tab]);
            assert_eq!(f.app.focus, focus);
        }
        for focus in [Focus::Places, Focus::Preview, Focus::List] {
            press(&mut f.app, &[KeyCode::BackTab]);
            assert_eq!(f.app.focus, focus);
        }
        // With the keyboard, the preview scrolls; Escape gives it back to
        // the list.
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Preview);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.focus, Focus::List);
        // Without the preview Tab goes between the panel and the list.
        f.app.handle_key(alt(KeyCode::Char('p')));
        for focus in [Focus::Places, Focus::List, Focus::Places, Focus::List] {
            press(&mut f.app, &[KeyCode::Tab]);
            assert_eq!(f.app.focus, focus);
        }
        // The columns and the grid show the preview too, and stop at it.
        f.app.handle_key(alt(KeyCode::Char('p')));
        for view in ['2', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            for focus in [Focus::Preview, Focus::Places, Focus::List] {
                press(&mut f.app, &[KeyCode::Tab]);
                assert_eq!(f.app.focus, focus, "view {view}");
            }
        }
        // Two panes have no preview: Tab goes from the panel to one list
        // and the other.
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        for (focus, active) in [
            (Focus::Places, 1),
            (Focus::List, 0),
            (Focus::List, 1),
            (Focus::Places, 1),
        ] {
            press(&mut f.app, &[KeyCode::Tab]);
            assert_eq!((f.app.focus, f.app.active()), (focus, active));
        }
    }

    #[test]
    fn notices_go_after_four_seconds_receipts_and_errors_after_eight() {
        let mut f = fixture();
        let clock = manual_clock(&mut f.app);
        let wait = |secs| clock.set(clock.get() + Duration::from_secs(secs));
        press(&mut f.app, &[KeyCode::Char('I')]);
        assert_eq!(f.app.message.as_ref().map(|m| m.undoable), Some(false));
        wait(3);
        f.app.tick();
        assert!(f.app.message.is_some());
        wait(1);
        f.app.tick();
        assert_eq!(f.app.message, None, "a notice: four seconds");

        // A change undo can take back says so, and stays eight seconds.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        let receipt = f.app.message.clone().unwrap();
        assert_eq!(
            (receipt.text.as_str(), receipt.undoable),
            ("Moved 1 item to Trash", true)
        );
        wait(7);
        f.app.tick();
        assert!(f.app.message.is_some());
        wait(1);
        f.app.tick();
        assert_eq!(f.app.message, None);
        // So does what undo took back.
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(
            f.app.message.as_ref().map(|m| m.lasts),
            Some(Duration::from_secs(8))
        );

        // Escape dismisses an error at once.
        f.app.navigate(&f.home.join("missing"));
        assert!(f.app.message.as_ref().is_some_and(|m| m.is_error));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn the_volume_is_asked_once_for_each_folder_and_never_while_drawing() {
        let mut f = fixture();
        f.app.tick();
        for _ in 0..3 {
            crate::ui::testing::buffer(&mut f.app, 120, 30);
            f.app.tick();
        }
        assert_eq!(f.system.volumes_asked.get(), 1);
        assert_eq!(f.app.volume().map(|v| v.kind.as_str()), Some("apfs"));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        f.app.tick();
        f.app.tick();
        assert_eq!(f.system.volumes_asked.get(), 2, "a new folder");
        press(&mut f.app, &[KeyCode::Backspace]);
        f.app.tick();
        assert_eq!(f.system.volumes_asked.get(), 2, "kept from before");
        // Ctrl+R and a change to files ask again: the space left may differ.
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        f.app.tick();
        assert_eq!(f.system.volumes_asked.get(), 3);
        // A volume that can't be told leaves the footer without it.
        *f.system.volume.borrow_mut() = None;
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        f.app.tick();
        let drawn = crate::ui::testing::screen(&mut f.app, 120, 30);
        assert!(!drawn[29].contains("free"), "{}", drawn[29]);
        assert!(drawn[29].trim_end().ends_with("? keys"));
    }

    #[test]
    fn alt_p_hides_and_shows_the_preview_and_remembers_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let state = dir.path().join("state");
        let mut app = launch_one_pane(&home, Some(&state));
        assert!(app.preview_visible, "shown at first");
        click_preview(&mut app);
        // Hidden, it gives the keyboard back to the list.
        app.handle_key(alt(KeyCode::Char('p')));
        assert!(!app.preview_visible);
        assert_eq!(app.focus, Focus::List);
        let saved = fs::read_to_string(state.join("state")).unwrap();
        assert!(saved.contains("preview=off"), "{saved}");
        assert!(!launch(&home, Some(&state)).preview_visible);
        app.handle_key(alt(KeyCode::Char('p')));
        assert!(launch(&home, Some(&state)).preview_visible);
    }

    #[test]
    fn with_the_preview_focused_up_and_down_scroll_it_and_leave_the_cursor() {
        let mut f = several();
        let lines: String = (1..=60).map(|n| format!("line {n}\n")).collect();
        fs::write(f.home.join("notes.txt"), lines).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::End]);
        f.app.tick();
        crate::ui::testing::buffer(&mut f.app, 120, 30);
        let at = f.app.pane().cursor;
        click_preview(&mut f.app);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('j')]);
        assert_eq!(f.app.preview_scroll, 2);
        press(&mut f.app, &[KeyCode::Char('k')]);
        assert_eq!(f.app.preview_scroll, 1);
        press(&mut f.app, &[KeyCode::PageDown]);
        assert_eq!(f.app.preview_scroll, 1 + f.app.preview_rows);
        press(&mut f.app, &[KeyCode::PageUp, KeyCode::Up, KeyCode::Up]);
        assert_eq!(f.app.preview_scroll, 0);
        assert_eq!(f.app.pane().cursor, at, "the list stays where it was");
        // Another item starts at the top.
        press(&mut f.app, &[KeyCode::Down, KeyCode::Esc, KeyCode::Up]);
        f.app.tick();
        assert_eq!(f.app.preview_scroll, 0);
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
    fn s_goes_round_the_sort_keys_and_capital_s_reverses_keeping_the_selection() {
        let f = fixture();
        fs::write(f.home.join("big.bin"), vec![0u8; 100]).unwrap();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch(&f.home, Some(state.path()));
        press(&mut app, &[KeyCode::End]);
        assert_eq!(selected_name(&app), Some("notes.txt"));
        let header = |app: &mut App| crate::ui::testing::screen(app, 120, 30)[0].clone();

        // Size, smallest first; folders lead, by name.
        press(&mut app, &[KeyCode::Char('s')]);
        assert_eq!(
            app.sort,
            SortMode {
                key: SortKey::Size,
                reversed: false
            }
        );
        assert_eq!(
            names(&app),
            ["Documents", "Pictures", "notes.txt", "big.bin"]
        );
        assert_eq!(selected_name(&app), Some("notes.txt"));
        assert!(header(&mut app).ends_with(" Size ▴ "));
        // S reverses it, each block the other way round, folders still first.
        press(&mut app, &[KeyCode::Char('S')]);
        assert_eq!(app.sort, BY_SIZE);
        assert_eq!(
            names(&app),
            ["Pictures", "Documents", "big.bin", "notes.txt"]
        );
        assert_eq!(selected_name(&app), Some("notes.txt"));
        assert!(header(&mut app).ends_with(" Size ▾ "));
        assert_eq!(app.message, None, "the header says it");
        // Remembered for the next launch.
        assert_eq!(launch(&f.home, Some(state.path())).sort, BY_SIZE);

        // The next key always starts ascending.
        press(&mut app, &[KeyCode::Char('s')]);
        assert_eq!(
            app.sort,
            SortMode {
                key: SortKey::Modified,
                reversed: false
            }
        );
        assert!(header(&mut app).ends_with(" Modified ▴ "));
        press(&mut app, &[KeyCode::Char('s')]);
        assert_eq!(app.sort, BY_KIND);
        press(&mut app, &[KeyCode::Char('s')]);
        assert_eq!(app.sort, SortMode::Name);
        assert_eq!(
            names(&app),
            ["Documents", "Pictures", "big.bin", "notes.txt"]
        );
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
    fn the_view_starts_as_the_list_and_ctrl_1_2_3_switch_it_and_save_it() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        assert_eq!(app.view(), ViewMode::List);
        assert!(
            !state.path().join("state").exists(),
            "nothing is written until something changes"
        );

        app.handle_key(ctrl(KeyCode::Char('2')));
        assert_eq!(app.view(), ViewMode::Columns);
        assert_eq!(
            launch(&f.home, Some(state.path())).view(),
            ViewMode::List,
            "the next launch opens in the list all the same"
        );
        app.handle_key(ctrl(KeyCode::Char('1')));
        assert_eq!(app.view(), ViewMode::List);
        let saved = fs::read_to_string(state.path().join("state")).unwrap();
        assert_eq!(
            saved,
            "view=list\ntiles=normal\npreview=on\nsort=name\nreverse=off\ndetail=size\nplaces=on\nhidden=off\nspace=terminal\ncolumns=size,user,permissions,kind,modified\n"
        );
        app.handle_key(ctrl(KeyCode::Char('3')));
        assert_eq!(app.view(), ViewMode::Grid);
        assert_eq!(state::load(state.path()).view, ViewMode::Grid);
        assert_eq!(launch(&f.home, Some(state.path())).view(), ViewMode::List);
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
        f.app.handle_key(key(KeyCode::Char('.')));
        assert!(f.app.show_hidden);
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", ".hidden", "notes.txt"]
        );
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        f.app.handle_key(key(KeyCode::Char('.')));
        assert!(!f.app.show_hidden);
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
    }

    #[test]
    fn plus_and_minus_resize_the_tiles_and_remember_the_size() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch(&f.home, Some(state.path()));
        assert_eq!(app.tile, TileSize::Normal);
        let mut sizes = Vec::new();
        for c in ['+', '+', '-', '-', '-', '0', '-', '+'] {
            app.handle_key(key(KeyCode::Char(c)));
            sizes.push(app.tile);
        }
        use TileSize::{Large, Normal, Small};
        // 0, Ctrl+= and Ctrl+0 are no longer tile keys.
        assert_eq!(
            sizes,
            [Large, Large, Normal, Small, Small, Small, Small, Normal]
        );
        for chord in [ctrl(KeyCode::Char('=')), ctrl(KeyCode::Char('0'))] {
            app.handle_key(chord);
            assert_eq!(app.tile, Normal);
        }
        app.handle_key(shift(KeyCode::Char('+')));
        let saved = fs::read_to_string(state.path().join("state")).unwrap();
        assert_eq!(
            saved,
            "view=list\ntiles=large\npreview=on\nsort=name\nreverse=off\ndetail=size\nplaces=on\nhidden=off\nspace=terminal\ncolumns=size,user,permissions,kind,modified\n"
        );
        assert_eq!(launch(&f.home, Some(state.path())).tile, Large);
    }

    #[test]
    fn in_columns_right_goes_into_a_folder_and_left_back_out() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
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
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        fs::write(f.home.join("later.txt"), vec![0u8; 100]).unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Up]);
        crate::ui::testing::screen(&mut f.app, 120, 24);
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", "notes.txt"],
            "moving and drawing don't read the folders again"
        );

        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", "later.txt", "notes.txt"]
        );
        f.app.handle_key(key(KeyCode::Char('.')));
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Documents", "Pictures", ".hidden", "later.txt", "notes.txt"]
        );
        press(&mut f.app, &[KeyCode::Char('s'), KeyCode::Char('S')]);
        assert_eq!(f.app.sort, BY_SIZE);
        assert_eq!(
            ancestor_names(&f.app, 0),
            ["Pictures", "Documents", "later.txt", "notes.txt", ".hidden"]
        );
        assert_eq!(f.app.pane().ancestors[0].on_path, Some(0));

        // The other views have no use for them.
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        assert!(f.app.pane().ancestors.is_empty());
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        assert_eq!(f.app.pane().ancestors.len(), 1);
    }

    #[test]
    fn space_opens_the_quick_look_card_whose_keys_move_zoom_scroll_and_close() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        press(&mut f.app, &[KeyCode::Char(' ')]);
        let look = f.app.look.clone().expect("the card");
        assert_eq!((look.zoom, look.scroll), (0, 0));
        assert!(f.system.previewed.borrow().is_empty(), "not macOS's window");
        assert!(f.system.opened.borrow().is_empty(), "nothing is opened");

        // `j`, `k` and the arrows move the list's cursor; the card follows.
        let mut seen = Vec::new();
        for code in [
            KeyCode::Char('j'),
            KeyCode::Down,
            KeyCode::Char('k'),
            KeyCode::Up,
        ] {
            press(&mut f.app, &[code]);
            seen.push(selected_name(&f.app).unwrap_or_default().to_string());
        }
        assert_eq!(seen, ["b.txt", "c.txt", "b.txt", "a.txt"]);
        assert!(f.app.look.is_some());

        // `+` and `-` zoom as far as they go each way; `0` fits again.
        let zoom = |app: &App| app.look.as_ref().map(|look| look.zoom);
        press(&mut f.app, &[KeyCode::Char('+'); 5]);
        assert_eq!(zoom(&f.app), Some(MOST_ZOOM));
        press(&mut f.app, &[KeyCode::Char('-'); 9]);
        assert_eq!(zoom(&f.app), Some(-MOST_ZOOM));
        press(&mut f.app, &[KeyCode::Char('0')]);
        assert_eq!(zoom(&f.app), Some(0));

        // Page Down and Page Up scroll by what the card showed; another
        // item starts at the top.
        f.app.look.as_mut().unwrap().rows = 4;
        press(
            &mut f.app,
            &[KeyCode::PageDown, KeyCode::PageDown, KeyCode::PageUp],
        );
        assert_eq!(f.app.look.as_ref().map(|look| look.scroll), Some(4));
        press(&mut f.app, &[KeyCode::Char('j')]);
        f.app.tick();
        assert_eq!(f.app.look.as_ref().map(|look| look.scroll), Some(0));

        // No other key reaches what is underneath.
        press(
            &mut f.app,
            &[
                KeyCode::Char('q'),
                KeyCode::Char('d'),
                KeyCode::Char('d'),
                KeyCode::Delete,
                KeyCode::Enter,
                KeyCode::Char('m'),
                KeyCode::Char('?'),
                KeyCode::Tab,
            ],
        );
        assert!(!f.app.should_quit && f.app.menu.is_none() && !f.app.help_visible);
        assert_eq!(
            (f.app.focus, f.app.pane().cwd.clone()),
            (Focus::List, f.home.clone())
        );
        assert!(on_disk(&f.trash).is_empty() && f.system.opened.borrow().is_empty());
        assert_eq!(selected_name(&f.app), Some("b.txt"));

        // Escape closes it, and so does Space.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.look.is_none());
        press(&mut f.app, &[KeyCode::Char(' '), KeyCode::Char(' ')]);
        assert!(f.app.look.is_none());
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn space_with_nothing_selected_does_nothing() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));
        assert!(f.app.selected().is_none());
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert!(f.app.look.is_none());
        assert!(f.system.previewed.borrow().is_empty());
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn the_newest_thumbnails_asked_for_are_made_first() {
        let (mut f, thumbnailer) = pictures();
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        f.app.handle_key(ctrl(KeyCode::Char('r')));
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
        f.app.handle_key(ctrl(KeyCode::Char('2')));
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
        match &app.prompt {
            Some(Prompt {
                asking: Asking::Rename(_) | Asking::NewFolder(_),
                input,
                ..
            }) => (input.text(), input.cursor()),
            _ => panic!("no text field is open"),
        }
    }

    /// What the prompt that is open asks for.
    fn asking(app: &App) -> Option<Asking> {
        app.prompt.as_ref().map(|prompt| prompt.asking.clone())
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
        assert_eq!(said(&f.app), ("Copied 1 item", false));

        // A copy can be pasted again: the name is taken, so it asks first,
        // with nothing written. Keep both leaves the one there as it is.
        fs::write(documents.join("notes.txt"), "changed since").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        let asked = f.app.question.as_ref().map(PasteQuestion::text);
        assert_eq!(
            asked.as_deref(),
            Some("notes.txt already exists in Documents.")
        );
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(&documents), ["notes.txt"]);
        press(&mut f.app, &[KeyCode::Enter]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["notes copy.txt", "notes.txt"]);
        assert_eq!(read(documents.join("notes.txt")), "changed since");
        assert_eq!(read(documents.join("notes copy.txt")), "hello");
        assert_eq!(selected_name(&f.app), Some("notes copy.txt"));
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
        assert_eq!(said(&f.app), ("Nothing to paste", false));
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
    fn r_renames_on_the_bottom_row_with_the_name_before_its_extension_selected() {
        use crate::ui::testing::{buffer, cursor, find};
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        let at = f.app.pane().cursor;
        press(&mut f.app, &[KeyCode::Char('r')]);
        assert_eq!(field(&f.app), ("notes.txt".to_string(), 5));
        let input = &f.app.prompt.as_ref().unwrap().input;
        assert_eq!(
            input.selection(),
            Some((0, 5)),
            "the name without its extension"
        );

        // The label, the name with its stem on the selection's fill, and
        // the keys against the right edge, all muted but the name.
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "Rename notes.txt ", 0..120), Some((1, 29)));
        assert_eq!(drawn[(1, 29)].fg, theme::MUTED);
        assert_eq!(drawn[(8, 29)].bg, theme::FIELD_SELECTION);
        assert_eq!(drawn[(12, 29)].bg, theme::FIELD_SELECTION);
        assert_eq!(drawn[(13, 29)].bg, theme::FIELD, "the extension");
        assert_eq!(drawn[(8, 29)].fg, theme::FOREGROUND);
        let hint = "enter saves · esc cancels · ctrl-a selects the whole name";
        let x = 119 - hint.chars().count() as u16;
        assert_eq!(find(&drawn, hint, 0..120), Some((x, 29)));
        assert_eq!(drawn[(x, 29)].fg, theme::MUTED);
        assert_eq!(
            find(&drawn, "3 items", 0..120),
            None,
            "the footer gives way"
        );

        // Typed into the name, over the selection: `q` doesn't quit and
        // the arrows don't reach the files.
        press(
            &mut f.app,
            &[KeyCode::Char('q'), KeyCode::Down, KeyCode::Up],
        );
        press(&mut f.app, &[KeyCode::Char('?'), KeyCode::Char('s')]);
        assert!(!f.app.should_quit && !f.app.help_visible);
        assert_eq!((f.app.pane().cursor, f.app.sort), (at, SortMode::Name));
        assert_eq!(field(&f.app), ("q?s.txt".to_string(), 3));
        assert_eq!(cursor(&mut f.app, 120, 30), (11, 29), "the caret");
        press(&mut f.app, &[KeyCode::Backspace; 3]);
        type_text(&mut f.app, "todo");
        press(&mut f.app, &[KeyCode::Enter]);

        assert!(f.app.prompt.is_none());
        assert_eq!(read(f.home.join("todo.txt")), "hello");
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(names(&f.app), ["Documents", "Pictures", "todo.txt"]);
        assert_eq!(selected_name(&f.app), Some("todo.txt"));
        assert_eq!(said(&f.app), ("Renamed", false));

        // Ctrl+A selects the whole name.
        press(&mut f.app, &[KeyCode::F(2)]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        let input = &f.app.prompt.as_ref().unwrap().input;
        assert_eq!(input.selection(), Some((0, 8)));
        type_text(&mut f.app, "plan");
        assert_eq!(field(&f.app), ("plan".to_string(), 4));
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.prompt.is_none());

        // A folder has no extension: all of it is selected. Escape leaves it be.
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::F(2)]);
        assert_eq!(field(&f.app), ("Documents".to_string(), 9));
        let input = &f.app.prompt.as_ref().unwrap().input;
        assert_eq!(input.selection(), Some((0, 9)));
        type_text(&mut f.app, " old");
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.prompt.is_none());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "todo.txt"]);
    }

    #[test]
    fn a_rename_that_cannot_be_done_says_why_in_the_prompt_and_changes_nothing() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(refused(&f.app), Some("Pictures already exists here."));
        assert_eq!(field(&f.app).0, "Pictures", "the prompt stays");
        assert_eq!(f.app.message, None);
        // In the error colour, one cell after the field.
        let drawn = crate::ui::testing::buffer(&mut f.app, 120, 30);
        let at = crate::ui::testing::find(&drawn, "Pictures already exists here.", 0..120);
        assert_eq!(at, Some((18, 29)));
        assert_eq!(drawn[(18, 29)].fg, theme::ERROR);
        press(&mut f.app, &[KeyCode::Esc]);

        for (typed, why) in [
            ("/2", "A name cannot contain /."),
            ("..", "A name cannot be a single dot or a double dot."),
            ("", "A name cannot be empty."),
            ("  ", "A name cannot be empty."),
        ] {
            press(&mut f.app, &[KeyCode::F(2)]);
            clear_field(&mut f.app);
            type_text(&mut f.app, typed);
            press(&mut f.app, &[KeyCode::Enter]);
            assert_eq!(refused(&f.app), Some(why), "{typed:?}");
            press(&mut f.app, &[KeyCode::Esc]);
        }

        // Unchanged: it closes, and nothing happens or is said.
        press(&mut f.app, &[KeyCode::F(2), KeyCode::Enter]);
        assert!(f.app.prompt.is_none());
        assert_eq!(f.app.message, None);

        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(on_disk(&f.home.join("Pictures")), ["Trip", "a.jpg"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
    }

    #[test]
    fn capital_u_duplicates_next_to_the_original() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(key(KeyCode::Char('U')));
        assert_eq!(f.app.working(), Some("Duplicating notes.txt…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes copy.txt")), "hello");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(selected_name(&f.app), Some("notes copy.txt"));
        assert_eq!(said(&f.app), ("Duplicated 1 item.", false));

        // With Shift held, as some terminals send a capital.
        select(&mut f.app, "notes.txt");
        f.app.handle_key(shift(KeyCode::Char('U')));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes copy 2.txt")), "hello");

        // Ctrl+Shift+D is not a key here, and Ctrl+D pages.
        let spelled = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        f.app.handle_key(KeyEvent::new(KeyCode::Char('d'), spelled));
        f.app.handle_key(ctrl_shift('d'));
        f.app.handle_key(ctrl(KeyCode::Char('d')));
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(&f.home).len(), 5, "{:?}", on_disk(&f.home));
    }

    #[test]
    fn n_and_ctrl_shift_n_ask_for_a_folder_name_and_make_the_folder() {
        let mut f = fixture();
        f.app.handle_key(ctrl_shift('n'));
        assert!(matches!(asking(&f.app), Some(Asking::NewFolder(_))));
        assert_eq!(field(&f.app), ("New Folder".to_string(), 10));
        let input = &f.app.prompt.as_ref().unwrap().input;
        assert_eq!(input.selection(), Some((0, 10)), "typing replaces it");
        let drawn = crate::ui::testing::buffer(&mut f.app, 120, 30);
        let label = crate::ui::testing::find(&drawn, "Folder name New Folder ", 0..120);
        assert_eq!(label, Some((1, 29)));
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.prompt.is_none());
        assert!(f.home.join("New Folder").is_dir());
        assert_eq!(selected_name(&f.app), Some("New Folder"));
        assert_eq!(said(&f.app), ("Folder created", false));

        // The same name again is refused; nothing is made or replaced.
        fs::write(f.home.join("New Folder").join("kept.txt"), "kept").unwrap();
        press(&mut f.app, &[KeyCode::Char('N')]);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(refused(&f.app), Some("New Folder already exists here."));
        assert_eq!(on_disk(&f.home.join("New Folder")), ["kept.txt"]);
        press(&mut f.app, &[KeyCode::Esc]);

        let spelled = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        f.app.handle_key(KeyEvent::new(KeyCode::Char('n'), spelled));
        type_text(&mut f.app, "Plans");
        assert_eq!(field(&f.app).0, "Plans");
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.home.join("Plans").exists(), "Escape makes nothing");

        // Ctrl+N without Shift is not it.
        f.app.handle_key(ctrl(KeyCode::Char('n')));
        assert!(f.app.prompt.is_none());
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
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));

        // The last one gone, the cursor falls on the one before.
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert_eq!(selected_name(&f.app), Some("Documents"));

        // Command+Backspace is not a key here.
        let command = KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER);
        f.app.handle_key(command);
        assert_eq!(names(&f.app), ["Documents"]);
        press(&mut f.app, &[KeyCode::Delete]);
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
        assert_eq!(said(&f.app), ("Nothing to paste", false));

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
        assert_eq!(said(&f.app), ("Undid the move to Trash", false));

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
        assert_eq!(said(&f.app), ("Undid the new folder", false));
        assert!(f.trash.join("New Folder").is_dir(), "trashed, not deleted");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid the rename", false));
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
        assert_eq!(said(&f.app), ("Undid the copy", false));
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
        // Pictures is there already: keep both.
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        press(&mut f.app, &[KeyCode::Char('b')]);
        assert_eq!(f.app.working(), Some("Copying 1 item…"));

        let wait = ("Try again once the work under way is done.", false);
        for refused in [
            key(KeyCode::Delete),
            shift(KeyCode::Delete),
            key(KeyCode::F(2)),
            ctrl_shift('n'),
            key(KeyCode::Char('U')),
            ctrl(KeyCode::Char('v')),
            key(KeyCode::Char('p')),
            key(KeyCode::Char('M')),
            key(KeyCode::Char('r')),
            key(KeyCode::Char('N')),
            key(KeyCode::Char('z')),
            ctrl(KeyCode::Char('z')),
            ctrl(KeyCode::Char('y')),
            key(KeyCode::Char('q')),
            key(KeyCode::Char('w')),
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
        assert!(on_disk(&f.home.join("Pictures copy")).len() < 602);

        // What got across before the stop is on record: undo moves it to the trash.
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert!(!f.home.join("Pictures copy").exists());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        // And with the work over, the keys work again.
        press(&mut f.app, &[KeyCode::Char('q')]);
        assert!(f.app.should_quit);
    }

    /// Opens the trash folder, which the menu offers.
    fn open_trash(app: &mut App) {
        let trash = app.trash_dir.clone();
        app.navigate(&trash);
    }

    #[test]
    fn trash_opens_the_trash_folder_where_delete_asks_to_delete_for_good() {
        let mut f = fixture();
        for name in ["notes.txt", "Pictures"] {
            select(&mut f.app, name);
            press(&mut f.app, &[KeyCode::Delete]);
        }
        open_trash(&mut f.app);
        assert_eq!(f.app.pane().cwd, f.trash);
        assert_eq!(names(&f.app), ["Pictures", "notes.txt"]);
        assert_eq!(f.app.focus, Focus::List);

        // Delete doesn't move it to the trash again: it asks, like Shift+Delete.
        select(&mut f.app, "notes.txt");
        for delete in [vec![key(KeyCode::Delete)], vec![key(KeyCode::Char('d')); 2]] {
            for key in delete {
                f.app.handle_key(key);
            }
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
        open_trash(&mut f.app);
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
        open_trash(&mut f.app);
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        let refused = "Trash: Permission was refused. Give your terminal \
                       Full Disk Access in System Settings to see it.";
        assert_eq!(said(&f.app), (refused, true));
    }

    // Go to (`:` and Ctrl+L), a prompt on the bottom row, and the search
    // (/) in the header. Both take the keyboard while they are open.

    fn path_field(app: &App) -> (String, usize) {
        match &app.prompt {
            Some(Prompt {
                asking: Asking::GoTo,
                input,
                ..
            }) => (input.text(), input.cursor()),
            _ => panic!("the path field isn't open"),
        }
    }

    fn open_path_field(app: &mut App) {
        app.handle_key(ctrl(KeyCode::Char('l')));
    }

    /// Why the prompt that is open refused what was typed.
    fn refused(app: &App) -> Option<&str> {
        app.prompt.as_ref()?.error.as_deref()
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
    fn colon_and_ctrl_l_ask_where_to_go_on_the_bottom_row() {
        use crate::ui::testing::{buffer, cursor, find};
        for chord in [key(KeyCode::Char(':')), ctrl(KeyCode::Char('l'))] {
            let mut f = fixture();
            fs::write(f.home.join("Pictures").join("b.jpg"), "b").unwrap();
            f.app.handle_key(chord);
            assert_eq!(path_field(&f.app), (String::new(), 0), "{chord:?}");
            let drawn = buffer(&mut f.app, 120, 30);
            // The label muted, a space, the field on its fill; the footer gone.
            assert_eq!(find(&drawn, "Go to ", 0..120), Some((1, 29)));
            assert_eq!(drawn[(1, 29)].fg, theme::MUTED);
            assert_eq!(drawn[(7, 29)].bg, theme::FIELD);
            let bottom: String = (0..120).map(|x| drawn[(x, 29)].symbol()).collect();
            assert!(!bottom.contains("items"), "{bottom}");
            assert_eq!(drawn[(1, 0)].symbol(), "~", "the header keeps the path");
            assert_eq!(cursor(&mut f.app, 120, 30), (7, 29));

            // What Tab would add shows after what is typed, in the
            // extension's ink; Tab takes it.
            type_text(&mut f.app, "~/Pi");
            let drawn = buffer(&mut f.app, 120, 30);
            assert_eq!(find(&drawn, "~/Pictures/ ", 0..120), Some((7, 29)));
            assert_eq!(drawn[(11, 29)].fg, theme::EXTENSION);
            assert_eq!(drawn[(11, 29)].bg, theme::FIELD);
            assert_eq!(cursor(&mut f.app, 120, 30), (11, 29));
            press(&mut f.app, &[KeyCode::Tab]);
            assert_eq!(path_field(&f.app), ("~/Pictures/".to_string(), 11));
            press(&mut f.app, &[KeyCode::Enter]);
            assert!(f.app.prompt.is_none());
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        }
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
            assert!(f.app.prompt.is_none(), "{typed}");
            assert_eq!(f.app.message, None, "{typed}");
            // It is a step in the history like any other.
            f.app.handle_key(key(KeyCode::Char('H')));
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
        assert!(f.app.prompt.is_none());
        assert!(f.system.opened.borrow().is_empty(), "selected, not opened");
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));

        // One in the folder that is showing only moves the cursor.
        let mut f = fixture();
        go_to_typed(&mut f.app, "notes.txt");
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(f.app.prompt.is_none());
        f.app.handle_key(key(KeyCode::Char('H')));
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
        let missing = "~/Nowhere/else does not exist or cannot be reached.";
        assert_eq!(path_field(&f.app), ("~/Nowhere/else".to_string(), 14));
        assert_eq!(refused(&f.app), Some(missing));
        assert_eq!(f.app.message, None);
        assert_eq!(f.app.pane().cwd, f.home);
        // After the field, in the error colour.
        let drawn = crate::ui::testing::buffer(&mut f.app, 120, 24);
        let at = crate::ui::testing::find(&drawn, missing, 0..120);
        assert_eq!(at, Some((23, 23)));
        assert_eq!(drawn[(23, 23)].fg, theme::ERROR);
        // Typing again takes it away.
        type_text(&mut f.app, "/");
        assert_eq!(refused(&f.app), None);

        // So does a folder that is there and can't be read.
        let locked = f.home.join("Documents");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        clear_field(&mut f.app);
        type_text(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(path_field(&f.app).0, "Documents");
        assert_eq!(refused(&f.app), Some("Documents: Permission was refused."));
        assert_eq!(f.app.pane().cwd, f.home);

        // It can be put right where it stands.
        clear_field(&mut f.app);
        type_text(&mut f.app, "~/Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert!(f.app.prompt.is_none());
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
        assert_eq!((f.app.focus, &f.app.pane().cwd), (Focus::List, &f.home));
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
        f.app.handle_key(key(KeyCode::Char('.')));
        open_path_field(&mut f.app);
        // Two of them now, with nothing alike.
        assert_eq!(completed(&mut f.app, "~/Box/"), "~/Box/");
    }

    #[test]
    fn tab_reads_the_folder_as_it_is_when_pressed_and_drawing_reads_nothing() {
        let mut f = fixture();
        open_path_field(&mut f.app);
        type_text(&mut f.app, "~/Mus");
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
        assert!(f.app.prompt.is_none());
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(selected_name(&f.app), Some("a.jpg"));
        assert_eq!(f.app.message, None);
        // Nothing was added to the history: back is still the home folder.
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn while_the_path_field_is_open_every_key_goes_to_it() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down]);
        open_path_field(&mut f.app);
        clear_field(&mut f.app);
        // The command keys are only letters here.
        type_text(&mut f.app, "qsS/? ");
        assert_eq!(path_field(&f.app), ("qsS/? ".to_string(), 6));
        for letter in "djkhlyxpvVMUrNzEI.:".chars() {
            press(&mut f.app, &[KeyCode::Char(letter)]);
        }
        assert_eq!(path_field(&f.app).0, "qsS/? djkhlyxpvVMUrNzEI.:");
        clear_field(&mut f.app);
        type_text(&mut f.app, "qsS/? ");
        for other in [
            key(KeyCode::Up),
            key(KeyCode::Down),
            key(KeyCode::PageDown),
            key(KeyCode::BackTab),
            key(KeyCode::F(2)),
            key(KeyCode::F(5)),
            shift(KeyCode::Delete),
            ctrl(KeyCode::Char('c')),
            ctrl(KeyCode::Char('v')),
            ctrl(KeyCode::Char('p')),
            ctrl(KeyCode::Char('2')),
            ctrl(KeyCode::Char('l')),
            ctrl_shift('n'),
            alt(KeyCode::Char('b')),
        ] {
            f.app.handle_key(other);
            assert_eq!(path_field(&f.app).0, "qsS/? ", "{other:?}");
        }
        assert!(!f.app.should_quit && !f.app.help_visible && f.app.dialog.is_none());
        let (sort, view, hidden) = (f.app.sort, f.app.view(), f.app.show_hidden);
        assert_eq!(
            (sort, view, hidden),
            (SortMode::Name, ViewMode::List, false)
        );
        assert_eq!((f.app.focus, f.app.pane().cursor), (Focus::List, 1));
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
    /// What Find finds for "trip", best first: the whole name, the start of
    /// a name, the start of a word.
    const TRIPS: [&str; 3] = ["Trip", "trip-plan.md", "day-trip.jpg"];
    /// The `.txt` files of `several()`.
    const TEXTS: [&str; 6] = ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "notes.txt"];

    /// Types into the find field and lets the walk look everywhere.
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

    fn typing_a_filter(app: &App) -> bool {
        matches!(app.field, Some(Field::Filter))
    }

    fn filter_text(app: &App) -> String {
        app.pane().filter().expect("no filter is on").input.text()
    }

    /// The bottom row as drawn 100 cells wide, without its padding.
    fn bottom_row(app: &mut App) -> String {
        let rows = crate::ui::testing::screen(app, 100, 20);
        rows.last().unwrap().trim().to_string()
    }

    /// Ticks until `done` holds, as the event loop would.
    fn tick_until(app: &mut App, done: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !done(app) {
            assert!(std::time::Instant::now() < deadline, "it never happened");
            std::thread::sleep(Duration::from_millis(2));
            app.tick();
        }
    }

    // The filter, `/`: narrows the listing as it is typed.

    #[test]
    fn slash_opens_the_filter_on_the_bottom_row_and_narrows_the_folder_as_it_is_typed() {
        let mut f = several();
        press(&mut f.app, &[KeyCode::Char('/')]);
        assert!(typing_a_filter(&f.app));
        assert_eq!(filter_text(&f.app), "");
        assert_eq!(names(&f.app).len(), 8, "nothing typed hides nothing");

        type_text(&mut f.app, "TXT");
        assert_eq!(names(&f.app), TEXTS);
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.app.pane().is_folder(), "still the folder's own items");
        let row = bottom_row(&mut f.app);
        assert!(row.starts_with("Filter TXT"), "{row}");
        assert!(row.contains("6 of 8 shown"), "{row}");

        type_text(&mut f.app, "!");
        assert!(names(&f.app).is_empty());
        press(&mut f.app, &[KeyCode::Backspace; 4]);
        type_text(&mut f.app, "note");
        assert_eq!(names(&f.app), ["notes.txt"]);
    }

    #[test]
    fn a_close_match_shows_only_when_nothing_matches_exactly() {
        let mut f = several();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "ntoes");
        assert_eq!(names(&f.app), ["notes.txt"]);
        let row = bottom_row(&mut f.app);
        assert!(row.contains("1 of 8 shown · 1 close match"), "{row}");
        press(&mut f.app, &[KeyCode::Backspace; 5]);
        type_text(&mut f.app, "notes");
        assert_eq!(names(&f.app), ["notes.txt"]);
        let row = bottom_row(&mut f.app);
        assert!(
            row.contains("1 of 8 shown") && !row.contains("close"),
            "{row}"
        );
    }

    #[test]
    fn enter_or_down_keeps_the_filter_and_hands_the_keyboard_to_the_list() {
        for done in [KeyCode::Enter, KeyCode::Down] {
            let mut f = several();
            press(&mut f.app, &[KeyCode::Char('/')]);
            type_text(&mut f.app, "txt");
            press(&mut f.app, &[done]);
            assert!(f.app.field.is_none(), "{done:?}");
            assert_eq!(names(&f.app), TEXTS);
            assert_eq!(selected_name(&f.app), Some("a.txt"), "nothing was opened");
            assert!(f.system.opened.borrow().is_empty());
            // The footer says what is filtered, and the keys are the list's again.
            let row = bottom_row(&mut f.app);
            assert!(row.starts_with("Filter txt  6 of 8 shown"), "{row}");
            press(&mut f.app, &[KeyCode::Char('j')]);
            assert_eq!(selected_name(&f.app), Some("b.txt"));
            // `/` goes back to the field as it was left.
            press(&mut f.app, &[KeyCode::Char('/')]);
            assert!(typing_a_filter(&f.app));
            assert_eq!(filter_text(&f.app), "txt");
        }
    }

    #[test]
    fn escape_clears_the_filter_from_the_field_or_from_the_list() {
        for from_the_list in [false, true] {
            let mut f = several();
            press(&mut f.app, &[KeyCode::Char('/')]);
            type_text(&mut f.app, "txt");
            press(&mut f.app, &[KeyCode::Down]);
            if from_the_list {
                press(&mut f.app, &[KeyCode::Down]);
            }
            press(&mut f.app, &[KeyCode::Esc]);
            assert!(f.app.field.is_none() && f.app.pane().filter().is_none());
            assert_eq!(names(&f.app).len(), 8);
            let kept = if from_the_list { "b.txt" } else { "a.txt" };
            assert_eq!(
                selected_name(&f.app),
                Some(kept),
                "the cursor stays on its item"
            );
            assert!(bottom_row(&mut f.app).starts_with("8 items"));
        }
    }

    #[test]
    fn ctrl_a_marks_only_what_the_filter_shows_and_that_is_what_is_acted_on() {
        let mut f = several();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "txt");
        // From the field: the keyboard moves on to the list, everything shown marked.
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert!(f.app.field.is_none());
        assert_eq!(chosen(&f.app), TEXTS);
        // From the list as well.
        press(&mut f.app, &[KeyCode::Char('V')]);
        assert_eq!(chosen(&f.app), ["a.txt"], "none marked: the cursor's");
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(chosen(&f.app), TEXTS);

        // Moving them to the trash takes those and no other, and the filter stays.
        press(&mut f.app, &[KeyCode::Delete]);
        finish_work(&mut f.app);
        let left: Vec<String> = on_disk(&f.home)
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .collect();
        assert_eq!(left, ["Documents", "Pictures"]);
        assert_eq!(filter_text(&f.app), "txt");
        assert!(names(&f.app).is_empty());
        let drawn = crate::ui::testing::screen(&mut f.app, 100, 20);
        assert!(drawn.iter().any(|row| row.contains("NOTHING MATCHES txt")));
        assert!(drawn.iter().any(|row| row.contains(crate::icons::SEARCH)));
        assert!(bottom_row(&mut f.app).starts_with("Filter txt  0 of 2 shown"));
    }

    #[test]
    fn tab_switches_the_filter_between_this_folder_and_its_subfolders() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "trip");
        assert!(names(&f.app).is_empty(), "nothing in the folder itself");

        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(
            f.app.pane().filter().unwrap().scope,
            FilterScope::Subfolders
        );
        finish_search(&mut f.app);
        // The folder's own first, then each level down; hidden ones stay out.
        assert_eq!(names(&f.app), ["trip-plan.md", "Trip", "day-trip.jpg"]);
        let row = bottom_row(&mut f.app);
        assert!(row.contains("3 of 7 shown in ~ and subfolders"), "{row}");
        assert!(typing_a_filter(&f.app), "Tab stays in the field");
        // What is narrowed below is the walk's, typed or not.
        type_text(&mut f.app, ".");
        assert_eq!(names(&f.app), ["day-trip.jpg"]);

        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.pane().filter().unwrap().scope, FilterScope::Folder);
        assert!(names(&f.app).is_empty());
        assert!(bottom_row(&mut f.app).contains("0 of 3 shown"));
    }

    #[test]
    fn the_filter_ends_when_the_pane_goes_to_another_folder() {
        let mut f = several();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "pic");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert!(f.app.pane().filter().is_none());
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.app.pane().filter().is_none());
        assert_eq!(names(&f.app).len(), 8);
    }

    #[test]
    fn what_the_filter_matches_in_a_name_is_bold() {
        let mut f = several();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "tes");
        let drawn = crate::ui::testing::buffer(&mut f.app, 100, 20);
        let files = crate::ui::testing::panes(&f.app, 100).files;
        let (x, y) = crate::ui::testing::find(&drawn, "notes.txt", files).unwrap();
        let bold = |at: u16| {
            drawn[(at, y)]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        };
        assert_eq!(
            (0..5).map(|i| bold(x + i)).collect::<Vec<_>>(),
            [false, false, true, true, true]
        );
    }

    // Find, `f` and Ctrl+F: a walk below the folder, ranked.

    #[test]
    fn f_opens_find_on_the_bottom_row_and_ranks_what_it_finds_as_it_is_typed() {
        for chord in [key(KeyCode::Char('f')), ctrl(KeyCode::Char('f'))] {
            let mut f = searchable();
            f.app.handle_key(chord);
            assert!(typing_a_search(&f.app), "{chord:?}");
            let row = bottom_row(&mut f.app);
            assert!(row.starts_with("Find"), "{row}");
            assert!(row.ends_with("in ~ · tab changes scope"), "{row}");

            type_text(&mut f.app, "TR");
            assert!(f.app.pane().search().unwrap().is_running());
            finish_search(&mut f.app);
            assert_eq!(search_text(&f.app), "TR");
            type_text(&mut f.app, "ip");
            finish_search(&mut f.app);
            assert_eq!(names(&f.app), TRIPS);
            assert_eq!(f.app.pane().cursor, 0);
            assert_eq!(f.app.pane().cwd, f.home, "the folder being searched");
        }
    }

    #[test]
    fn a_typo_shows_only_when_nothing_matches_at_a_word_start_or_better() {
        let mut f = fixture();
        fs::write(f.home.join("lmap.txt"), "").unwrap();
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "lamp");
        assert_eq!(names(&f.app), ["lmap.txt"]);
        fs::create_dir(f.home.join("Documents").join("Lamp")).unwrap();
        press(&mut f.app, &[KeyCode::Backspace]);
        search_for(&mut f.app, "p");
        assert_eq!(names(&f.app), ["Lamp"]);
    }

    #[test]
    fn tab_takes_find_from_this_folder_to_home_to_everywhere_and_back() {
        let mut f = searchable();
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Char('f')]);
        search_for(&mut f.app, "trip");
        assert_eq!(names(&f.app), ["Trip", "day-trip.jpg"]);
        assert!(bottom_row(&mut f.app).ends_with("in ~/Pictures · tab changes scope"));

        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.pane().search().unwrap().scope, Scope::Home);
        finish_search(&mut f.app);
        assert_eq!(names(&f.app), TRIPS);
        assert!(bottom_row(&mut f.app).ends_with("in ~ · tab changes scope"));
        assert!(typing_a_search(&f.app), "Tab stays in the field");

        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.pane().search().unwrap().scope, Scope::Everywhere);
        assert!(bottom_row(&mut f.app).ends_with("everywhere · tab changes scope"));
        // No waiting for all of the disk: Escape stops the walk.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.app.pane().search().unwrap().is_running());

        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.pane().search().unwrap().scope, Scope::Folder);
        finish_search(&mut f.app);
        assert_eq!(names(&f.app), ["Trip", "day-trip.jpg"]);

        // At home, Home is this folder again: Tab goes on to everywhere.
        press(
            &mut f.app,
            &[KeyCode::Esc, KeyCode::Esc, KeyCode::Char('h')],
        );
        press(&mut f.app, &[KeyCode::Char('f'), KeyCode::Tab]);
        assert_eq!(f.app.pane().search().unwrap().scope, Scope::Everywhere);
    }

    #[test]
    fn ctrl_g_finds_in_contents_through_ripgrep_and_says_so_when_it_is_missing() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('f')]);
        f.app.handle_key(ctrl(KeyCode::Char('g')));
        assert!(!f.app.pane().search().unwrap().contents);
        let row = bottom_row(&mut f.app);
        assert!(row.contains("ripgrep is unavailable."), "{row}");
        let drawn = crate::ui::testing::buffer(&mut f.app, 100, 20);
        let (x, y) = crate::ui::testing::find(&drawn, "ripgrep", 0..100).unwrap();
        assert_eq!((y, drawn[(x, y)].fg), (19, crate::theme::ERROR));

        let notes = f.home.join("notes.txt");
        *f.system.grep_finds.borrow_mut() = Some(vec![notes.clone()]);
        f.app.handle_key(ctrl(KeyCode::Char('g')));
        assert!(f.app.pane().search().unwrap().contents);
        assert!(bottom_row(&mut f.app).ends_with("contents in ~ · tab changes scope"));
        search_for(&mut f.app, "hello");
        assert_eq!(names(&f.app), ["notes.txt"]);
        let asked = f.system.grepped.borrow().last().cloned();
        assert_eq!(asked, Some((vec![f.home.clone()], "hello".to_string())));
        // Back to names.
        f.app.handle_key(ctrl(KeyCode::Char('g')));
        finish_search(&mut f.app);
        assert!(names(&f.app).is_empty());
    }

    #[test]
    fn results_rerank_as_they_stream_and_the_cursor_stays_on_its_item() {
        let mut f = searchable();
        *f.system.grep_finds.borrow_mut() = Some(Vec::new());
        let (feed, gate) = std::sync::mpsc::channel();
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "a");
        *f.system.grep_feed.borrow_mut() = Some(gate);
        f.app.handle_key(ctrl(KeyCode::Char('g')));
        let deep = f.home.join("Pictures").join("Trip").join("day-trip.jpg");
        feed.send(deep).unwrap();
        tick_until(&mut f.app, |app| names(app) == ["day-trip.jpg"]);
        assert!(bottom_row(&mut f.app).ends_with("contents in ~ · tab changes scope"));
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(bottom_row(&mut f.app).starts_with("1 found · 1 scanned"));

        // A shorter path ranks first; the cursor stays on what it was on.
        feed.send(f.home.join("notes.txt")).unwrap();
        tick_until(&mut f.app, |app| names(app).len() == 2);
        assert_eq!(names(&f.app), ["notes.txt", "day-trip.jpg"]);
        assert_eq!(selected_name(&f.app), Some("day-trip.jpg"));
        assert!(f.app.pane().search().unwrap().is_running());

        // Escape stops the walk first, keeping what it found; then it leaves.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.app.pane().search().unwrap().is_running());
        assert_eq!(names(&f.app), ["notes.txt", "day-trip.jpg"]);
        let row = bottom_row(&mut f.app);
        assert!(row.starts_with("2 found  Find “a” contents in ~"), "{row}");
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.pane().search().is_none());
        assert_eq!(names(&f.app), FOLDER);
        drop(feed);
    }

    #[test]
    fn enter_keeps_the_results_and_moves_the_keyboard_into_them() {
        let mut f = searchable();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "trip");
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.field.is_none());
        assert_eq!(names(&f.app), TRIPS);
        assert!(f.system.opened.borrow().is_empty(), "Enter opened nothing");
        let row = bottom_row(&mut f.app);
        assert!(row.starts_with("3 found  Find “trip” in ~"), "{row}");

        // Enter on a file opens it; on a folder goes into it.
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Enter]);
        let plan = f.home.join("Documents").join("trip-plan.md");
        assert_eq!(*f.system.opened.borrow(), [plan]);
        assert!(f.app.pane().search().is_some());
        press(&mut f.app, &[KeyCode::Char('k'), KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        assert!(f.app.pane().search().is_none());
        // Back is the folder that was searched, as it was.
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(names(&f.app), FOLDER);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
    }

    #[test]
    fn o_reveals_a_result_in_its_folder_as_a_new_place_in_the_history() {
        let mut f = searchable();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "trip");
        press(
            &mut f.app,
            &[KeyCode::Enter, KeyCode::End, KeyCode::Char('o')],
        );
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        assert_eq!(selected_name(&f.app), Some("day-trip.jpg"));
        assert!(f.app.pane().search().is_none());
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        // Away from results `o` does nothing.
        press(&mut f.app, &[KeyCode::Char('o')]);
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn escape_leaves_find_and_shows_the_folder_with_the_cursor_where_it_was() {
        let mut f = searchable();
        select(&mut f.app, "notes.txt");
        for from_the_results in [false, true] {
            press(&mut f.app, &[KeyCode::Char('f')]);
            search_for(&mut f.app, "trip");
            if from_the_results {
                press(&mut f.app, &[KeyCode::Enter]);
            }
            press(&mut f.app, &[KeyCode::Esc]);
            assert!(f.app.field.is_none() && f.app.pane().search().is_none());
            assert_eq!(names(&f.app), FOLDER);
            assert_eq!(selected_name(&f.app), Some("notes.txt"));
        }
    }

    #[test]
    fn find_says_what_it_found_and_when_it_found_nothing() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "zebra");
        press(&mut f.app, &[KeyCode::Enter]);
        let row = bottom_row(&mut f.app);
        assert!(
            row.starts_with("Nothing matches zebra  Find “zebra” in ~"),
            "{row}"
        );
        let drawn = crate::ui::testing::screen(&mut f.app, 100, 20);
        assert!(
            drawn
                .iter()
                .any(|row| row.contains("NOTHING MATCHES zebra"))
        );
    }

    #[test]
    fn what_find_finds_works_like_any_item_but_nothing_is_pasted_or_made_there() {
        let mut f = searchable();
        let plan = f.home.join("Documents").join("trip-plan.md");
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "trip");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Down]);
        assert_eq!(selected_name(&f.app), Some("trip-plan.md"));

        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert!(f.app.look.is_some(), "Quick Look");
        press(&mut f.app, &[KeyCode::Esc]);
        f.app.handle_key(ctrl_shift('c'));
        assert_eq!(*f.system.copied.borrow(), [plan.display().to_string()]);
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        assert_eq!(said(&f.app), ("Files can't be pasted here.", false));
        f.app.handle_key(ctrl_shift('n'));
        assert_eq!(said(&f.app), ("Folders can't be made here.", false));

        // F2 renames it where it is; the search looks again and finds it.
        press(&mut f.app, &[KeyCode::F(2)]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "trip-notes.md");
        press(&mut f.app, &[KeyCode::Enter]);
        finish_search(&mut f.app);
        assert_eq!(on_disk(&f.home.join("Documents")), ["trip-notes.md"]);
        assert_eq!(names(&f.app), ["Trip", "trip-notes.md", "day-trip.jpg"]);
        assert_eq!(selected_name(&f.app), Some("trip-notes.md"));

        // Delete moves it to the trash, and the cursor to what was next to it.
        select(&mut f.app, "day-trip.jpg");
        press(&mut f.app, &[KeyCode::Delete]);
        finish_work(&mut f.app);
        finish_search(&mut f.app);
        assert_eq!(read(f.trash.join("day-trip.jpg")), "jpg");
        assert_eq!(names(&f.app), ["Trip", "trip-notes.md"]);
        assert_eq!(selected_name(&f.app), Some("trip-notes.md"));

        // `f` hands the keyboard back to the field, as it was left.
        press(&mut f.app, &[KeyCode::Char('f')]);
        assert!(typing_a_search(&f.app));
        assert_eq!(search_text(&f.app), "trip");
    }

    #[test]
    fn while_a_filter_or_find_field_is_open_what_is_typed_goes_into_it() {
        for (open, filter) in [('/', true), ('f', false)] {
            let mut f = searchable();
            press(&mut f.app, &[KeyCode::Char(open)]);
            type_text(&mut f.app, "q s?/1j");
            let text = |app: &App| match filter {
                true => filter_text(app),
                false => search_text(app),
            };
            assert_eq!(text(&f.app), "q s?/1j");
            for other in [
                key(KeyCode::F(2)),
                key(KeyCode::F(5)),
                ctrl(KeyCode::Char('c')),
                ctrl(KeyCode::Char('p')),
                ctrl(KeyCode::Char('2')),
                ctrl(KeyCode::Char('l')),
                ctrl_shift('n'),
                alt(KeyCode::Char('b')),
                key(KeyCode::Up),
            ] {
                f.app.handle_key(other);
                assert_eq!(text(&f.app), "q s?/1j", "{other:?}");
                assert!(f.app.field.is_some(), "{other:?}");
            }
            assert!(!f.app.should_quit && !f.app.help_visible && f.app.dialog.is_none());
            assert_eq!((f.app.sort, f.app.view()), (SortMode::Name, ViewMode::List));
            assert_eq!(f.app.tabs.len(), 1);
            assert!(f.system.previewed.borrow().is_empty());
            assert_eq!(f.app.pane().cwd, f.home);

            // With nothing typed, Enter closes the field and shows the folder.
            press(&mut f.app, &[KeyCode::Backspace; 7]);
            press(&mut f.app, &[KeyCode::Enter]);
            assert!(f.app.field.is_none());
            assert!(f.app.pane().search().is_none() && f.app.pane().filter().is_none());
            assert_eq!(names(&f.app), FOLDER);
        }
    }

    #[test]
    fn escape_leaves_find_before_it_stops_the_file_work_under_way() {
        let mut f = searchable();
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "trip");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Down]);
        f.app.handle_key(key(KeyCode::Char('U')));
        assert_eq!(f.app.working(), Some("Duplicating trip-plan.md…"));
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(
            f.app.pane().search().is_none(),
            "Escape's order: the search first"
        );
        assert_eq!(names(&f.app), FOLDER);
        assert_eq!(f.app.working(), Some("Duplicating trip-plan.md…"));
        press(&mut f.app, &[KeyCode::Esc]);
        finish_work(&mut f.app);
        assert!(f.app.pane().search().is_none());
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
        assert_eq!(f.app.sort.key, SortKey::Size);
        press(&mut f.app, &[KeyCode::Char('?')]);
        assert!(f.app.help_visible);
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Down, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        // In the preview (a click, Tab passing it by), a key with no
        // command there is nothing.
        click_preview(&mut f.app);
        press(&mut f.app, &[KeyCode::Char('e')]);
        assert_eq!(f.app.focus, Focus::Preview);
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
        let entries = app.panes()[pane].entries.iter();
        entries.map(|entry| entry.name.as_str()).collect()
    }

    /// The name of the item a pane's cursor is on.
    fn under_cursor(app: &App, pane: usize) -> Option<&str> {
        app.panes()[pane]
            .selected()
            .map(|entry| entry.name.as_str())
    }

    /// Where the keyboard is: the region, and the pane that is the active one.
    fn keyboard(app: &App) -> (Focus, usize) {
        (app.focus, app.active())
    }

    #[test]
    fn the_app_opens_in_one_pane_in_the_list_whatever_view_was_used_last() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        fs::create_dir(home.join("Pictures")).unwrap();
        let state = tempfile::tempdir().unwrap();

        // A first start, nothing remembered: one pane, the list, on the
        // start folder with the keyboard.
        let mut app = launch(&home, Some(state.path()));
        assert_eq!((app.view(), app.panes().len()), (ViewMode::List, 1));
        assert_eq!(app.pane().cwd, home);
        assert_eq!((app.focus, app.active()), (Focus::List, 0));
        assert!(
            !state.path().join("state").exists(),
            "starting saves nothing"
        );

        // The grid used last, then two panes: the next start is the list.
        app.handle_key(ctrl(KeyCode::Char('3')));
        app.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!(app.view(), ViewMode::Dual);
        assert_eq!(state::load(state.path()).view, ViewMode::Grid);
        let mut again = launch(&home, Some(state.path()));
        assert_eq!((again.view(), again.panes().len()), (ViewMode::List, 1));
        assert_eq!(
            state::load(state.path()).view,
            ViewMode::Grid,
            "kept as it was"
        );
        // Ctrl+4 splits it, and again goes back to the list.
        again.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!((again.view(), again.panes().len()), (ViewMode::Dual, 2));
        again.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!((again.view(), again.panes().len()), (ViewMode::List, 1));
    }

    #[test]
    fn a_bar_opens_a_second_pane_and_closes_it_again() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);

        press(&mut f.app, &[KeyCode::Char('|')]);
        assert_eq!(f.app.panes().len(), 2);
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        assert_eq!(f.app.pane().cwd, pictures);

        // Pressed again it closes the right pane, as Ctrl+W does.
        press(&mut f.app, &[KeyCode::Char('|')]);
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(keyboard(&f.app), (Focus::List, 0));

        // Some terminals send the bar with Shift held, since it takes Shift to type.
        f.app
            .handle_key(KeyEvent::new(KeyCode::Char('|'), KeyModifiers::SHIFT));
        assert_eq!(f.app.panes().len(), 2);

        // In a prompt it is just a character.
        press(&mut f.app, &[KeyCode::Char('|')]);
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        press(&mut f.app, &[KeyCode::Char('|')]);
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(f.app.prompt.as_ref().unwrap().input.text(), "|");
    }

    #[test]
    fn alt_1_splits_and_closes_like_the_bar() {
        // On a Spanish Mac keyboard the bar is Option+1; with Option sent as
        // Alt, the app gets Alt+1 instead.
        let mut f = fixture();
        f.app.handle_key(alt(KeyCode::Char('1')));
        assert_eq!(f.app.panes().len(), 2);
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        f.app.handle_key(alt(KeyCode::Char('1')));
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(keyboard(&f.app), (Focus::List, 0));

        // Ctrl+1 still means the grid, and splits nothing.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        assert_eq!(f.app.view(), ViewMode::Grid);
        assert_eq!(f.app.panes().len(), 1);
    }

    #[test]
    fn ctrl_4_splits_the_window_in_two_and_again_returns_to_the_view() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);

        // The new pane is on the right, in the same folder, with the keyboard.
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!(f.app.panes().len(), 2);
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        assert_eq!(f.app.panes()[0].cwd, pictures);
        assert_eq!(under_cursor(&f.app, 0), Some("a.jpg"));

        // From the left pane too it is the right one that closes, and the
        // view is the one it came from.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        assert_eq!(f.app.view(), ViewMode::Columns);
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(selected_name(&f.app), Some("a.jpg"));
    }

    #[test]
    fn w_closes_the_last_tab_which_quits_and_ctrl_w_q_and_capital_q_too() {
        for chord in [
            key(KeyCode::Char('w')),
            ctrl(KeyCode::Char('w')),
            key(KeyCode::Char('q')),
            shift(KeyCode::Char('Q')),
        ] {
            let mut f = fixture();
            split(&mut f.app);
            f.app.handle_key(chord);
            assert!(f.app.should_quit, "{chord:?}");
        }
    }

    #[test]
    fn each_pane_has_its_own_folder_history_cursor_and_listing() {
        let mut f = searchable();
        let pictures = f.home.join("Pictures");
        press(&mut f.app, &[KeyCode::Down]);
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, pictures.join("Trip"));
        assert_eq!(f.app.panes()[0].cwd, f.home);
        assert_eq!(under_cursor(&f.app, 0), Some("Pictures"));

        // Back in the right pane; the left one has nowhere to go back to.
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        assert_eq!(f.app.pane().cwd, f.home);
        press(&mut f.app, &[KeyCode::End]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(under_cursor(&f.app, 1), Some("Trip"));

        // A search on the left, a folder on the right.
        press(&mut f.app, &[KeyCode::Char('f')]);
        search_for(&mut f.app, "trip");
        assert_eq!(names(&f.app), TRIPS);
        assert_eq!(pane_names(&f.app, 1), ["Trip", "a.jpg"]);
        // Out of the field, then on to the right pane.
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Tab]);
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        assert!(f.app.panes()[0].results().is_some());

        // Up from there is the right pane's business alone.
        press(&mut f.app, &[KeyCode::Backspace]);
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.app.pane().is_folder());
        assert_eq!(pane_names(&f.app, 0), TRIPS);
    }

    #[test]
    fn tab_goes_between_the_two_panes() {
        let mut f = fixture();
        split(&mut f.app);
        let walk = |app: &mut App, code: KeyCode| {
            let stops = (0..3).map(|_| {
                press(app, &[code]);
                keyboard(app)
            });
            stops.collect::<Vec<_>>()
        };
        // Two panes show no preview: the places panel and they are the stops.
        assert_eq!(
            walk(&mut f.app, KeyCode::Tab),
            [(Focus::Places, 1), (Focus::List, 0), (Focus::List, 1)]
        );
        assert_eq!(
            walk(&mut f.app, KeyCode::BackTab),
            [(Focus::List, 0), (Focus::Places, 0), (Focus::List, 1)]
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
        assert_eq!(f.app.panes()[0].cwd, documents);

        // Go to starts from the right pane's folder.
        open_path_field(&mut f.app);
        type_text(&mut f.app, "Trip");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, pictures.join("Trip"));
        assert_eq!(f.app.panes()[0].cwd, documents);
        press(&mut f.app, &[KeyCode::Char('H')]);
        // A new folder is made in its folder.
        f.app.handle_key(ctrl_shift('n'));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(on_disk(&pictures), ["New Folder", "Trip", "a.jpg"]);
        assert!(on_disk(&documents).is_empty());

        // And from the left one's once the keyboard is there.
        press(&mut f.app, &[KeyCode::BackTab]);
        open_path_field(&mut f.app);
        type_text(&mut f.app, "../Pictures/Trip");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.panes()[0].cwd, pictures.join("Trip"));
        press(&mut f.app, &[KeyCode::Char('H')]);
        assert_eq!(f.app.pane().cwd, documents);
        // Ctrl+R reads the one pane again.
        fs::write(documents.join("new.txt"), "").unwrap();
        fs::write(pictures.join("new.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
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
        assert_eq!(f.app.panes()[1].cwd, documents);

        // Copied on the left, pasted on the right.
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(f.app.active(), 1);
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(pane_names(&f.app, 0), FOLDER);
        assert_eq!(said(&f.app), ("Copied 1 item", false));

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
        assert_eq!(f.app.active(), 0);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(pane_names(&f.app, 1).is_empty(), "the right pane saw it go");
        assert_eq!(said(&f.app), ("Moved 1 item.", false));
    }

    #[test]
    fn after_a_change_both_panes_list_their_folder_again_and_keep_their_place() {
        let mut f = several();
        select(&mut f.app, "c.txt");
        split(&mut f.app);
        f.app.tab_mut().panes[0].offset = 2;

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
        assert_eq!(f.app.panes()[0].offset, 2, "scrolled where it was");

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
        let listed = &f.app.panes()[0].entries[1];
        assert_eq!(listed.name, "Documents");
        assert_eq!(listed.item_count(), Some(1));
        assert_eq!(under_cursor(&f.app, 0), Some("d.txt"));

        // Undone from the left, the copy leaves the right pane's list.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert!(pane_names(&f.app, 1).is_empty());
        assert_eq!(f.app.panes()[0].entries[1].item_count(), Some(0));
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
        f.app.handle_key(key(KeyCode::Char('.')));
        assert_eq!(
            pane_names(&f.app, 0),
            ["Trip", ".secret", "a.jpg", "big.bin"]
        );
        assert_eq!(names(&f.app)[2..], [".hidden", "notes.txt"]);
        f.app.handle_key(key(KeyCode::Char('.')));
        assert_eq!(pane_names(&f.app, 0), ["Trip", "a.jpg", "big.bin"]);
        assert_eq!(names(&f.app), FOLDER);

        press(&mut f.app, &[KeyCode::Char('s'), KeyCode::Char('S')]);
        assert_eq!(f.app.sort, BY_SIZE);
        assert_eq!(pane_names(&f.app, 0), ["Trip", "big.bin", "a.jpg"]);
        assert_eq!(under_cursor(&f.app, 0), Some("Trip"));

        // Ctrl+2 leaves the two panes for the columns of the left one,
        // which lists the folders above it.
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(f.app.pane().cwd, pictures);
        assert_eq!(f.app.panes()[0].ancestors.len(), 1);
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        assert_eq!(f.app.panes()[0].ancestors.len(), 0);
    }

    #[test]
    fn ctrl_shift_n_and_ctrl_n_do_what_they_did_and_split_nothing() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('n')));
        assert!(f.app.dialog.is_none());
        let held = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        for chord in [ctrl_shift('n'), KeyEvent::new(KeyCode::Char('n'), held)] {
            f.app.handle_key(chord);
            assert!(matches!(asking(&f.app), Some(Asking::NewFolder(_))));
            press(&mut f.app, &[KeyCode::Esc]);
        }
        // The new chords' letters alone, or with Alt alone, are only letters.
        type_text(&mut f.app, "nwa");
        f.app.handle_key(alt(KeyCode::Char('n')));
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(chosen(&f.app), ["Documents"]);
        assert_eq!((f.app.working(), &f.app.message), (None, &None));
        assert_eq!(on_disk(&f.home), FOLDER);
    }

    #[test]
    fn shift_arrows_and_capital_j_and_k_mark_the_way_and_keep_earlier_marks() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(selected_name(&f.app), Some("c.txt"));

        // Back over what is marked, nothing is let go.
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(selected_name(&f.app), Some("b.txt"));
        press(&mut f.app, &[KeyCode::Char('K'), KeyCode::Char('K')]);
        assert_eq!(chosen(&f.app), ["Pictures", "a.txt", "b.txt", "c.txt"]);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        // J, also as the small letter's capital with Shift held.
        f.app.handle_key(shift(KeyCode::Char('J')));
        press(
            &mut f.app,
            &[KeyCode::Char('j'), KeyCode::Char('j'), KeyCode::Char('j')],
        );
        press(&mut f.app, &[KeyCode::Char('J')]);
        assert_eq!(
            chosen(&f.app),
            ["Pictures", "a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]
        );
        // Shift with Home and End only moves the cursor; the marks stay.
        f.app.handle_key(shift(KeyCode::End));
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(chosen(&f.app).len(), 6);
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn v_marks_the_item_under_the_cursor_without_moving_and_capital_v_inverts() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        press(&mut f.app, &[KeyCode::Char('v')]);
        assert_eq!(selected_name(&f.app), Some("a.txt"), "the cursor stays");
        assert_eq!(chosen(&f.app), ["a.txt"]);
        // Moving keeps the marks: what is selected is them, not the cursor's item.
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char('j')]);
        assert_eq!(chosen(&f.app), ["a.txt"]);
        press(&mut f.app, &[KeyCode::Char('v')]);
        assert_eq!(chosen(&f.app), ["a.txt", "c.txt"]);
        // V inverts the marks among what is shown.
        press(&mut f.app, &[KeyCode::Char('V')]);
        assert_eq!(
            chosen(&f.app),
            [
                "Documents",
                "Pictures",
                "b.txt",
                "d.txt",
                "e.txt",
                "notes.txt"
            ]
        );
        press(&mut f.app, &[KeyCode::Char('V')]);
        assert_eq!(chosen(&f.app), ["a.txt", "c.txt"]);
        // v again unmarks; with none marked, the item under the cursor is it.
        press(&mut f.app, &[KeyCode::Char('v'), KeyCode::Char('k')]);
        press(&mut f.app, &[KeyCode::Char('k'), KeyCode::Char('v')]);
        assert_eq!(chosen(&f.app), ["a.txt"]);
        assert!(!f.app.pane().is_marked(2));
        press(&mut f.app, &[KeyCode::Char('v')]);
        assert!(f.app.pane().is_marked(2));
        assert_eq!(chosen(&f.app), ["a.txt"]);
        press(&mut f.app, &[KeyCode::Char('v')]);
        // Drawn marked, and the footer counts them.
        press(&mut f.app, &[KeyCode::Char('v'), KeyCode::Char('G')]);
        let drawn = crate::ui::testing::buffer(&mut f.app, 120, 30);
        let files = crate::ui::testing::panes(&f.app, 120).files;
        let marked = crate::ui::testing::find(&drawn, "a.txt", files).unwrap();
        assert_eq!(drawn[marked].bg, theme::MARKED);
        assert!(crate::ui::testing::find(&drawn, "1 of 8 selected", 0..120).is_some());
    }

    #[test]
    fn in_the_grid_shift_and_the_arrows_mark_by_a_tile_or_a_row_keeping_the_marks() {
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
        f.app.handle_key(shift(KeyCode::Up));
        assert_eq!(chosen(&f.app).len(), 5);
        assert_eq!(selected_name(&f.app), Some("Pictures"));
        f.app.handle_key(shift(KeyCode::Left));
        assert_eq!(
            chosen(&f.app),
            ["Documents", "Pictures", "a.txt", "b.txt", "c.txt", "d.txt"]
        );
        assert_eq!(selected_name(&f.app), Some("Documents"));
        assert_eq!(f.app.pane().cwd, f.home, "no folder was entered or left");
    }

    #[test]
    fn ctrl_a_marks_everything_in_the_pane_and_leaves_the_cursor() {
        let mut f = several();
        select(&mut f.app, "b.txt");
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(chosen(&f.app).len(), 8);
        assert_eq!(chosen(&f.app), pane_names(&f.app, 0));
        assert_eq!(selected_name(&f.app), Some("b.txt"));
        // In an empty folder there is nothing to take.
        press(&mut f.app, &[KeyCode::Home, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert!(chosen(&f.app).is_empty());
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(f.app.message, None);
    }

    #[test]
    fn escape_dismisses_an_error_first_then_clears_the_marks() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        f.app.handle_key(shift(KeyCode::Down));
        f.app.handle_key(shift(KeyCode::Down));
        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(
            chosen(&f.app),
            ["a.txt", "b.txt", "c.txt"],
            "moving keeps them"
        );
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(chosen(&f.app), ["d.txt"]);
        assert_eq!(selected_name(&f.app), Some("d.txt"));

        // A notice stays; the marks go.
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("8 items ready to paste.", false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(chosen(&f.app), ["d.txt"]);
        // An error goes before the marks do.
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        f.app.navigate(&f.home.join("missing"));
        assert!(f.app.message.as_ref().is_some_and(|m| m.is_error));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.message, None);
        assert_eq!(chosen(&f.app).len(), 8);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(chosen(&f.app), ["d.txt"]);

        // Another folder starts with nothing marked, and so does the way back.
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
        assert_eq!(said(&f.app), ("Copied 3 items", false));

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
        assert_eq!(said(&f.app), ("Moved 3 items to Trash", false));
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
        // Space and Enter are for the one under the cursor.
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert!(f.app.look.is_some());
        assert_eq!(f.app.selected().map(|entry| &entry.path), Some(&b));
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), std::slice::from_ref(&b));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);

        f.app.handle_key(key(KeyCode::Char('U')));
        assert_eq!(f.app.working(), Some("Duplicating 2 items…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("a copy.txt")), "a");
        assert_eq!(read(f.home.join("b copy.txt")), "b");
        assert_eq!(on_disk(&f.home).len(), 10);
        assert_eq!(said(&f.app), ("Duplicated 2 items.", false));

        // With one item selected F2 asks for its new name, as before.
        select(&mut f.app, "c.txt");
        press(&mut f.app, &[KeyCode::F(2)]);
        let Some(Asking::Rename(path)) = asking(&f.app) else {
            panic!("it didn't ask");
        };
        assert_eq!(path, f.home.join("c.txt"));
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
        // Round the places panel and the two panes, and on to the left one.
        press(&mut f.app, &[KeyCode::Tab; 5]);
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
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

        // Delete takes the one item; Escape leaves what was said, which is no error.
        select(&mut f.app, "a.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(on_disk(&f.trash), ["a.txt"]);
        assert_eq!(on_disk(&f.home).len(), 7);
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));
        assert_eq!(chosen(&f.app), ["b.txt"]);
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        assert_eq!(said(&f.app), ("1 item ready to paste.", false));

        // Tab goes to the preview, the places panel and back, and the one
        // pane stays the active one.
        let mut stops = Vec::new();
        for _ in 0..3 {
            press(&mut f.app, &[KeyCode::Tab]);
            stops.push(keyboard(&f.app));
        }
        assert_eq!(
            stops,
            [(Focus::Preview, 0), (Focus::Places, 0), (Focus::List, 0)]
        );
        press(&mut f.app, &[KeyCode::F(2)]);
        assert!(matches!(asking(&f.app), Some(Asking::Rename(_))));
    }

    // The menu. The programs that zip and the terminal are fakes, the trash
    // is the fixture's own folder, and so is where its records are kept.

    /// The lines of the menu that is open: the labels, and a dash for each
    /// rule between two groups.
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

    /// Which menu is showing: the root one or a submenu.
    fn sub(app: &App) -> Sub {
        app.menu.as_ref().expect("no menu is open").sub
    }

    #[test]
    fn a_menu_open_when_file_work_finishes_still_acts_on_what_it_was_opened_for() {
        let mut f = fixture();
        // Duplicate notes.txt on the worker, then open the menu on Documents
        // before the work is taken in.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('U')]);
        assert!(f.app.working().is_some());
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Char('m')]);
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes copy.txt")), "hello");
        // The duplicate landing must not move the menu onto it.
        assert_eq!(
            f.app.selected().map(|entry| entry.name.as_str()),
            Some("Pictures")
        );
        let lines = menu(&f.app);
        let row = lines
            .iter()
            .position(|line| *line == "Move to Trash")
            .unwrap();
        while menu_cursor(&f.app) != row {
            press(&mut f.app, &[KeyCode::Down]);
        }
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(on_disk(&f.trash), ["Pictures"]);
        assert!(f.home.join("notes copy.txt").exists());
    }

    #[test]
    fn a_menu_whose_item_is_gone_when_work_finishes_closes_and_says_so() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('U')]);
        press(&mut f.app, &[KeyCode::Char('m')]);
        // The item the menu is for disappears behind the app's back.
        fs::remove_file(f.home.join("notes.txt")).unwrap();
        finish_work(&mut f.app);
        assert!(f.app.menu.is_none(), "the menu closes");
        assert!(on_disk(&f.trash).is_empty());
        assert_eq!(
            said(&f.app).0,
            "What the menu was for changed, so it closed."
        );
    }

    #[test]
    fn the_menu_pins_and_unpins_the_one_folder_under_the_cursor() {
        let mut f = fixture();
        fs::create_dir(f.home.join("Work")).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        select(&mut f.app, "Work");
        f.app.handle_key(key(KeyCode::Char('m')));
        let lines = menu(&f.app);
        let copy = lines.iter().position(|line| *line == "Copy Path").unwrap();
        assert_eq!(lines[copy + 1], "Pin");
        press(&mut f.app, &[KeyCode::Esc]);
        pick(&mut f.app, "Pin");
        assert_eq!(
            f.app.message.as_ref().map(|m| m.text.as_str()),
            Some("Pinned Work")
        );
        assert_eq!(f.app.pins, [f.home.join("Work")]);
        // Pinned, the line says Unpin.
        pick(&mut f.app, "Unpin");
        assert!(f.app.pins.is_empty());
        // Not for a file, nor for several.
        select(&mut f.app, "notes.txt");
        f.app.handle_key(key(KeyCode::Char('m')));
        assert!(!menu(&f.app).contains(&"Pin"));
    }

    /// `m`, down to the line with this label, and Enter.
    fn pick(app: &mut App, label: &str) {
        pick_in(app, &[label]);
    }

    /// `m`, then down to each label in turn and Enter on it: a line that
    /// opens a submenu, then a line of that submenu.
    fn pick_in(app: &mut App, labels: &[&str]) {
        app.handle_key(key(KeyCode::Char('m')));
        for label in labels {
            let lines = menu(app);
            let row = lines.iter().position(|line| line == label);
            let row = row.unwrap_or_else(|| panic!("{label} isn't in {lines:?}"));
            while menu_cursor(app) != row {
                let before = menu_cursor(app);
                press(app, &[KeyCode::Down]);
                assert_ne!(menu_cursor(app), before, "{label} can't be reached");
            }
            press(app, &[KeyCode::Enter]);
        }
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

    /// The menu for one file or folder that is not an archive.
    const FILE_MENU: [&str; 17] = [
        "Open",
        "Open With",
        "Open in Terminal",
        "-",
        "Compress",
        "-",
        "Tags",
        "Copy Path",
        "-",
        "Rename",
        "Duplicate",
        "-",
        "Move to Trash",
        "Delete Permanently…",
        "-",
        "Show Hidden Files",
        "Open Trash",
    ];

    #[test]
    fn m_menu_and_shift_f10_open_the_menu_with_its_rows_in_groups() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(menu(&f.app), FILE_MENU);
        assert!(dim(&f.app).is_empty());
        assert_eq!((sub(&f.app), menu_cursor(&f.app)), (Sub::Root, 0));
        assert_eq!(Sub::Root.title(), None);
        // Escape closes it with nothing done.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.menu.is_none());
        assert!(f.system.opened.borrow().is_empty());
        assert_eq!(selected_name(&f.app), Some("notes.txt"));

        // A folder is offered the same.
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Menu]);
        assert_eq!(menu(&f.app), FILE_MENU);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.pane().cwd, f.home, "it was not opened");

        // An archive can be extracted: Extract sits beside Compress.
        fs::write(f.home.join("pack.zip"), "notes.txt").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        select(&mut f.app, "pack.zip");
        f.app.handle_key(shift(KeyCode::F(10)));
        let archive = [
            "Open",
            "Open With",
            "Open in Terminal",
            "-",
            "Compress",
            "Extract",
            "-",
        ];
        assert_eq!(&menu(&f.app)[..7], archive);
        assert_eq!(menu(&f.app).len(), FILE_MENU.len() + 1);
        press(&mut f.app, &[KeyCode::Esc]);

        // With hidden files showing, it offers to hide them.
        press(&mut f.app, &[KeyCode::Char('.')]);
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(menu(&f.app).last(), Some(&"Open Trash"));
        assert!(menu(&f.app).contains(&"Hide Hidden Files"));
        assert!(!menu(&f.app).contains(&"Show Hidden Files"));

        // From the preview pane it is the same item's menu.
        press(&mut f.app, &[KeyCode::Esc]);
        click_preview(&mut f.app);
        f.app.handle_key(key(KeyCode::Char('m')));
        assert!(f.app.menu.is_some());
    }

    #[test]
    fn with_several_marked_open_with_is_dim_and_rename_renames_them_all() {
        let mut f = several();
        select(&mut f.app, "a.txt");
        press(&mut f.app, &[KeyCode::Char('J')]);
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(menu(&f.app), FILE_MENU);
        assert_eq!(dim(&f.app), ["Open With"]);
        // A dim line with a `›` opens nothing, and Enter on it does nothing.
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Char('l'), KeyCode::Enter],
        );
        assert_eq!((sub(&f.app), menu_cursor(&f.app)), (Sub::Root, 1));
        press(&mut f.app, &[KeyCode::Esc]);

        // Rename runs what `r` runs with several marked: all of them in
        // the editor, here one that changes nothing.
        pick(&mut f.app, "Rename");
        assert_eq!(f.system.edited.borrow().len(), 1);
        assert_eq!(said(&f.app), ("0 renamed · 2 unchanged", false));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);

        // What Find found in two folders has no one folder for an archive.
        fs::write(f.home.join("Pictures").join("b.txt"), "b").unwrap();
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "b.txt");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        assert_eq!(chosen(&f.app).len(), 2);
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(dim(&f.app), ["Open With", "Compress"]);
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Enter],
        );
        assert_eq!(sub(&f.app), Sub::Root, "a dim Compress opens nothing");
    }

    #[test]
    fn an_empty_folder_and_the_trash_offer_only_what_applies_there() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(names(&f.app).is_empty());
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(
            menu(&f.app),
            ["Open in Terminal", "-", "Show Hidden Files", "Open Trash"]
        );
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Char('h')]);

        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        pick(&mut f.app, "Open Trash");
        assert_eq!(f.app.pane().cwd, f.trash);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        f.app.handle_key(key(KeyCode::Char('m')));
        let trash = [
            "Restore",
            "Delete Permanently…",
            "-",
            "Empty Trash…",
            "-",
            "Show Hidden Files",
        ];
        assert_eq!(menu(&f.app), trash);
        press(&mut f.app, &[KeyCode::Esc]);
        pick(&mut f.app, "Restore");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert!(names(&f.app).is_empty());

        // An empty trash: emptying it, and the hidden files.
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(menu(&f.app), ["Empty Trash…", "-", "Show Hidden Files"]);
    }

    #[test]
    fn submenus_open_with_l_right_or_enter_and_close_with_h_or_left() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(key(KeyCode::Char('m')));
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char('l')]);
        assert_eq!(sub(&f.app), Sub::OpenWith);
        assert_eq!(Sub::OpenWith.title(), Some("OPEN WITH"));
        assert_eq!(menu(&f.app), ["Default app", "Quick Look"]);
        press(&mut f.app, &[KeyCode::Char('h')]);
        assert_eq!((sub(&f.app), menu_cursor(&f.app)), (Sub::Root, 1));

        press(
            &mut f.app,
            &[KeyCode::Char('j'), KeyCode::Char('j'), KeyCode::Right],
        );
        assert_eq!(sub(&f.app), Sub::Compress);
        assert_eq!(Sub::Compress.title(), Some("COMPRESS"));
        let formats = [".zip", ".tar", ".tar.gz", ".tar.bz2", ".tar.xz"];
        assert_eq!(menu(&f.app), formats);
        press(&mut f.app, &[KeyCode::Left]);
        assert_eq!((sub(&f.app), menu_cursor(&f.app)), (Sub::Root, 4));

        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Enter]);
        assert_eq!(sub(&f.app), Sub::Tags);
        assert_eq!(Sub::Tags.title(), Some("TAGS"));
        assert_eq!(menu(&f.app), SEVEN);
        // Escape closes them all, with nothing done.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.menu.is_none());
        assert!(f.app.message.is_none() && f.app.working().is_none());

        // With 7-Zip installed Compress offers it last.
        f.system.seven_zip.set(true);
        pick(&mut f.app, "Compress");
        assert_eq!(menu(&f.app).last(), Some(&".7z"));
        // `h` on the menu itself leaves it open; on `T`'s own menu of
        // tags it closes it.
        press(
            &mut f.app,
            &[KeyCode::Esc, KeyCode::Char('m'), KeyCode::Char('h')],
        );
        assert_eq!(sub(&f.app), Sub::Root);
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Char('T')]);
        assert_eq!(sub(&f.app), Sub::Tags);
        press(&mut f.app, &[KeyCode::Char('h')]);
        assert!(f.app.menu.is_none());
    }

    #[test]
    fn the_menu_keeps_every_key_to_itself_and_j_k_and_the_arrows_skip_the_rules() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl_shift('c'));
        assert!(f.app.message.is_some());
        f.app.handle_key(key(KeyCode::Char('m')));

        let mut stops = Vec::new();
        for code in [
            KeyCode::Down,
            KeyCode::Char('j'),
            KeyCode::Char('j'),
            KeyCode::Up,
            KeyCode::Char('k'),
            KeyCode::Char('k'),
            KeyCode::Up,
        ] {
            press(&mut f.app, &[code]);
            stops.push(menu_cursor(&f.app));
        }
        assert_eq!(
            stops,
            [1, 2, 4, 2, 1, 0, 0],
            "over the rule, and no further"
        );
        press(&mut f.app, &[KeyCode::Down; 30]);
        assert_eq!(menu_cursor(&f.app), FILE_MENU.len() - 1);

        // No other key reaches what is underneath.
        for other in [
            key(KeyCode::Char('q')),
            key(KeyCode::Char('?')),
            key(KeyCode::Char('/')),
            key(KeyCode::Char(' ')),
            key(KeyCode::Char('d')),
            key(KeyCode::Delete),
            key(KeyCode::F(2)),
            key(KeyCode::Tab),
            key(KeyCode::Backspace),
            key(KeyCode::Home),
            ctrl(KeyCode::Char('a')),
            ctrl(KeyCode::Char('l')),
            ctrl_shift('n'),
            key(KeyCode::Char('H')),
        ] {
            f.app.handle_key(other);
            assert_eq!(menu_cursor(&f.app), FILE_MENU.len() - 1, "{other:?}");
        }
        assert!(!f.app.should_quit && !f.app.help_visible && f.app.look.is_none());
        assert!(f.app.dialog.is_none() && f.app.field.is_none());
        assert_eq!(f.app.focus, Focus::List);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert!(f.system.previewed.borrow().is_empty());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);

        // Escape closes the menu, and leaves what was said.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.menu.is_none());
        assert_eq!(said(&f.app), ("Copied the path.", false));
    }

    #[test]
    fn each_menu_row_runs_the_code_its_key_runs() {
        let mut f = fixture();
        let notes = f.home.join("notes.txt");
        let only_notes = std::slice::from_ref(&notes);
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Open");
        assert!(f.app.menu.is_none(), "chosen, it closes");
        assert_eq!(f.system.opened.borrow().as_slice(), only_notes);
        pick_in(&mut f.app, &["Open With", "Default app"]);
        assert_eq!(*f.system.opened.borrow(), [notes.clone(), notes.clone()]);
        // macOS's own Quick Look window, not the card Space opens.
        pick_in(&mut f.app, &["Open With", "Quick Look"]);
        assert_eq!(f.system.previewed.borrow().as_slice(), only_notes);
        assert!(f.app.look.is_none() && f.app.menu.is_none());

        pick(&mut f.app, "Copy Path");
        assert_eq!(*f.system.copied.borrow(), [notes.display().to_string()]);
        assert_eq!(said(&f.app), ("Copied the path.", false));

        pick(&mut f.app, "Rename");
        assert_eq!(field(&f.app), ("notes.txt".to_string(), 5));
        press(&mut f.app, &[KeyCode::Esc]);

        pick(&mut f.app, "Duplicate");
        assert_eq!(f.app.working(), Some("Duplicating notes.txt…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes copy.txt")), "hello");

        select(&mut f.app, "notes copy.txt");
        pick(&mut f.app, "Move to Trash");
        assert_eq!(on_disk(&f.trash), ["notes copy.txt"]);
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);

        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Delete Permanently…");
        let Some(Dialog::Delete { paths, armed }) = &f.app.dialog else {
            panic!("it didn't ask");
        };
        assert_eq!((paths.as_slice(), *armed), (only_notes, false));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(read(notes.clone()), "hello");

        pick_in(&mut f.app, &["Tags", "Red"]);
        assert_eq!(said(&f.app), ("Tagged 1 item Red", false));
        assert!(f.app.menu.is_none());

        pick_in(&mut f.app, &["Compress", ".zip"]);
        assert_eq!(f.app.working(), Some("Compressing notes.txt to .zip"));
        finish_work(&mut f.app);
        select(&mut f.app, "notes.zip");
        pick(&mut f.app, "Extract");
        assert_eq!(f.app.working(), Some("Extracting notes.zip"));
        finish_work(&mut f.app);
        assert_eq!(
            read(f.home.join("notes").join("notes.txt")),
            "from notes.zip"
        );

        pick(&mut f.app, "Show Hidden Files");
        assert!(f.app.show_hidden);
        pick(&mut f.app, "Hide Hidden Files");
        assert!(!f.app.show_hidden);

        select(&mut f.app, "Pictures");
        pick(&mut f.app, "Open");
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        pick(&mut f.app, "Open Trash");
        assert_eq!(f.app.pane().cwd, f.trash);
        assert_eq!(f.app.pane().back.len(), 2, "in the history");
    }

    #[test]
    fn ctrl_opt_1_to_4_switch_views_where_a_window_manager_takes_ctrl_1_to_4() {
        let mut f = fixture();
        let digit = |c| ctrl_alt(KeyCode::Char(c));
        f.app.handle_key(digit('2'));
        assert_eq!(f.app.view(), ViewMode::Columns);
        f.app.handle_key(digit('3'));
        assert_eq!(f.app.view(), ViewMode::Grid);
        f.app.handle_key(digit('1'));
        assert_eq!(f.app.view(), ViewMode::List);
        f.app.handle_key(digit('4'));
        assert_eq!(f.app.panes().len(), 2);
        f.app.handle_key(digit('4'));
        assert_eq!(f.app.panes().len(), 1);
        // From the places panel too, like the window's other keys.
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.app.focus, Focus::Places);
        f.app.handle_key(digit('3'));
        assert_eq!(f.app.view(), ViewMode::Grid);
        // Ctrl+1–4 still do the same.
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        assert_eq!(f.app.view(), ViewMode::Columns);
        // And Alt+1 is still the bar of a Spanish keyboard.
        f.app.handle_key(alt(KeyCode::Char('1')));
        assert_eq!(f.app.panes().len(), 2);
    }

    #[test]
    fn the_menu_s_open_in_terminal_starts_one_where_alt_t_does() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        // On a folder, in it; on a file, in its folder.
        select(&mut f.app, "Pictures");
        pick(&mut f.app, "Open in Terminal");
        assert!(f.app.menu.is_none(), "the menu closes");
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Open in Terminal");
        assert_eq!(
            *f.system.terminals.borrow(),
            [pictures.clone(), f.home.clone()]
        );
        // In an empty folder, in the folder itself.
        let empty = f.home.join("Documents");
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(names(&f.app).is_empty());
        pick(&mut f.app, "Open in Terminal");
        assert_eq!(f.system.terminals.borrow().last(), Some(&empty));
        // No terminal: it says so, as Alt+T does.
        f.system.no_terminal.set(true);
        pick(&mut f.app, "Open in Terminal");
        assert_eq!(said(&f.app), ("Couldn't start a terminal.", true));
    }

    #[test]
    fn alt_t_starts_a_terminal_like_ctrl_t() {
        let mut f = fixture();
        select(&mut f.app, "Pictures");
        f.app.handle_key(alt(KeyCode::Char('t')));
        f.app.handle_key(ctrl(KeyCode::Char('t')));
        let pictures = f.home.join("Pictures");
        assert_eq!(*f.system.terminals.borrow(), [pictures.clone(), pictures]);
        // A plain `t` is still a new tab.
        assert_eq!(f.app.tabs.len(), 1);
    }

    #[test]
    fn ctrl_t_starts_a_terminal_in_the_folder_or_in_the_files_folder() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        select(&mut f.app, "Pictures");
        f.app.handle_key(ctrl(KeyCode::Char('t')));
        select(&mut f.app, "notes.txt");
        f.app.handle_key(ctrl(KeyCode::Char('t')));
        // With nothing under the cursor it is the folder that is showing.
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('t')));
        assert_eq!(
            *f.system.terminals.borrow(),
            [pictures, f.home.clone(), f.home.join("Documents")]
        );
        assert_eq!(f.app.message, None);

        f.system.no_terminal.set(true);
        f.app.handle_key(ctrl(KeyCode::Char('t')));
        assert_eq!(said(&f.app), ("Couldn't start a terminal.", true));
        assert_eq!(f.system.terminals.borrow().len(), 3);
    }

    #[test]
    fn compress_makes_a_zip_beside_the_item_and_undo_moves_it_to_the_trash() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        compress_as(&mut f.app, "zip");
        assert_eq!(f.app.working(), Some("Compressing notes.txt to .zip"));
        finish_work(&mut f.app);
        assert_eq!(
            all_on_disk(&f.home),
            [".hidden", "Documents", "Pictures", "notes.txt", "notes.zip"],
            "the archive, and nothing left over from making it"
        );
        assert_eq!(read(f.home.join("notes.zip")), "notes.txt");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(said(&f.app), ("Compressed into notes.zip.", false));
        // The folder was listed again, the cursor on the archive.
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", "notes.txt", "notes.zip"]
        );
        assert_eq!(selected_name(&f.app), Some("notes.zip"));

        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(read(f.trash.join("notes.zip")), "notes.txt");
        let undid = "Undid the compress";
        assert_eq!(said(&f.app), (undid, false));
        // There it is known like anything else this app moved to the trash.
        let known = f.app.trashed(&f.trash.join("notes.zip"));
        assert_eq!(
            known.map(|record| &record.from),
            Some(&f.home.join("notes.zip"))
        );
        f.app.handle_key(ctrl_shift('z'));
        assert_eq!(read(f.home.join("notes.zip")), "notes.txt");
        assert!(on_disk(&f.trash).is_empty());
        let redid = "Redid compressing into notes.zip.";
        assert_eq!(said(&f.app), (redid, false));
    }

    #[test]
    fn compressing_several_items_names_the_archive_after_their_folder_and_keeps_both() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        f.app.navigate(&pictures);
        f.app.handle_key(shift(KeyCode::Down));
        assert_eq!(chosen(&f.app), ["Trip", "a.jpg"]);
        compress_as(&mut f.app, "zip");
        assert_eq!(f.app.working(), Some("Compressing 2 items to .zip"));
        finish_work(&mut f.app);
        assert_eq!(read(pictures.join("Pictures.zip")), "Trip\na.jpg");
        assert_eq!(selected_name(&f.app), Some("Pictures.zip"));
        assert_eq!(said(&f.app), ("Compressed into Pictures.zip.", false));

        // Again: the name is taken, and what has it is not written over.
        fs::write(pictures.join("Pictures.zip"), "the first one").unwrap();
        press(&mut f.app, &[KeyCode::Home]);
        f.app.handle_key(shift(KeyCode::Down));
        compress_as(&mut f.app, "zip");
        assert_eq!(f.app.working(), Some("Compressing 2 items to .zip"));
        finish_work(&mut f.app);
        assert_eq!(read(pictures.join("Pictures.zip")), "the first one");
        assert_eq!(read(pictures.join("Pictures copy.zip")), "Trip\na.jpg");
        assert_eq!(selected_name(&f.app), Some("Pictures copy.zip"));

        // The same for one item.
        f.app.navigate(&f.home.clone());
        fs::write(f.home.join("notes.zip"), "someone else's").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        select(&mut f.app, "notes.txt");
        compress_as(&mut f.app, "zip");
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes.zip")), "someone else's");
        assert_eq!(read(f.home.join("notes copy.zip")), "notes.txt");
        assert_eq!(
            all_on_disk(&f.home),
            [
                ".hidden",
                "Documents",
                "Pictures",
                "notes copy.zip",
                "notes.txt",
                "notes.zip"
            ]
        );
        assert_eq!(
            all_on_disk(&pictures),
            ["Pictures copy.zip", "Pictures.zip", "Trip", "a.jpg"]
        );
    }

    #[test]
    fn a_compress_that_is_stopped_or_fails_leaves_nothing_behind() {
        let mut f = fixture();
        let untouched = [".hidden", "Documents", "Pictures", "notes.txt"];
        f.system.archiving.set(Archiving::Hangs);
        select(&mut f.app, "notes.txt");
        compress_as(&mut f.app, "zip");
        assert_eq!(f.app.working(), Some("Compressing notes.txt to .zip"));
        // The window isn't frozen meanwhile, and other changes wait.
        f.app.tick();
        press(&mut f.app, &[KeyCode::Delete]);
        let wait = ("Try again once the work under way is done.", false);
        assert_eq!(said(&f.app), wait);
        assert_eq!(f.app.working(), Some("Compressing notes.txt to .zip"));

        press(&mut f.app, &[KeyCode::Esc]);
        finish_work(&mut f.app);
        assert_eq!(said(&f.app), ("Stopped compressing.", false));
        assert_eq!(all_on_disk(&f.home), untouched);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));

        f.system.archiving.set(Archiving::Fails);
        compress_as(&mut f.app, "zip");
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
        app.tab_mut().view = ViewMode::List;
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

    const TRASH_MENU: [&str; 6] = [
        "Restore",
        "Delete Permanently…",
        "-",
        "Empty Trash…",
        "-",
        "Show Hidden Files",
    ];

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

        open_trash(&mut f.app);
        assert_eq!(names(&f.app), ["a.jpg", "notes.txt"]);
        f.app.handle_key(key(KeyCode::Char('m')));
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
        assert_eq!(said(&f.app), ("Undid the restore", false));
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

        open_trash(&mut f.app);
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

        open_trash(&mut app);
        pick(&mut app, "Empty Trash…");
        let Some(Dialog::EmptyTrash { paths, armed, .. }) = &app.dialog else {
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
    fn emptying_the_trash_says_one_item_for_one() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        open_trash(&mut f.app);
        pick(&mut f.app, "Empty Trash…");
        assert!(matches!(f.app.dialog, Some(Dialog::EmptyTrash { .. })));
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Enter]);
        assert_eq!(said(&f.app), ("Emptied the trash: 1 item deleted.", false));
        assert!(on_disk(&f.trash).is_empty());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
        assert_eq!(f.app.pane().cwd, f.trash);
        assert!(names(&f.app).is_empty());
    }

    #[test]
    fn an_empty_trash_and_one_that_cannot_be_listed_are_not_asked_about() {
        let mut f = fixture();
        let already = ("The trash is already empty.", false);
        // One nothing was ever moved to.
        open_trash(&mut f.app);
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
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(menu(&f.app), ["Empty Trash…", "-", "Show Hidden Files"]);
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(said(&f.app), already);
        assert!(f.trash.join(".DS_Store").exists());

        // One macOS keeps the terminal out of: nothing is deleted, and it
        // says how to allow it.
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
    fn shift_f10_in_the_search_field_opens_the_menu_for_what_was_found() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "notes");
        finish_search(&mut f.app);
        assert!(typing_a_search(&f.app));
        f.app.handle_key(shift(KeyCode::F(10)));
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
        // Enter opens, and H and L walk the history.
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, pictures);
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home);
        f.app.handle_key(key(KeyCode::Char('L')));
        assert_eq!(f.app.pane().cwd, pictures);
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), [f.home.join("notes.txt")]);
        assert!(f.app.menu.is_none());

        // Ctrl+V pastes into the folder that is showing, whatever is under
        // the cursor.
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        f.app.navigate(&pictures);
        assert_eq!(selected_name(&f.app), Some("Trip"));
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&pictures), ["Trip", "a.jpg", "notes.txt"]);
        assert!(on_disk(&pictures.join("Trip")).is_empty());
        f.app.navigate(&documents);
        assert_eq!(
            (f.app.pane().cwd.clone(), f.app.focus),
            (documents, Focus::List)
        );

        // Delete moves to the trash, and asks in the trash itself.
        f.app.navigate(&pictures);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Delete]);
        assert_eq!(on_disk(&f.trash), ["notes.txt"]);
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));
        open_trash(&mut f.app);
        press(&mut f.app, &[KeyCode::Delete]);
        assert!(matches!(f.app.dialog, Some(Dialog::Delete { .. })));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(on_disk(&f.trash), ["notes.txt"]);
    }

    // Paste conflicts: the question on the bottom row, and its answers.

    fn inode(path: &Path) -> u64 {
        std::os::unix::fs::MetadataExt::ino(&fs::symlink_metadata(path).unwrap())
    }

    /// `notes.txt` copied (`y`) and Documents opened, where a `notes.txt`
    /// of its own says "old".
    fn clash() -> Fixture {
        let mut f = fixture();
        fs::write(f.home.join("Documents").join("notes.txt"), "old").unwrap();
        fs::write(f.home.join("Documents").join("plan.md"), "plan").unwrap();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('y')]);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        f
    }

    #[test]
    fn a_paste_that_meets_a_name_asks_on_the_bottom_row_with_the_cursor_on_it() {
        use crate::ui::testing::{buffer, find};
        let mut f = clash();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::End]);
        assert_eq!(selected_name(&f.app), Some("plan.md"));
        press(&mut f.app, &[KeyCode::Char('p')]);
        assert_eq!(
            selected_name(&f.app),
            Some("notes.txt"),
            "the cursor on the clash"
        );
        assert_eq!(f.app.working(), None, "nothing is written yet");
        let drawn = buffer(&mut f.app, 120, 30);
        let asked = "notes.txt already exists in Documents.";
        assert_eq!(find(&drawn, asked, 0..120), Some((1, 29)));
        assert_eq!(drawn[(1, 29)].fg, theme::FOREGROUND);
        let keys = "enter or b keeps both · r replaces · s skips · esc cancels";
        let x = 1 + asked.chars().count() as u16 + 2;
        assert_eq!(find(&drawn, keys, 0..120), Some((x, 29)));
        assert_eq!(drawn[(x, 29)].fg, theme::MUTED);

        // Any other key leaves it standing, and reaches nothing else.
        for other in [
            key(KeyCode::Char('q')),
            key(KeyCode::Char('j')),
            key(KeyCode::Char('d')),
            key(KeyCode::Delete),
            ctrl(KeyCode::Char('v')),
        ] {
            f.app.handle_key(other);
            assert!(f.app.question.is_some(), "{other:?}");
        }
        assert!(!f.app.should_quit);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        assert_eq!(on_disk(&documents), ["notes.txt", "plan.md"]);
        assert_eq!(read(documents.join("notes.txt")), "old");

        // Escape cancels the whole paste: nothing changes, nothing to undo.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.question.is_none());
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(&documents), ["notes.txt", "plan.md"]);
        assert_eq!(read(documents.join("notes.txt")), "old");
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));
        // The copy can still be pasted.
        press(&mut f.app, &[KeyCode::Char('p')]);
        assert!(f.app.question.is_some());
    }

    #[test]
    fn keep_both_pastes_under_copy_names_and_undo_takes_it_back() {
        for answer in [KeyCode::Enter, KeyCode::Char('b')] {
            let mut f = clash();
            let documents = f.home.join("Documents");
            press(&mut f.app, &[KeyCode::Char('p'), answer]);
            finish_work(&mut f.app);
            assert_eq!(
                on_disk(&documents),
                ["notes copy.txt", "notes.txt", "plan.md"]
            );
            assert_eq!(read(documents.join("notes copy.txt")), "hello");
            assert_eq!(read(documents.join("notes.txt")), "old");
            assert_eq!(said(&f.app), ("Copied 1 item", false));
            assert_eq!(selected_name(&f.app), Some("notes copy.txt"));
            f.app.handle_key(ctrl(KeyCode::Char('z')));
            assert_eq!(on_disk(&documents), ["notes.txt", "plan.md"]);
            f.app.handle_key(ctrl(KeyCode::Char('y')));
            assert_eq!(read(documents.join("notes copy.txt")), "hello");
        }
    }

    #[test]
    fn replace_and_undo_keep_the_spelling_of_a_name_that_differs_only_in_case() {
        let mut f = fixture();
        let documents = f.home.join("Documents");
        fs::write(documents.join("NOTES.TXT"), "old").unwrap();
        if !documents.join("notes.txt").exists() {
            // A case-sensitive volume: nothing clashes.
            return;
        }
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('y')]);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Char('p')]);
        let asked = f.app.question.as_ref().map(PasteQuestion::text);
        assert_eq!(
            asked.as_deref(),
            Some("NOTES.TXT already exists in Documents.")
        );
        assert_eq!(selected_name(&f.app), Some("NOTES.TXT"));
        press(&mut f.app, &[KeyCode::Char('r')]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["notes.txt"]);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(on_disk(&f.trash), ["NOTES.TXT"]);
        let record = f.app.trashed(&f.trash.join("NOTES.TXT")).unwrap();
        assert_eq!(record.from, documents.join("NOTES.TXT"));
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(on_disk(&documents), ["NOTES.TXT"]);
        assert_eq!(read(documents.join("NOTES.TXT")), "old");
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(on_disk(&documents), ["notes.txt"]);
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(on_disk(&documents), ["NOTES.TXT"]);
        assert_eq!(read(documents.join("NOTES.TXT")), "old");
    }

    #[test]
    fn replace_moves_the_one_there_to_the_trash_and_undo_brings_it_back() {
        let mut f = clash();
        let documents = f.home.join("Documents");
        let there = documents.join("notes.txt");
        let original = inode(&there);
        press(&mut f.app, &[KeyCode::Char('p'), KeyCode::Char('r')]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["notes.txt", "plan.md"]);
        assert_eq!(read(there.clone()), "hello");
        assert_eq!(
            read(f.home.join("notes.txt")),
            "hello",
            "the copy's original stays"
        );
        // The one that was there is in the trash, whole: moved, not written over.
        assert_eq!(on_disk(&f.trash), ["notes.txt"]);
        assert_eq!(read(f.trash.join("notes.txt")), "old");
        assert_eq!(inode(&f.trash.join("notes.txt")), original);
        assert_ne!(inode(&there), original);
        // On record, so that Restore knows where it goes back to.
        let record = f.app.trashed(&f.trash.join("notes.txt")).unwrap();
        assert_eq!(record.from, there);
        assert_eq!(said(&f.app), ("Copied 1 item", false));
        assert!(f.app.message.as_ref().unwrap().undoable);

        // One undo: the original back, byte for byte, and the copy gone.
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(read(there.clone()), "old");
        assert_eq!(inode(&there), original);
        assert_eq!(on_disk(&documents), ["notes.txt", "plan.md"]);
        assert_eq!(on_disk(&f.trash), ["notes copy.txt"]);
        assert_eq!(read(f.trash.join("notes copy.txt")), "hello");
        assert_eq!(said(&f.app), ("Undid the copy", false));
        // Redo replaces again; undo again brings the original back again.
        f.app.handle_key(ctrl_shift('z'));
        assert_eq!(read(there.clone()), "hello");
        assert_eq!(inode(&f.trash.join("notes.txt")), original);
        f.app.handle_key(ctrl(KeyCode::Char('z')));
        assert_eq!(
            (read(there.clone()), inode(&there)),
            ("old".to_string(), original)
        );
    }

    #[test]
    fn skip_leaves_the_clashing_items_and_pastes_the_others() {
        let mut f = clash();
        let documents = f.home.join("Documents");
        // Two taken (notes.txt, plan.md), one free (Pictures): asked once.
        fs::write(f.home.join("plan.md"), "new plan").unwrap();
        f.app.navigate(&f.home);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        for name in ["notes.txt", "plan.md", "Pictures"] {
            select(&mut f.app, name);
            press(&mut f.app, &[KeyCode::Char('v')]);
        }
        press(&mut f.app, &[KeyCode::Char('y')]);
        f.app.navigate(&documents);
        press(&mut f.app, &[KeyCode::Char('p')]);
        let asked = f.app.question.as_ref().map(PasteQuestion::text);
        assert_eq!(
            asked.as_deref(),
            Some("2 items already exist in Documents.")
        );
        press(&mut f.app, &[KeyCode::Char('s')]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["Pictures", "notes.txt", "plan.md"]);
        assert_eq!(read(documents.join("notes.txt")), "old");
        assert_eq!(read(documents.join("plan.md")), "plan");
        assert_eq!(on_disk(&documents.join("Pictures")), ["Trip", "a.jpg"]);
        assert!(on_disk(&f.trash).is_empty());
        assert_eq!(said(&f.app), ("Copied 1 item · 2 skipped", false));
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(on_disk(&documents), ["notes.txt", "plan.md"]);
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(on_disk(&documents), ["Pictures", "notes.txt", "plan.md"]);
    }

    #[test]
    fn one_answer_replaces_every_clash_of_the_paste_as_one_undo_step() {
        let mut f = clash();
        let documents = f.home.join("Documents");
        fs::write(f.home.join("plan.md"), "new plan").unwrap();
        f.app.navigate(&f.home);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        for name in ["notes.txt", "plan.md"] {
            select(&mut f.app, name);
            press(&mut f.app, &[KeyCode::Char('v')]);
        }
        press(&mut f.app, &[KeyCode::Char('x')]);
        f.app.navigate(&documents);
        press(&mut f.app, &[KeyCode::Char('p'), KeyCode::Char('r')]);
        finish_work(&mut f.app);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(read(documents.join("plan.md")), "new plan");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"], "they moved");
        assert_eq!(on_disk(&f.trash), ["notes.txt", "plan.md"]);
        assert_eq!(read(f.trash.join("plan.md")), "plan");
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(read(documents.join("notes.txt")), "old");
        assert_eq!(read(documents.join("plan.md")), "plan");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(read(f.home.join("plan.md")), "new plan");
        assert!(on_disk(&f.trash).is_empty());
        // Escape on a second paste of several changes nothing at all.
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(read(documents.join("plan.md")), "new plan");
    }

    #[test]
    fn capital_m_moves_here_whatever_was_taken() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('y')]);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::Char('M')]);
        assert_eq!(f.app.working(), Some("Moving 1 item…"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("Documents").join("notes.txt")), "hello");
        assert!(!f.home.join("notes.txt").exists());
        // What moved is taken no more.
        press(&mut f.app, &[KeyCode::Char('p')]);
        assert_eq!(said(&f.app), ("Nothing to paste", false));
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
    }

    // dd: the first d arms, the second moves to the trash.

    #[test]
    fn dd_within_a_second_and_a_half_moves_to_the_trash() {
        let mut f = fixture();
        let clock = manual_clock(&mut f.app);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert_eq!(
            said(&f.app),
            (
                "Press d again to move it to Trash, or press Delete on its own.",
                false
            )
        );
        assert!(f.home.join("notes.txt").exists());
        clock.set(clock.get() + Duration::from_millis(1400));
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
    }

    #[test]
    fn a_second_d_too_late_or_after_another_key_only_arms_again() {
        let mut f = fixture();
        let clock = manual_clock(&mut f.app);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('d')]);
        clock.set(clock.get() + Duration::from_millis(1500));
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(f.home.join("notes.txt").exists(), "the timeout disarmed it");
        // Armed again by that one: another key between disarms it.
        press(&mut f.app, &[KeyCode::Char('k'), KeyCode::Char('j')]);
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(f.home.join("notes.txt").exists());
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Char('d')]);
        assert!(f.home.join("notes.txt").exists());
        assert!(on_disk(&f.trash).is_empty());
        // With several marked it says "them", and takes them all.
        press(&mut f.app, &[KeyCode::Char('V')]);
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert_eq!(
            said(&f.app).0,
            "Press d again to move them to Trash, or press Delete on its own."
        );
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert_eq!(on_disk(&f.trash), ["Documents", "Pictures", "notes.txt"]);
    }

    #[test]
    fn nothing_arms_in_an_empty_folder() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(names(&f.app).is_empty());
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert_eq!(f.app.message, None);
        assert!(f.app.trash_armed.is_none());
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(on_disk(&f.trash).is_empty());
    }

    #[test]
    fn a_click_disarms_a_first_d_so_the_second_trashes_nothing() {
        use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut f = fixture();
        crate::ui::testing::buffer(&mut f.app, 120, 30);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(f.app.trash_armed.is_some());
        // A click on Documents' row: the cursor moves there with the mouse.
        let click = |button| MouseEvent {
            kind: MouseEventKind::Down(button),
            column: 30,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        f.app.handle_mouse(click(MouseButton::Left));
        assert!(f.app.trash_armed.is_none());
        assert_eq!(selected_name(&f.app), Some("Documents"));
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(on_disk(&f.trash).is_empty(), "the second d only arms again");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        // A right click too, whatever it lands on.
        f.app.handle_mouse(click(MouseButton::Right));
        assert!(f.app.trash_armed.is_none());
    }

    #[test]
    fn a_turn_of_the_wheel_disarms_a_first_d_so_the_second_trashes_nothing() {
        use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
        let mut f = fixture();
        crate::ui::testing::buffer(&mut f.app, 120, 30);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(f.app.trash_armed.is_some());
        // The wheel over the list moves the cursor off the armed row.
        f.app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 30,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert!(f.app.trash_armed.is_none());
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(on_disk(&f.trash).is_empty(), "the second d only arms again");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
    }

    #[test]
    fn file_work_that_finishes_disarms_a_first_d() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('U')]);
        assert!(f.app.working().is_some());
        // Armed while the duplicate is being made: when it lands the items
        // are listed again, and the second d trashes nothing.
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(f.app.trash_armed.is_some());
        finish_work(&mut f.app);
        assert!(f.app.trash_armed.is_none());
        press(&mut f.app, &[KeyCode::Char('d')]);
        assert!(on_disk(&f.trash).is_empty());
        assert_eq!(
            on_disk(&f.home),
            ["Documents", "Pictures", "notes copy.txt", "notes.txt"]
        );
    }

    // The detail column.

    #[test]
    fn capital_i_goes_round_size_modified_mode_and_owner_and_remembers_it() {
        use crate::ui::testing::screen;
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        fs::set_permissions(f.home.join("Documents"), fs::Permissions::from_mode(0o700)).unwrap();
        // The list and the two panes show every column under a header; the
        // columns view keeps one, which `I` changes.
        app.handle_key(ctrl(KeyCode::Char('r')));
        app.handle_key(ctrl(KeyCode::Char('2')));
        // The column's own cells on the first row, between the places panel
        // and the preview.
        let row = |app: &mut App| -> String {
            screen(app, 120, 30)[2].chars().skip(26).take(47).collect()
        };
        assert!(row(&mut app).ends_with(" No items  "), "{}", row(&mut app));
        let mut said_now = Vec::new();
        for _ in 0..4 {
            press(&mut app, &[KeyCode::Char('I')]);
            said_now.push(app.message.as_ref().unwrap().text.clone());
            let shown = row(&mut app);
            match app.detail {
                Detail::Modified => {
                    let modified = app.pane().entries[0].modified.unwrap();
                    let date =
                        crate::format::short_date(modified, SystemTime::now(), app.date_style);
                    assert!(shown.ends_with(&format!(" {date}  ")), "{shown}");
                }
                Detail::Mode => assert!(shown.ends_with(" drwx------  "), "{shown}"),
                Detail::Owner => {
                    let owner = model::owner(app.pane().entries[0].uid);
                    assert!(shown.ends_with(&format!(" {owner}  ")), "{shown}");
                }
                Detail::Size => assert!(shown.ends_with(" No items  "), "{shown}"),
            }
        }
        assert_eq!(
            said_now,
            [
                "Detail column · Modified",
                "Detail column · Mode",
                "Detail column · Owner",
                "Detail column · Size"
            ]
        );
        press(&mut app, &[KeyCode::Char('I'), KeyCode::Char('I')]);
        assert_eq!(launch(&f.home, Some(state.path())).detail, Detail::Mode);
    }

    // Keys that hand things to other programs, or move far.

    #[test]
    fn capital_e_edits_the_file_under_the_cursor_and_only_a_file() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('E')]);
        assert_eq!(*f.system.edited.borrow(), [f.home.join("notes.txt")]);
        assert!(f.app.take_redraw(), "the window is drawn whole again");
        assert!(!f.app.take_redraw());
        assert_eq!(f.app.message, None);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Char('E')]);
        assert_eq!(said(&f.app), ("Only a regular file can be edited", true));
        assert_eq!(f.system.edited.borrow().len(), 1);
        f.system.no_editor.set(true);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('E')]);
        assert_eq!(said(&f.app), ("Could not start the editor", true));
    }

    #[test]
    fn capital_y_copies_the_folders_path_and_ctrl_shift_c_the_items() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('Y')]);
        f.app.handle_key(ctrl_shift('c'));
        let home = f.home.display().to_string();
        let notes = f.home.join("notes.txt").display().to_string();
        assert_eq!(*f.system.copied.borrow(), [home, notes]);
        assert_eq!(said(&f.app), ("Copied the path.", false));
    }

    #[test]
    fn ctrl_t_opens_a_terminal_and_m_and_shift_f10_the_menu() {
        let mut f = fixture();
        select(&mut f.app, "Pictures");
        f.app.handle_key(ctrl(KeyCode::Char('t')));
        assert_eq!(*f.system.terminals.borrow(), [f.home.join("Pictures")]);
        for opens in [
            key(KeyCode::Char('m')),
            shift(KeyCode::F(10)),
            key(KeyCode::Menu),
        ] {
            f.app.handle_key(opens);
            assert!(f.app.menu.is_some(), "{opens:?}");
            press(&mut f.app, &[KeyCode::Esc]);
        }
        // Alt+Enter is not a key here.
        f.app.handle_key(alt(KeyCode::Enter));
        assert!(f.app.menu.is_none());
    }

    #[test]
    fn g_and_capital_g_and_the_page_keys_move_far() {
        let mut f = several();
        crate::ui::testing::screen(&mut f.app, 120, 8);
        press(&mut f.app, &[KeyCode::Char('G')]);
        assert_eq!(selected_name(&f.app), Some("notes.txt"));
        press(&mut f.app, &[KeyCode::Char('g')]);
        assert_eq!(selected_name(&f.app), Some("Documents"));
        // A page is the three rows a window this short shows.
        f.app.handle_key(ctrl(KeyCode::Char('d')));
        assert_eq!(selected_name(&f.app), Some("b.txt"));
        press(&mut f.app, &[KeyCode::PageDown]);
        assert_eq!(selected_name(&f.app), Some("e.txt"));
        f.app.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(selected_name(&f.app), Some("b.txt"));
        press(&mut f.app, &[KeyCode::PageUp]);
        assert_eq!(selected_name(&f.app), Some("Documents"));
    }

    #[test]
    fn dot_shows_hidden_files_and_ctrl_r_reloads() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('.')]);
        assert!(names(&f.app).contains(&".hidden".to_string()));
        press(&mut f.app, &[KeyCode::Char('.')]);
        fs::write(f.home.join("later.txt"), "").unwrap();
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", "later.txt", "notes.txt"]
        );
        // Ctrl+H is not a key here.
        f.app.handle_key(ctrl(KeyCode::Char('h')));
        assert!(!f.app.show_hidden);
    }

    // Compress formats and Extract, in the menu. The programs that write
    // and read archives are fakes: an archive holds the names of what went
    // in, a line each, and unpacking it makes a file of each name.

    /// Compress, then the format, in its submenu.
    fn compress_as(app: &mut App, format: &str) {
        pick_in(app, &["Compress", &format!(".{format}")]);
    }

    #[test]
    fn compress_offers_its_formats_and_names_the_archive_after_the_item() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        pick(&mut f.app, "Compress");
        assert_eq!(
            menu(&f.app),
            [".zip", ".tar", ".tar.gz", ".tar.bz2", ".tar.xz"]
        );
        press(&mut f.app, &[KeyCode::Esc]);
        // With 7-Zip installed, 7z too.
        f.system.seven_zip.set(true);
        pick(&mut f.app, "Compress");
        assert_eq!(
            menu(&f.app),
            [".zip", ".tar", ".tar.gz", ".tar.bz2", ".tar.xz", ".7z"]
        );
        press(&mut f.app, &[KeyCode::Esc]);

        compress_as(&mut f.app, "tar.gz");
        assert_eq!(f.app.working(), Some("Compressing notes.txt to .tar.gz"));
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes.tar.gz")), "notes.txt");
        assert_eq!(*f.system.formats_asked.borrow(), [Format::TarGz]);
        assert_eq!(said(&f.app), ("Compressed into notes.tar.gz.", false));
        // Again: the next free copy name, the whole extension kept.
        select(&mut f.app, "notes.txt");
        compress_as(&mut f.app, "tar.gz");
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes copy.tar.gz")), "notes.txt");
        select(&mut f.app, "notes.txt");
        compress_as(&mut f.app, "tar.gz");
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes copy 2.tar.gz")), "notes.txt");
        // Several: named after their folder.
        press(&mut f.app, &[KeyCode::Home]);
        f.app.handle_key(shift(KeyCode::Down));
        compress_as(&mut f.app, "7z");
        assert_eq!(f.app.working(), Some("Compressing 2 items to .7z"));
        finish_work(&mut f.app);
        let folder = name_of(&f.home);
        assert_eq!(
            read(f.home.join(format!("{folder}.7z"))),
            "Documents\nPictures"
        );
    }

    #[test]
    fn extract_unpacks_an_archive_into_a_new_folder_beside_it_and_undo_trashes_it() {
        let mut f = fixture();
        fs::write(f.home.join("pack.tar.gz"), "a.txt\nsub/b.txt").unwrap();
        fs::create_dir(f.home.join("pack")).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        // Only an archive has the line.
        select(&mut f.app, "notes.txt");
        f.app.handle_key(key(KeyCode::Char('m')));
        assert!(!menu(&f.app).contains(&"Extract"));
        press(&mut f.app, &[KeyCode::Esc]);

        select(&mut f.app, "pack.tar.gz");
        pick(&mut f.app, "Extract");
        assert_eq!(f.app.working(), Some("Extracting pack.tar.gz"));
        finish_work(&mut f.app);
        assert_eq!(said(&f.app), ("Extracted.", false));
        assert!(f.app.message.as_ref().is_some_and(|said| said.undoable));
        // pack is taken: pack copy, holding what the archive held.
        assert_eq!(
            read(f.home.join("pack copy").join("a.txt")),
            "from pack.tar.gz"
        );
        assert_eq!(read(f.home.join("pack copy/sub/b.txt")), "from pack.tar.gz");
        assert_eq!(selected_name(&f.app), Some("pack copy"));
        assert_eq!(
            all_on_disk(&f.home),
            [
                ".hidden",
                "Documents",
                "Pictures",
                "notes.txt",
                "pack",
                "pack copy",
                "pack.tar.gz"
            ]
        );

        f.app.handle_key(key(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid the extract", false));
        assert!(!f.home.join("pack copy").exists());
        assert_eq!(read(f.trash.join("pack copy/a.txt")), "from pack.tar.gz");
    }

    #[test]
    fn an_extract_that_is_refused_stopped_or_fails_leaves_nothing_behind() {
        let mut f = fixture();
        fs::write(f.home.join("slip.zip"), "../evil.txt").unwrap();
        fs::write(f.home.join("pack.zip"), "a.txt").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        let untouched = all_on_disk(&f.home);
        select(&mut f.app, "slip.zip");
        pick(&mut f.app, "Extract");
        finish_work(&mut f.app);
        let outside =
            "Couldn't extract slip.zip: it holds items that would land outside its folder.";
        assert_eq!(said(&f.app), (outside, true));
        assert_eq!(all_on_disk(&f.home), untouched);

        f.system.archiving.set(Archiving::Hangs);
        select(&mut f.app, "pack.zip");
        pick(&mut f.app, "Extract");
        f.app.tick();
        press(&mut f.app, &[KeyCode::Esc]);
        finish_work(&mut f.app);
        assert_eq!(said(&f.app), ("Extraction cancelled.", false));
        assert_eq!(all_on_disk(&f.home), untouched);

        f.system.archiving.set(Archiving::Fails);
        pick(&mut f.app, "Extract");
        finish_work(&mut f.app);
        let damaged = "Couldn't extract pack.zip: it is damaged or needs a password.";
        assert_eq!(said(&f.app), (damaged, true));
        assert_eq!(all_on_disk(&f.home), untouched);
        f.app.handle_key(key(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));

        // A 7z archive with no 7-Zip installed has no Extract.
        fs::write(f.home.join("pack.7z"), "a.txt").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        select(&mut f.app, "pack.7z");
        f.app.handle_key(key(KeyCode::Char('m')));
        assert!(!menu(&f.app).contains(&"Extract"));
    }

    // Bulk rename: `r` with several selected, in `$EDITOR`. The editor is
    // a stand-in that rewrites the file of names as each test says.

    fn editor(f: &Fixture, rewrite: impl Fn(&str) -> Option<String> + 'static) {
        *f.system.editor.borrow_mut() = Some(Rc::new(move |file: &Path| {
            let names = fs::read_to_string(file).unwrap();
            match rewrite(&names) {
                Some(edited) => fs::write(file, edited),
                None => Err(io::Error::other("the editor failed")),
            }
        }));
    }

    fn mark_three(f: &mut Fixture) {
        if chosen(&f.app) == ["a.txt", "b.txt", "c.txt"] {
            return;
        }
        select(&mut f.app, "a.txt");
        let mark = KeyCode::Char('v');
        press(
            &mut f.app,
            &[mark, KeyCode::Down, mark, KeyCode::Down, mark],
        );
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn r_with_several_selected_renames_them_in_the_editor_as_one_undo_step() {
        let mut f = several();
        editor(&f, |names| {
            assert_eq!(names, "a.txt\nb.txt\nc.txt\n");
            Some("b.txt\nc.txt\na.txt\n".to_string())
        });
        mark_three(&mut f);
        press(&mut f.app, &[KeyCode::Char('r')]);
        assert!(f.app.prompt.is_none());
        // The file of names was in the system's temporary folder, and is gone.
        let edited = f.system.edited.borrow().clone();
        assert_eq!(edited.len(), 1);
        assert!(edited[0].starts_with(std::env::temp_dir()));
        assert!(!edited[0].exists());
        // a → b, b → c, c → a.
        assert_eq!(read(f.home.join("b.txt")), "a");
        assert_eq!(read(f.home.join("c.txt")), "b");
        assert_eq!(read(f.home.join("a.txt")), "c");
        assert_eq!(
            on_disk(&f.home),
            [
                "Documents",
                "Pictures",
                "a.txt",
                "b.txt",
                "c.txt",
                "d.txt",
                "e.txt",
                "notes.txt"
            ]
        );
        assert_eq!(said(&f.app), ("Renamed 3 items · Undo reverts one", false));
        assert!(f.app.message.as_ref().is_some_and(|said| said.undoable));

        f.app.handle_key(key(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid the renames", false));
        assert_eq!(read(f.home.join("a.txt")), "a");
        assert_eq!(read(f.home.join("b.txt")), "b");
        assert_eq!(read(f.home.join("c.txt")), "c");
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(read(f.home.join("a.txt")), "c");

        // Only the names that changed count.
        editor(&f, |_| Some("a.txt\nx.txt\nc.txt\n".to_string()));
        mark_three(&mut f);
        press(&mut f.app, &[KeyCode::Char('r')]);
        assert_eq!(said(&f.app), ("Renamed 1 item · Undo reverts one", false));
        assert_eq!(read(f.home.join("x.txt")), "a");
    }

    #[test]
    fn a_bulk_rename_that_is_refused_or_given_up_renames_nothing() {
        let mut f = several();
        let before = on_disk(&f.home);
        let tries: [(&str, Option<&str>, (&str, bool)); 5] = [
            (
                "a.txt\nb.txt\n",
                Some("a.txt\nb.txt\n"),
                ("Bulk rename refused: the number of names changed", true),
            ),
            (
                "x.txt\nx.txt\nc.txt\n",
                Some("x.txt\nx.txt\nc.txt\n"),
                (
                    "Line 2: same as line 1. Give each item a different name.",
                    true,
                ),
            ),
            (
                "a.txt\nnotes.txt\nc.txt\n",
                Some("a.txt\nnotes.txt\nc.txt\n"),
                ("Bulk rename refused: a destination already exists", true),
            ),
            ("unchanged", None, ("0 renamed · 3 unchanged", false)),
            (
                "the editor fails",
                None,
                ("The editor quit with an error; nothing was renamed.", true),
            ),
        ];
        for (what, edited, expected) in tries {
            let edited = edited.map(str::to_string);
            let fails = what == "the editor fails";
            editor(&f, move |names| {
                if fails {
                    None
                } else {
                    Some(edited.clone().unwrap_or_else(|| names.to_string()))
                }
            });
            mark_three(&mut f);
            press(&mut f.app, &[KeyCode::Char('r')]);
            assert_eq!(said(&f.app), expected, "{what}");
            assert_eq!(on_disk(&f.home), before, "{what}");
            assert_eq!(read(f.home.join("a.txt")), "a");
        }
        f.app.handle_key(key(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));
        // One item is renamed on the bottom row, as before.
        press(&mut f.app, &[KeyCode::Esc]);
        select(&mut f.app, "a.txt");
        press(&mut f.app, &[KeyCode::Char('r')]);
        assert!(matches!(asking(&f.app), Some(Asking::Rename(_))));
    }

    // Finder tags: `T`, the receipts, undo and `tag:` in Find. The tags are
    // real ones, written on the fixture's files in a temporary folder.

    fn tags_of(path: PathBuf) -> Vec<String> {
        crate::tags::labels(&path)
    }

    fn tag(path: PathBuf, labels: &[&str]) {
        let labels: Vec<String> = labels.iter().map(|label| label.to_string()).collect();
        crate::tags::write(&path, Some(&crate::tags::encode(&labels))).unwrap();
    }

    const SEVEN: [&str; 7] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"];

    #[test]
    fn capital_t_offers_the_seven_tags_and_a_digit_toggles_one_over_the_selection() {
        let mut f = several();
        let (a, b) = (f.home.join("a.txt"), f.home.join("b.txt"));
        tag(a.clone(), &["Red\n6", "Work"]);
        select(&mut f.app, "a.txt");
        press(
            &mut f.app,
            &[KeyCode::Char('v'), KeyCode::Down, KeyCode::Char('v')],
        );
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);

        // A tag on one of them: Remove All Tags is offered under the seven.
        press(&mut f.app, &[KeyCode::Char('T')]);
        let mut offered = SEVEN.to_vec();
        offered.extend(["-", "Remove All Tags"]);
        assert_eq!(menu(&f.app), offered);
        // Red is on a, not on b: added where it is missing.
        press(&mut f.app, &[KeyCode::Char('1')]);
        assert!(f.app.menu.is_none());
        assert_eq!(tags_of(a.clone()), ["Red\n6", "Work"]);
        assert_eq!(tags_of(b.clone()), ["Red\n6"]);
        assert_eq!(said(&f.app), ("Tagged 1 item Red", false));
        assert!(f.app.message.as_ref().is_some_and(|said| said.undoable));
        assert_eq!(chosen(&f.app), ["a.txt", "b.txt"]);

        // On both now: taken off both.
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('1')]);
        assert_eq!(tags_of(a.clone()), ["Work"]);
        assert!(tags_of(b.clone()).is_empty());
        assert_eq!(said(&f.app), ("Removed Red from 2 items", false));

        // None of the seven on either: no Remove All Tags, and Escape
        // closes the menu with nothing done.
        press(&mut f.app, &[KeyCode::Char('T')]);
        assert_eq!(menu(&f.app), SEVEN);
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.menu.is_none());
        assert_eq!(tags_of(a.clone()), ["Work"]);

        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('5')]);
        assert_eq!(said(&f.app), ("Tagged 2 items Blue", false));
        pick_tag_remove_all(&mut f.app);
        assert_eq!(said(&f.app), ("Removed all tags from 2 items", false));
        // The user's own tag stays.
        assert_eq!(tags_of(a.clone()), ["Work"]);
        assert!(tags_of(b.clone()).is_empty());

        // Undo puts Blue back on both, redo takes it off again.
        f.app.handle_key(key(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("Undid the tag change", false));
        assert_eq!(tags_of(a.clone()), ["Work", "Blue\n4"]);
        assert_eq!(tags_of(b.clone()), ["Blue\n4"]);
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert_eq!(tags_of(a.clone()), ["Work"]);
        assert!(tags_of(b).is_empty());

        // One item: the singular.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('7')]);
        assert_eq!(said(&f.app), ("Tagged 1 item Gray", false));
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('7')]);
        assert_eq!(said(&f.app), ("Removed Gray from 1 item", false));
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('3')]);
        pick_tag_remove_all(&mut f.app);
        assert_eq!(said(&f.app), ("Removed all tags from 1 item", false));
    }

    fn pick_tag_remove_all(app: &mut App) {
        press(app, &[KeyCode::Char('T')]);
        press(app, &[KeyCode::Down; 8]);
        assert_eq!(menu(app)[menu_cursor(app)], "Remove All Tags");
        press(app, &[KeyCode::Enter]);
    }

    #[test]
    fn tags_show_after_the_name_once_the_folder_is_listed_again() {
        let mut f = fixture();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('2')]);
        let entry = f.app.selected().expect("notes.txt");
        assert_eq!(entry.name, "notes.txt");
        assert_eq!(entry.tags, 0b10);
    }

    #[test]
    fn tagging_on_a_disk_without_room_for_tags_says_so_and_changes_nothing() {
        let mut f = fixture();
        crate::tags::testing::pretend_unsupported(&f.home);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('1')]);
        let unsupported = "This filesystem does not support file tags.";
        assert_eq!(said(&f.app), (unsupported, true));
        assert!(tags_of(f.home.join("notes.txt")).is_empty());
        f.app.handle_key(key(KeyCode::Char('z')));
        assert_eq!(said(&f.app), ("There's nothing to undo.", false));
    }

    #[test]
    fn find_takes_tag_terms_and_finds_only_what_carries_every_tag_named() {
        let mut f = several();
        tag(f.home.join("a.txt"), &["Red\n6"]);
        tag(f.home.join("b.txt"), &["red\n6", "Blue\n4"]);
        tag(f.home.join("Pictures").join("a.jpg"), &["Red\n6", "Work"]);
        tag(f.home.join("c.txt"), &["Blue\n4"]);
        let found = |app: &App| {
            let mut found = names(app);
            found.sort();
            found
        };
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "tag:red");
        finish_search(&mut f.app);
        assert_eq!(found(&f.app), ["a.jpg", "a.txt", "b.txt"]);
        // Nothing is bold: no text was asked for.
        assert_eq!(f.app.pane().matching(), "");

        clear_field(&mut f.app);
        type_text(&mut f.app, "TAG:red tag:BLUE");
        finish_search(&mut f.app);
        assert_eq!(found(&f.app), ["b.txt"]);

        // With text as well: what carries the tag and matches the text.
        clear_field(&mut f.app);
        type_text(&mut f.app, "TAG:red a");
        finish_search(&mut f.app);
        assert_eq!(found(&f.app), ["a.jpg", "a.txt"]);
        assert_eq!(f.app.pane().matching(), "a");

        // A tag of the user's own is found the same way.
        clear_field(&mut f.app);
        type_text(&mut f.app, "tag:work");
        finish_search(&mut f.app);
        assert_eq!(found(&f.app), ["a.jpg"]);
    }

    /// The fixture with Spotlight: a.txt and Pictures/a.jpg red, c.txt blue,
    /// and a hidden red file. The stand-in for Spotlight also names c.txt
    /// (no longer red), the hidden one and one that has gone.
    fn spotlit() -> Fixture {
        let f = several();
        let red = [f.home.join("a.txt"), f.home.join("Pictures").join("a.jpg")];
        for path in &red {
            tag(path.clone(), &["Red\n6"]);
        }
        tag(f.home.join("c.txt"), &["Blue\n4"]);
        let hidden = f.home.join(".secret.txt");
        fs::write(&hidden, "s").unwrap();
        tag(hidden.clone(), &["Red\n6"]);
        let mut finds = red.to_vec();
        finds.extend([f.home.join("c.txt"), hidden, f.home.join("gone.txt")]);
        *f.system.spotlight_finds.borrow_mut() = Some(finds);
        f
    }

    #[test]
    fn a_find_of_only_tags_everywhere_asks_spotlight_and_keeps_what_still_carries_them() {
        let mut f = spotlit();
        let found = |app: &App| {
            let mut found = names(app);
            found.sort();
            found
        };
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "tag:red");
        finish_search(&mut f.app);
        // In a folder (or home) the walk looks, exact where Spotlight
        // doesn't index.
        assert!(f.system.spotlit.borrow().is_empty());
        assert_eq!(found(&f.app), ["a.jpg", "a.txt"]);
        // Everywhere: Spotlight, over the whole Mac; c.txt isn't red any
        // more, one has gone. (The test's home is in a hidden temporary
        // folder, so hidden items show for this part.)
        f.app.show_hidden = true;
        while f.app.pane().search().unwrap().scope != Scope::Everywhere {
            press(&mut f.app, &[KeyCode::Tab]);
        }
        finish_search(&mut f.app);
        assert_eq!(f.system.spotlit.borrow().last().unwrap().1, None);
        assert_eq!(found(&f.app), [".secret.txt", "a.jpg", "a.txt"]);
        f.app.show_hidden = false;
        while f.app.pane().search().unwrap().scope != Scope::Folder {
            press(&mut f.app, &[KeyCode::Tab]);
        }
        finish_search(&mut f.app);
        // With words as well, the walk looks as before (typed words first,
        // so no step of it is tags alone).
        clear_field(&mut f.app);
        let asked = f.system.spotlit.borrow().len();
        type_text(&mut f.app, "a tag:red");
        finish_search(&mut f.app);
        assert_eq!(f.system.spotlit.borrow().len(), asked);
        assert_eq!(found(&f.app), ["a.jpg", "a.txt"]);
    }

    #[test]
    fn what_spotlight_finds_under_a_hidden_folder_shows_only_with_hidden_items() {
        let paths = ["/a/b.txt", "/a/.c/d.txt", "/a/.e.txt", "/root/x/.y"];
        let through = |root: Option<&str>, show_hidden: bool| {
            let grep: Grep = Box::new(move |found, _stop| {
                for path in paths {
                    if !found(PathBuf::from(path)) {
                        break;
                    }
                }
                Ok(())
            });
            let mut seen = Vec::new();
            let stop = AtomicBool::new(false);
            super::shown_only(grep, root.map(PathBuf::from), show_hidden)(
                &mut |path| {
                    seen.push(path.display().to_string());
                    true
                },
                &stop,
            )
            .unwrap();
            seen
        };
        assert_eq!(through(None, false), ["/a/b.txt"]);
        assert_eq!(through(None, true), paths);
        // Below a hidden root only what is hidden below it counts.
        assert_eq!(through(Some("/a/.c"), false), ["/a/b.txt", "/a/.c/d.txt"]);
    }

    #[test]
    fn enter_keeps_a_find_of_only_tags_and_nothing_found_says_so() {
        let mut f = spotlit();
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "tag:red");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.field.is_none(), "the keyboard is in the results");
        assert!(f.app.pane().results().is_some());
        assert_eq!(names(&f.app).len(), 2);

        *f.system.spotlight_finds.borrow_mut() = Some(Vec::new());
        press(&mut f.app, &[KeyCode::Char('f')]);
        clear_field(&mut f.app);
        type_text(&mut f.app, "tag:green");
        finish_search(&mut f.app);
        assert!(names(&f.app).is_empty());
        assert_eq!(
            crate::ui::empty_text(f.app.pane(), 0),
            "NOTHING MATCHES tag:green"
        );
    }

    #[test]
    fn an_item_that_loses_the_tag_leaves_the_results() {
        let mut f = spotlit();
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "tag:red");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);
        let at = names(&f.app)
            .iter()
            .position(|name| name == "a.txt")
            .unwrap();
        f.app.pane_mut().cursor = at;
        // `T` then 1 takes Red off a.txt.
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('1')]);
        assert!(crate::tags::labels(&f.home.join("a.txt")).is_empty());
        assert_eq!(names(&f.app), ["a.jpg"]);
    }

    #[test]
    fn the_panel_lists_the_tags_in_use_and_one_opens_find_for_it() {
        let mut f = spotlit();
        // The test's home is in a hidden temporary folder: hidden items show.
        f.app.show_hidden = true;
        f.system
            .in_use
            .set(Some(crate::tags::mask(&["Red".into(), "Blue".into()])));
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        let rows: Vec<String> = places::flat(&f.app.places)
            .map(|place| place.name.clone())
            .collect();
        let red = rows
            .iter()
            .position(|name| name == "Red")
            .expect("a Red row");
        assert_eq!(rows[red + 1], "Blue");
        f.app.open_place(red);
        finish_search(&mut f.app);
        let search = f.app.pane().search().expect("Find");
        assert_eq!(search.input.text(), "tag:red");
        assert_eq!(search.scope, Scope::Everywhere);
        assert_eq!(f.system.spotlit.borrow().last().unwrap().1, None);
        assert_eq!(f.app.focus, Focus::List);
        assert!(f.app.field.is_none());
        let mut found = names(&f.app);
        found.sort();
        assert_eq!(found, [".secret.txt", "a.jpg", "a.txt"]);
        // Its row is the current place while they show.
        assert_eq!(f.app.current_place(), Some(red));
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(f.app.pane().search().is_none());
        assert_ne!(f.app.current_place(), Some(red));
    }

    #[test]
    fn the_tags_in_use_are_asked_at_start_on_ctrl_r_and_after_tagging_only() {
        let mut f = spotlit();
        let start = f.system.in_use_asked.get();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert_eq!(f.system.in_use_asked.get(), start + 1);
        // Entering the panel uses what is known.
        press(&mut f.app, &[KeyCode::Tab]);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.system.in_use_asked.get(), start + 1);
        f.system
            .in_use
            .set(Some(crate::tags::mask(&["Green".into()])));
        press(&mut f.app, &[KeyCode::Char('T'), KeyCode::Char('4')]);
        assert_eq!(f.system.in_use_asked.get(), start + 2);
        let rows: Vec<String> = places::flat(&f.app.places)
            .map(|place| place.name.clone())
            .collect();
        assert!(rows.contains(&"Green".to_string()));
    }
}

/// The mouse: every click lands on what the last frame drew under it, and
/// does what the key for the same thing does.
#[cfg(test)]
mod mouse_tests {
    use super::testing::*;
    use super::*;
    use crate::ui::testing::{buffer, find, panes, tracks};
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

    /// The names in a folder on disk, none when there is no such folder.
    fn on_disk_names(folder: &std::path::Path) -> Vec<String> {
        let items = fs::read_dir(folder).into_iter().flatten();
        items
            .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
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

    const VIEWS: [char; 3] = ['1', '3', '2'];

    /// The columns a view draws a single pane's items in, at 120 across:
    /// between the places panel and the preview, in every view.
    fn files(app: &App) -> Range<u16> {
        panes(app, 120).files
    }

    /// Draws the window and says where a piece of text starts in the files.
    fn spot_file(app: &mut App, text: &str) -> (u16, u16) {
        let files = files(app);
        spot(app, text, files)
    }

    #[test]
    fn a_click_on_an_item_puts_the_keyboard_and_the_cursor_on_it_alone_in_every_view() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            f.app.handle_key(shift(KeyCode::Down));
            // The keyboard elsewhere: in the preview.
            f.app.focus = Focus::Preview;
            let (x, y) = spot_file(&mut f.app, "c.txt");
            // A tile is its picture as much as its name.
            let at = if view == '3' { (x, y - 3) } else { (x, y) };
            click(&mut f.app, at);
            assert_eq!(f.app.focus, Focus::List, "view {view}");
            assert_eq!(f.app.active(), 0);
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
            assert_eq!(f.app.active(), 1);
            let at = spot(&mut f.app, "a.txt", 0..60);
            click(&mut f.app, at);
            assert_eq!(
                (f.app.active(), f.app.focus),
                (0, Focus::List),
                "view {view}"
            );
            assert_eq!(cursor_name(&f.app), "a.txt", "view {view}");
            assert_eq!(f.app.panes()[1].cursor, 1, "the other pane's cursor stays");

            // Empty space in the right pane: active, its cursor where it was.
            draw(&mut f.app);
            click(&mut f.app, (90, 36));
            assert_eq!(
                (f.app.active(), f.app.focus),
                (1, Focus::List),
                "view {view}"
            );
            assert_eq!(f.app.panes()[1].cursor, 1, "view {view}");
            assert_eq!(f.app.panes()[0].cursor, 2, "view {view}");
        }
    }

    #[test]
    fn a_click_on_empty_space_makes_the_pane_active_and_leaves_the_selection_alone() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            f.app.handle_key(shift(KeyCode::Down));
            f.app.focus = Focus::Preview;
            draw(&mut f.app);
            click(&mut f.app, (70, 37));
            assert_eq!(f.app.focus, Focus::List, "view {view}");
            assert_eq!(selection(&f.app), ["Documents", "Pictures"], "view {view}");
            assert_eq!(cursor_name(&f.app), "Pictures", "view {view}");
        }
    }

    #[test]
    fn in_columns_a_click_above_goes_to_that_folder_and_one_in_the_last_column_selects() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        // The way down is home, then Pictures: a track each.
        let tracks = tracks(&f.app, 120);
        assert_eq!(tracks.len(), 2);
        let trip = spot(&mut f.app, "Trip", tracks[1].clone());
        let jpg = spot(&mut f.app, "a.jpg", tracks[1].clone());
        click(&mut f.app, jpg);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(cursor_name(&f.app), "a.jpg");
        click(&mut f.app, trip);
        assert_eq!(cursor_name(&f.app), "Trip");

        // The home column, left of it: there, with the row clicked selected.
        let notes = spot(&mut f.app, "notes.txt", tracks[0].clone());
        click(&mut f.app, notes);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(cursor_name(&f.app), "notes.txt");
        assert_eq!(f.app.focus, Focus::List);
        // Ordinary navigation: Back returns.
        f.app.handle_key(key(KeyCode::Char('H')));
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
    }

    #[test]
    fn two_clicks_on_one_item_within_400_ms_open_it_like_enter_in_every_view() {
        for view in VIEWS {
            let mut f = fixture();
            let now = clock(&mut f.app);
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let pictures = spot_file(&mut f.app, "Pictures");
            click(&mut f.app, pictures);
            wait(&now, 400);
            draw(&mut f.app);
            click(&mut f.app, pictures);
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"), "view {view}");
            assert_eq!(f.app.pane().back.len(), 1, "in the history");

            // A file opens with its app.
            let jpg = spot_file(&mut f.app, "a.jpg");
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
            let trip = spot_file(&mut f.app, "Trip");
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
    fn shift_click_selects_the_range_from_the_cursor_to_the_item() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let a = spot_file(&mut f.app, "a.txt");
            let b = spot_file(&mut f.app, "b.txt");
            let c = spot_file(&mut f.app, "c.txt");
            let d = spot_file(&mut f.app, "d.txt");
            let e = spot_file(&mut f.app, "e.txt");
            click(&mut f.app, a);
            click_with(&mut f.app, d, KeyModifiers::SHIFT);
            assert_eq!(selection(&f.app), ["a.txt", "b.txt", "c.txt", "d.txt"]);
            assert_eq!(cursor_name(&f.app), "d.txt", "view {view}");
            // Again from there: the range is from the cursor, nothing else.
            click_with(&mut f.app, b, KeyModifiers::SHIFT);
            assert_eq!(
                selection(&f.app),
                ["b.txt", "c.txt", "d.txt"],
                "view {view}"
            );
            assert_eq!(cursor_name(&f.app), "b.txt", "view {view}");
            // A plain click starts again; Shift+arrows go on from the cursor.
            click(&mut f.app, e);
            assert_eq!(selection(&f.app), ["e.txt"], "view {view}");
            click_with(&mut f.app, c, KeyModifiers::SHIFT);
            assert_eq!(
                selection(&f.app),
                ["c.txt", "d.txt", "e.txt"],
                "view {view}"
            );
            let back = if view == '3' {
                KeyCode::Left
            } else {
                KeyCode::Up
            };
            f.app.handle_key(shift(back));
            assert_eq!(
                selection(&f.app),
                ["b.txt", "c.txt", "d.txt", "e.txt"],
                "view {view}"
            );
            assert_eq!(cursor_name(&f.app), "b.txt", "view {view}");
        }
    }
    #[test]
    fn ctrl_or_cmd_click_marks_or_unmarks_one_item() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let at = |f: &mut Fixture, name| spot_file(&mut f.app, name);
            let (a, c, e) = (
                at(&mut f, "a.txt"),
                at(&mut f, "c.txt"),
                at(&mut f, "e.txt"),
            );
            // The item a plain click chose is the selection: it stays in it.
            click(&mut f.app, a);
            click_with(&mut f.app, c, KeyModifiers::CONTROL);
            assert_eq!(selection(&f.app), ["a.txt", "c.txt"], "view {view}");
            assert_eq!(cursor_name(&f.app), "c.txt");
            click_with(&mut f.app, e, KeyModifiers::SUPER);
            assert_eq!(selection(&f.app), ["a.txt", "c.txt", "e.txt"]);
            click_with(&mut f.app, e, KeyModifiers::CONTROL);
            assert_eq!(selection(&f.app), ["a.txt", "c.txt"], "view {view}");
            assert_eq!(cursor_name(&f.app), "e.txt");
            click_with(&mut f.app, a, KeyModifiers::SUPER);
            assert_eq!(selection(&f.app), ["c.txt"], "view {view}");
            // With the cursor left there by keys, only the item clicked.
            click_with(&mut f.app, c, KeyModifiers::CONTROL);
            press(&mut f.app, &[KeyCode::Esc]);
            assert!(f.app.pane().marks.is_empty());
            click_with(&mut f.app, e, KeyModifiers::CONTROL);
            assert_eq!(selection(&f.app), ["e.txt"], "view {view}");
            assert_eq!(f.app.pane().marks.len(), 1);
            // Alt is not a click modifier here: a plain click.
            click_with(&mut f.app, a, KeyModifiers::ALT);
            assert_eq!(selection(&f.app), ["a.txt"], "view {view}");
            assert!(f.app.pane().marks.is_empty());
        }
    }
    #[test]
    fn right_click_selects_what_is_outside_the_selection_and_opens_its_menu_at_the_row() {
        for view in VIEWS {
            let mut f = several();
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let at = |f: &mut Fixture, name| spot_file(&mut f.app, name);
            let (a, b, c, e) = (
                at(&mut f, "a.txt"),
                at(&mut f, "b.txt"),
                at(&mut f, "c.txt"),
                at(&mut f, "e.txt"),
            );
            click(&mut f.app, a);
            click_with(&mut f.app, c, KeyModifiers::SHIFT);
            press(&mut f.app, &[KeyCode::BackTab]);

            right_click(&mut f.app, (e.0 + 2, e.1));
            assert_eq!(selection(&f.app), ["e.txt"], "view {view}");
            assert_eq!(f.app.focus, Focus::List);
            assert_eq!(
                &labels(&f.app)[..4],
                ["Open", "Open With", "Open in Terminal", "-"]
            );
            // Under the row clicked, at its name, or above it without room.
            let drawn = draw(&mut f.app);
            if view == '1' {
                let corner = drawn[(e.0, e.1 + 1)].symbol();
                let above = drawn[(e.0, e.1 - 1)].symbol();
                assert!(corner == "┌" || above == "└", "view {view}");
            }
            assert!(find(&drawn, "Duplicate", 0..120).is_some(), "view {view}");

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
            let rows = &f.app.menu.as_ref().unwrap().rows;
            let rename = MenuRow::Item {
                label: "Rename",
                action: Action::Rename,
                enabled: true,
                tag: None,
            };
            assert!(rows.contains(&rename), "several renamed in the editor");
        }

        // `m` after it opens at the same row, wherever the click was.
        let mut f = several();
        let e = spot_file(&mut f.app, "e.txt");
        right_click(&mut f.app, (e.0 + 10, e.1));
        let clicked = draw(&mut f.app);
        press(&mut f.app, &[KeyCode::Esc]);
        f.app.handle_key(key(KeyCode::Char('m')));
        assert_eq!(draw(&mut f.app), clicked);
        assert_eq!(clicked[(e.0, e.1 + 1)].symbol(), "┌");
    }
    #[test]
    fn right_click_on_empty_space_opens_nothing() {
        let mut f = several();
        draw(&mut f.app);
        right_click(&mut f.app, (40, 30));
        assert!(f.app.menu.is_none());
        assert_eq!(cursor_name(&f.app), "Documents", "nothing selected anew");
    }

    #[test]
    fn a_click_on_a_line_with_a_chevron_opens_its_submenu_and_moving_over_lines_lights_them() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(key(KeyCode::Char('m')));
        assert!(f.app.wants_motion(), "every move is heard over the menu");
        let compress = spot_file(&mut f.app, "Compress");
        mouse(
            &mut f.app,
            MouseEventKind::Moved,
            compress,
            KeyModifiers::NONE,
        );
        assert_eq!(f.app.menu.as_ref().map(|menu| menu.cursor), Some(4));
        // Over a rule nothing moves.
        let rule = (compress.0, compress.1 + 1);
        mouse(&mut f.app, MouseEventKind::Moved, rule, KeyModifiers::NONE);
        assert_eq!(f.app.menu.as_ref().map(|menu| menu.cursor), Some(4));
        click(&mut f.app, compress);
        assert_eq!(
            f.app.menu.as_ref().map(|menu| menu.sub),
            Some(Sub::Compress)
        );
        let zip = spot_file(&mut f.app, ".zip");
        click(&mut f.app, zip);
        assert!(f.app.menu.is_none());
        assert_eq!(f.app.working(), Some("Compressing notes.txt to .zip"));
        assert!(f.app.menu.is_none());
        // Moves are still heard, for the view buttons the header shows.
        assert!(f.app.wants_motion());
    }
    #[test]
    fn a_click_on_a_menu_line_runs_it_and_one_outside_only_closes_the_menu() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(key(KeyCode::Char('m')));
        let documents = spot_file(&mut f.app, "Documents");
        click(&mut f.app, documents);
        assert!(f.app.menu.is_none());
        assert_eq!(
            cursor_name(&f.app),
            "notes.txt",
            "nothing reached the files"
        );

        f.app.handle_key(key(KeyCode::Char('m')));
        let copy_path = spot_file(&mut f.app, "Copy Path");
        click(&mut f.app, copy_path);
        assert!(f.app.menu.is_none());
        let path = f.home.join("notes.txt").display().to_string();
        assert_eq!(*f.system.copied.borrow(), [path]);

        // A right click on another item while it is open only closes it too.
        f.app.handle_key(key(KeyCode::Char('m')));
        draw(&mut f.app);
        right_click(&mut f.app, documents);
        assert!(f.app.menu.is_none());
        assert_eq!(cursor_name(&f.app), "notes.txt");
    }

    #[test]
    fn a_click_on_a_breadcrumb_goes_there_and_back_returns() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));
        let crumb = spot(&mut f.app, "Pictures", 0..86);
        assert_eq!(crumb.1, 0, "in the header");
        click(&mut f.app, crumb);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        f.app.handle_key(key(KeyCode::Char('H')));
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
        assert_eq!(f.app.panes()[1].cwd, f.home);
        assert_eq!(f.app.active(), 1);
    }

    fn said(app: &App) -> Option<&str> {
        app.message.as_ref().map(|message| message.text.as_str())
    }

    #[test]
    fn the_wheel_moves_the_cursor_one_row_like_j_and_k() {
        let mut f = several();
        let top = spot_file(&mut f.app, "Documents");
        f.app.focus = Focus::Preview;
        wheel(&mut f.app, top, true);
        assert_eq!(cursor_name(&f.app), "Pictures");
        assert_eq!(f.app.focus, Focus::List, "the keyboard back in the list");
        wheel(&mut f.app, top, true);
        wheel(&mut f.app, top, true);
        assert_eq!(cursor_name(&f.app), "b.txt");
        wheel(&mut f.app, top, false);
        assert_eq!(cursor_name(&f.app), "a.txt");
        assert_eq!(selection(&f.app), ["a.txt"], "nothing marked");
        // Not past the first item.
        for _ in 0..5 {
            wheel(&mut f.app, top, false);
        }
        assert_eq!(cursor_name(&f.app), "Documents");
    }
    #[test]
    fn the_wheel_over_the_preview_scrolls_it_and_a_click_gives_it_the_keyboard() {
        let mut f = fixture();
        let lines: String = (1..=80).map(|n| format!("line {n}\n")).collect();
        fs::write(f.home.join("notes.txt"), lines).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::End]);
        f.app.tick();
        draw(&mut f.app);
        wheel(&mut f.app, (100, 20), true);
        wheel(&mut f.app, (100, 20), true);
        assert_eq!(f.app.preview_scroll, 2);
        let drawn = draw(&mut f.app);
        assert_eq!(find(&drawn, "3 line 3", 74..120).map(|(_, y)| y), Some(5));
        wheel(&mut f.app, (100, 20), false);
        assert_eq!(f.app.preview_scroll, 1);
        assert_eq!(
            f.app.focus,
            Focus::List,
            "the wheel leaves the keyboard alone"
        );
        click(&mut f.app, (100, 20));
        assert_eq!(f.app.focus, Focus::Preview);
        assert_eq!(cursor_name(&f.app), "notes.txt");
    }
    #[test]
    fn the_wheel_moves_the_grid_s_cursor_a_row_of_tiles() {
        let mut f = grid_fixture();
        // Ten tiles, three across.
        let drawn = buffer(&mut f.app, 62, 26);
        let first = find(&drawn, "Documents", 0..62).unwrap();
        wheel(&mut f.app, first, true);
        assert_eq!(f.app.pane().cursor, 3);
        wheel(&mut f.app, first, true);
        wheel(&mut f.app, first, true);
        assert_eq!(f.app.pane().cursor, 9);
        wheel(&mut f.app, first, false);
        assert_eq!(f.app.pane().cursor, 6);
    }
    #[test]
    fn the_wheel_over_the_shortcuts_scrolls_them_and_a_click_outside_closes_them() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let top = crate::ui::testing::screen(&mut f.app, 120, 24);
        wheel(&mut f.app, (60, 12), true);
        assert_eq!(f.app.help_scroll, 1);
        let wheeled = crate::ui::testing::screen(&mut f.app, 120, 24);
        assert_ne!(wheeled, top);
        press(&mut f.app, &[KeyCode::Up]);
        assert_eq!(crate::ui::testing::screen(&mut f.app, 120, 24), top);
        press(&mut f.app, &[KeyCode::Down]);

        // A click inside, a right click or the wheel outside reach nothing.
        click(&mut f.app, (60, 12));
        right_click(&mut f.app, (1, 1));
        wheel(&mut f.app, (1, 23), true);
        assert!(f.app.help_visible && f.app.menu.is_none());
        assert_eq!(crate::ui::testing::screen(&mut f.app, 120, 24), wheeled);
        assert_eq!((f.app.pane().cursor, f.app.focus), (0, Focus::List));
        // A click outside closes them, and does nothing else.
        click(&mut f.app, (1, 23));
        assert!(!f.app.help_visible);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(f.app.pane().cursor, 0);
    }
    #[test]
    fn a_click_on_a_card_s_button_presses_it_and_one_outside_cancels() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(shift(KeyCode::Delete));
        assert!(f.app.wants_motion(), "the buttons follow the pointer");
        let documents = spot_file(&mut f.app, "Documents");
        // A right click outside does nothing; moving onto Delete puts the
        // keyboard on it, back on Cancel puts it back.
        right_click(&mut f.app, documents);
        assert!(matches!(
            f.app.dialog,
            Some(Dialog::Delete { armed: false, .. })
        ));
        let delete = spot(&mut f.app, "[ Delete ]", 0..120);
        let cancel = spot(&mut f.app, "[ Cancel ]", 0..120);
        mouse(
            &mut f.app,
            MouseEventKind::Moved,
            delete,
            KeyModifiers::NONE,
        );
        assert!(f.app.dialog.as_ref().is_some_and(Dialog::armed));
        mouse(
            &mut f.app,
            MouseEventKind::Moved,
            cancel,
            KeyModifiers::NONE,
        );
        assert!(f.app.dialog.as_ref().is_some_and(|dialog| !dialog.armed()));
        // A click outside cancels, and reaches nothing under it.
        click(&mut f.app, documents);
        assert!(f.app.dialog.is_none() && f.app.menu.is_none());
        assert_eq!(cursor_name(&f.app), "notes.txt");
        assert!(f.home.join("notes.txt").exists());

        f.app.handle_key(shift(KeyCode::Delete));
        let cancel = spot(&mut f.app, "[ Cancel ]", 0..120);
        click(&mut f.app, (cancel.0 + 3, cancel.1));
        assert!(f.app.dialog.is_none());
        assert!(f.home.join("notes.txt").exists());

        f.app.handle_key(shift(KeyCode::Delete));
        let delete = spot(&mut f.app, "[ Delete ]", 0..120);
        click(&mut f.app, (delete.0 + 2, delete.1));
        assert!(f.app.dialog.is_none());
        assert!(!f.home.join("notes.txt").exists());
        assert!(on_disk_names(&f.trash).is_empty(), "deleted, not trashed");
        assert_eq!(said(&f.app), Some("Deleted 1 item."));
    }

    #[test]
    fn two_clicks_on_what_find_found_show_it_in_its_folder_like_o() {
        let mut f = fixture();
        let now = clock(&mut f.app);
        press(&mut f.app, &[KeyCode::Char('f')]);
        for c in "jpg".chars() {
            f.app.handle_key(key(KeyCode::Char(c)));
        }
        finish_search(&mut f.app);
        let found = spot_file(&mut f.app, "a.jpg");
        click(&mut f.app, found);
        wait(&now, 200);
        draw(&mut f.app);
        click(&mut f.app, found);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert!(f.app.pane().results().is_none());
        assert_eq!(cursor_name(&f.app), "a.jpg");
        assert!(f.system.opened.borrow().is_empty(), "shown, not opened");
    }

    #[test]
    fn the_quick_look_card_closes_on_its_cross_or_outside_and_scrolls_with_the_wheel() {
        let mut f = fixture();
        let lines: String = (1..=80).map(|n| format!("line {n}\n")).collect();
        fs::write(f.home.join("notes.txt"), lines).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::End, KeyCode::Char(' ')]);
        f.app.tick();
        assert!(!f.app.wants_motion(), "the card has no buttons to follow");
        let drawn = draw(&mut f.app);
        let cross = find(&drawn, "×", 0..120).expect("the cross");
        // Inside, a click does nothing; the wheel scrolls it a row.
        click(&mut f.app, (60, 20));
        wheel(&mut f.app, (60, 20), true);
        wheel(&mut f.app, (60, 20), true);
        assert_eq!(f.app.look.as_ref().map(|look| look.scroll), Some(2));
        assert_eq!(f.app.preview_scroll, 0, "not the preview under it");
        assert_eq!(cursor_name(&f.app), "notes.txt");
        click(&mut f.app, cross);
        assert!(f.app.look.is_none());

        press(&mut f.app, &[KeyCode::Char(' ')]);
        draw(&mut f.app);
        click(&mut f.app, (2, 20));
        assert!(f.app.look.is_none(), "outside it");
        assert_eq!(cursor_name(&f.app), "notes.txt", "and nothing else");
        assert_eq!(f.app.pane().cwd, f.home);
    }
    #[test]
    fn a_click_while_a_prompt_asks_does_nothing_and_one_in_the_files_hands_the_search_over() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        let notes = spot_file(&mut f.app, "notes.txt");
        click(&mut f.app, notes);
        right_click(&mut f.app, notes);
        assert!(f.app.prompt.is_some(), "the prompt waits for keys");
        assert!(f.app.menu.is_none());
        assert_eq!(cursor_name(&f.app), "Documents");
        press(&mut f.app, &[KeyCode::Esc]);
        click(&mut f.app, notes);
        assert_eq!(cursor_name(&f.app), "notes.txt");

        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "jpg");
        finish_search(&mut f.app);
        let found = spot_file(&mut f.app, "a.jpg");
        click(&mut f.app, found);
        assert!(f.app.field.is_none(), "the keyboard is with what was found");
        assert!(f.app.pane().results().is_some(), "still showing it");
        assert_eq!(cursor_name(&f.app), "a.jpg");
    }

    #[test]
    fn drags_moves_releases_and_the_middle_button_do_nothing() {
        let mut f = fixture();
        let notes = spot_file(&mut f.app, "notes.txt");
        for kind in [
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
            MouseEventKind::ScrollLeft,
            MouseEventKind::Down(MouseButton::Middle),
        ] {
            mouse(&mut f.app, kind, notes, KeyModifiers::NONE);
        }
        assert_eq!((f.app.pane().cursor, f.app.focus), (0, Focus::List));
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

#[cfg(test)]
mod tab_tests {
    use super::testing::*;
    use super::*;
    use std::fs;

    /// What the footer says, if anything.
    fn said(app: &App) -> Option<&str> {
        app.message.as_ref().map(|message| message.text.as_str())
    }

    /// The folder each tab shows, in the strip's order.
    fn folders(app: &App) -> Vec<PathBuf> {
        let cwd = |tab: &Tab| tab.pane().cwd.clone();
        app.tabs.iter().map(cwd).collect()
    }

    #[test]
    fn t_opens_a_tab_in_the_folder_showing_at_the_end_of_the_strip_and_shows_it() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Enter, KeyCode::Char('t')],
        );
        assert_eq!(folders(&f.app), [pictures.clone(), pictures.clone()]);
        assert_eq!(f.app.current, 1);
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        // From the first tab, the new one still goes after the last.
        press(&mut f.app, &[KeyCode::Char('1'), KeyCode::Char('h')]);
        press(&mut f.app, &[KeyCode::Char('t')]);
        assert_eq!(
            folders(&f.app),
            [f.home.clone(), pictures.clone(), f.home.clone()]
        );
        assert_eq!(f.app.current, 2);
    }

    #[test]
    fn nine_tabs_is_the_most() {
        let mut f = fixture();
        for _ in 0..8 {
            press(&mut f.app, &[KeyCode::Char('t')]);
        }
        assert_eq!((f.app.tabs.len(), f.app.current), (9, 8));
        assert_eq!(said(&f.app), None);
        press(&mut f.app, &[KeyCode::Char('t')]);
        assert_eq!((f.app.tabs.len(), f.app.current), (9, 8));
        assert_eq!(said(&f.app), Some("Nine tabs is the most"));
        // `O` on a folder says the same.
        f.app.message = None;
        press(&mut f.app, &[KeyCode::Char('O')]);
        assert_eq!(f.app.tabs.len(), 9);
        assert_eq!(said(&f.app), Some("Nine tabs is the most"));
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn w_and_ctrl_w_close_the_tab_the_next_takes_its_place_and_the_last_one_quits() {
        let mut f = fixture();
        let (documents, pictures) = (f.home.join("Documents"), f.home.join("Pictures"));
        // Home, Documents, Pictures.
        press(
            &mut f.app,
            &[KeyCode::Char('O'), KeyCode::Char('1'), KeyCode::Down],
        );
        press(&mut f.app, &[KeyCode::Char('O'), KeyCode::Char('2')]);
        assert_eq!(
            folders(&f.app),
            [f.home.clone(), documents.clone(), pictures.clone()]
        );
        assert_eq!(f.app.current, 1);

        press(&mut f.app, &[KeyCode::Char('w')]);
        assert_eq!(folders(&f.app), [f.home.clone(), pictures.clone()]);
        assert_eq!((f.app.current, f.app.pane().cwd.clone()), (1, pictures));
        // The last in the strip closed, the one before it shows.
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        assert_eq!(folders(&f.app), [f.home.clone()]);
        assert_eq!(f.app.current, 0);
        assert!(!f.app.should_quit);
        press(&mut f.app, &[KeyCode::Char('w')]);
        assert!(f.app.should_quit, "closing the last tab quits");
    }

    #[test]
    fn brackets_go_to_the_next_and_previous_tab_round_the_ends() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('t'), KeyCode::Char('t')]);
        assert_eq!(f.app.current, 2);
        press(&mut f.app, &[KeyCode::Char(']')]);
        assert_eq!(f.app.current, 0);
        press(&mut f.app, &[KeyCode::Char('[')]);
        assert_eq!(f.app.current, 2);
        press(&mut f.app, &[KeyCode::Char('['), KeyCode::Char('[')]);
        assert_eq!(f.app.current, 0);
        // With one tab, nothing moves.
        press(&mut f.app, &[KeyCode::Char('w'), KeyCode::Char('w')]);
        press(&mut f.app, &[KeyCode::Char(']'), KeyCode::Char('[')]);
        assert_eq!((f.app.tabs.len(), f.app.current), (1, 0));
    }

    #[test]
    fn braces_move_the_tab_one_place_as_far_as_the_ends_and_it_stays_showing() {
        let mut f = fixture();
        let (documents, pictures) = (f.home.join("Documents"), f.home.join("Pictures"));
        press(
            &mut f.app,
            &[KeyCode::Char('O'), KeyCode::Char('1'), KeyCode::Down],
        );
        press(&mut f.app, &[KeyCode::Char('O'), KeyCode::Char('1')]);
        let home = f.home.clone();
        press(&mut f.app, &[KeyCode::Char('}')]);
        assert_eq!(
            folders(&f.app),
            [documents.clone(), home.clone(), pictures.clone()]
        );
        assert_eq!(f.app.current, 1);
        press(&mut f.app, &[KeyCode::Char('}'), KeyCode::Char('}')]);
        assert_eq!(
            folders(&f.app),
            [documents.clone(), pictures.clone(), home.clone()]
        );
        assert_eq!((f.app.current, f.app.pane().cwd.clone()), (2, home.clone()));
        press(&mut f.app, &[KeyCode::Char('{')]);
        assert_eq!(
            folders(&f.app),
            [documents.clone(), home.clone(), pictures.clone()]
        );
        assert_eq!(f.app.current, 1);
        press(&mut f.app, &[KeyCode::Char('{'), KeyCode::Char('{')]);
        assert_eq!(folders(&f.app), [home.clone(), documents, pictures]);
        assert_eq!(f.app.current, 0);
    }

    #[test]
    fn digits_go_to_that_tab_and_a_missing_one_says_so() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('O')]);
        assert_eq!(f.app.current, 1);
        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!(f.app.current, 0);
        assert_eq!(said(&f.app), None);
        press(&mut f.app, &[KeyCode::Char('5')]);
        assert_eq!(f.app.current, 0);
        assert_eq!(said(&f.app), Some("No tab 5"));
        // A digit is never a count: the cursor stays where it was.
        press(&mut f.app, &[KeyCode::Char('2'), KeyCode::Char('j')]);
        assert_eq!((f.app.current, f.app.pane().cursor), (1, 1));
    }

    #[test]
    fn capital_o_opens_the_folder_under_the_cursor_in_a_new_tab_and_no_file() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('O')]);
        assert_eq!(folders(&f.app), [f.home.clone(), f.home.join("Pictures")]);
        assert_eq!(names(&f.app), ["Trip", "a.jpg"]);
        // The first tab stays where it was, on Pictures.
        assert_eq!(f.app.tabs[0].pane().cursor, 1);

        press(
            &mut f.app,
            &[KeyCode::Char('1'), KeyCode::End, KeyCode::Char('O')],
        );
        assert_eq!(f.app.tabs.len(), 2);
        assert_eq!(f.app.current, 0);
        assert_eq!(said(&f.app), Some("Only a folder opens in a new tab"));
    }

    #[test]
    fn each_tab_keeps_its_own_folder_view_marks_and_history() {
        let mut f = fixture();
        let pictures = f.home.join("Pictures");
        // The first tab: Pictures in the grid, a.jpg marked.
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Right, KeyCode::Char('v')]);
        // The second opens in the same folder, in the list, nothing marked
        // and no history; it goes home.
        press(&mut f.app, &[KeyCode::Char('t')]);
        assert_eq!(f.app.view(), ViewMode::List);
        assert_eq!(f.app.pane().selection().count(), 1);
        assert!(!f.app.pane().is_marked(1));
        press(&mut f.app, &[KeyCode::Char('h')]);
        assert_eq!(f.app.pane().cwd, f.home);

        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!(f.app.view(), ViewMode::Grid);
        assert_eq!(f.app.pane().cwd, pictures);
        assert!(f.app.pane().is_marked(1), "a.jpg is still marked");
        press(&mut f.app, &[KeyCode::Char('H')]);
        assert_eq!(f.app.pane().cwd, f.home, "its own way back");

        press(&mut f.app, &[KeyCode::Char('2')]);
        assert_eq!(f.app.view(), ViewMode::List);
        assert_eq!(f.app.pane().cwd, f.home);
        press(&mut f.app, &[KeyCode::Char('H')]);
        assert_eq!(f.app.pane().cwd, pictures, "its own way back");
        press(&mut f.app, &[KeyCode::Char('H')]);
        assert_eq!(f.app.pane().cwd, pictures, "and no further");
    }

    #[test]
    fn a_tab_lists_its_folder_again_when_it_shows_keeping_its_place() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::Char('t')]);
        fs::write(f.home.join("later.txt"), "").unwrap();
        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", "later.txt", "notes.txt"]
        );
        assert_eq!(
            f.app.selected().map(|entry| entry.name.as_str()),
            Some("notes.txt")
        );
    }

    #[test]
    fn two_panes_belong_to_their_tab_and_ctrl_4_returns_to_the_view_they_came_from() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::Dual, 2));
        // A new tab from two panes opens with one, in the list.
        press(&mut f.app, &[KeyCode::Char('t')]);
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::List, 1));
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        f.app.handle_key(key(KeyCode::Char('|')));
        assert_eq!(f.app.view(), ViewMode::Dual);
        f.app.handle_key(alt(KeyCode::Char('1')));
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::Grid, 1));

        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::Dual, 2));
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::Columns, 1));
        // Ctrl+1 from two panes is the list with one.
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::List, 1));
    }

    #[test]
    fn m_moves_from_one_pane_into_the_other_ones_folder() {
        let mut f = fixture();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::End, KeyCode::Char('y')]);
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        press(
            &mut f.app,
            &[KeyCode::Home, KeyCode::Enter, KeyCode::Char('M')],
        );
        finish_work(&mut f.app);
        assert_eq!(names(&f.app), ["notes.txt"]);
        assert!(!f.home.join("notes.txt").exists());
        assert!(documents.join("notes.txt").exists());
        // The left pane saw it go.
        let left: Vec<&str> = f.app.panes()[0]
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(left, ["Documents", "Pictures"]);
    }
}

#[cfg(test)]
mod places_tests {
    use super::testing::*;
    use super::*;
    use crate::ui::testing::{buffer, find, screen};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Where the keyboard is: the panel, or a pane's list, or the preview.
    fn keyboard(app: &App) -> (Focus, usize) {
        (app.focus, app.active())
    }

    /// The names the panel lists, in order.
    fn listed(app: &App) -> Vec<&str> {
        let places = crate::places::flat(&app.places);
        places.map(|place| place.name.as_str()).collect()
    }

    fn on_disk(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .unwrap()
            .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn tab_goes_round_the_places_the_list_and_the_preview_and_shift_tab_back() {
        let mut f = fixture();
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::Preview, 0));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::Places, 0));
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(keyboard(&f.app), (Focus::Preview, 0));
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(keyboard(&f.app), (Focus::List, 0));

        // Two panes: the panel, the left list, the right one.
        split(&mut f.app);
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::Places, 1));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(keyboard(&f.app), (Focus::Places, 0));

        // With the panel hidden, Tab never stops there.
        f.app.handle_key(ctrl(KeyCode::Char('b')));
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::List, 1));
        f.app.handle_key(key(KeyCode::Tab));
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
    }

    #[test]
    fn in_the_panel_the_cursor_starts_on_this_place_and_moves_with_j_k_g_and_capital_g() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(listed(&f.app), ["Home", "Documents", "Pictures", "Trash"]);
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.app.focus, Focus::Places);
        // On the place holding the folder: Pictures.
        assert_eq!(f.app.places_cursor, 2);
        for (code, cursor) in [
            (KeyCode::Char('j'), 3),
            (KeyCode::Char('j'), 3),
            (KeyCode::Char('k'), 2),
            (KeyCode::Up, 1),
            (KeyCode::Down, 2),
            (KeyCode::Char('g'), 0),
            (KeyCode::Char('G'), 3),
            (KeyCode::Home, 0),
            (KeyCode::End, 3),
        ] {
            f.app.handle_key(key(code));
            assert_eq!(f.app.places_cursor, cursor, "{code:?}");
        }
        // The list's cursor stayed where it was.
        assert_eq!(f.app.pane().cursor, 0);
    }

    #[test]
    fn file_keys_do_nothing_while_the_panel_has_the_keyboard() {
        let mut f = fixture();
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.app.focus, Focus::Places);
        let before = on_disk(&f.home);
        for code in [
            KeyCode::Char('d'),
            KeyCode::Char('d'),
            KeyCode::Delete,
            KeyCode::Char('v'),
            KeyCode::Char(' '),
            KeyCode::Char('m'),
            KeyCode::Char('y'),
            KeyCode::Char('p'),
            KeyCode::Char('U'),
            KeyCode::Char('r'),
            KeyCode::Char('N'),
            KeyCode::Char('/'),
            KeyCode::Char('h'),
        ] {
            f.app.handle_key(key(code));
        }
        f.app.handle_key(ctrl(KeyCode::Char('a')));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&f.home), before);
        assert!(f.app.dialog.is_none() && f.app.menu.is_none() && f.app.look.is_none());
        assert!(f.app.prompt.is_none() && f.app.field.is_none());
        assert_eq!(f.app.pane().selection().count(), 1, "nothing marked");
        assert_eq!(
            (f.app.pane().cwd.clone(), f.app.focus),
            (f.home.clone(), Focus::Places)
        );
        // The window's own keys still work: the views, and quitting.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        assert_eq!(f.app.view(), ViewMode::Grid);
        f.app.handle_key(key(KeyCode::Char('q')));
        assert!(f.app.should_quit);
    }

    #[test]
    fn enter_l_or_right_opens_the_place_and_gives_the_keyboard_to_the_list() {
        let mut f = fixture();
        f.app.handle_key(key(KeyCode::BackTab));
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));
        assert_eq!(f.app.focus, Focus::List);
        // A place in the history: back returns.
        press(&mut f.app, &[KeyCode::Char('H')]);
        assert_eq!(f.app.pane().cwd, f.home);
        for opens in [KeyCode::Char('l'), KeyCode::Right] {
            f.app.navigate(&f.home);
            f.app.handle_key(key(KeyCode::BackTab));
            press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char('j'), opens]);
            assert_eq!(f.app.pane().cwd, f.home.join("Pictures"), "{opens:?}");
            assert_eq!(f.app.focus, Focus::List);
        }
        // In two panes, the pane that had the keyboard goes there.
        split(&mut f.app);
        f.app.handle_key(key(KeyCode::BackTab));
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(keyboard(&f.app), (Focus::Places, 0));
        press(&mut f.app, &[KeyCode::Char('g'), KeyCode::Enter]);
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        assert_eq!(f.app.panes()[0].cwd, f.home);
        assert_eq!(f.app.panes()[1].cwd, f.home.join("Pictures"));
        // Escape gives the keyboard back without going anywhere.
        f.app.handle_key(key(KeyCode::BackTab));
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Esc]);
        assert_eq!(keyboard(&f.app), (Focus::List, 0));
        assert_eq!(f.app.panes()[0].cwd, f.home);
    }

    #[test]
    fn the_trash_opens_from_the_panel_and_one_that_cannot_be_listed_says_how() {
        let mut f = fixture();
        f.app.handle_key(key(KeyCode::BackTab));
        press(&mut f.app, &[KeyCode::Char('G'), KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.trash);
        assert_eq!(f.app.focus, Focus::List);
        assert_eq!(f.app.message, None);

        f.app.navigate(&f.home.join("Pictures"));
        fs::create_dir(&f.trash).unwrap();
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o000)).unwrap();
        f.app.handle_key(key(KeyCode::BackTab));
        press(&mut f.app, &[KeyCode::Char('G'), KeyCode::Enter]);
        fs::set_permissions(&f.trash, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        let message = f.app.message.as_ref().expect("it says why");
        assert_eq!(
            (message.text.as_str(), message.is_error),
            (TRASH_REFUSED, true)
        );
        assert_eq!(f.app.focus, Focus::Places, "the keyboard stays");
    }

    #[test]
    fn the_panel_is_read_on_start_on_ctrl_r_and_when_the_keyboard_enters_it_never_while_drawing() {
        let mut f = fixture();
        assert_eq!(listed(&f.app), ["Home", "Documents", "Pictures", "Trash"]);
        fs::create_dir(f.home.join("Music")).unwrap();
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        screen(&mut f.app, 120, 30);
        f.app.tick();
        assert_eq!(listed(&f.app), ["Home", "Documents", "Pictures", "Trash"]);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        let read = ["Home", "Documents", "Music", "Pictures", "Trash", "Backup"];
        assert_eq!(listed(&f.app), read);
        fs::create_dir(f.home.join("Movies")).unwrap();
        screen(&mut f.app, 120, 30);
        assert_eq!(listed(&f.app), read);
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(
            listed(&f.app),
            [
                "Home",
                "Documents",
                "Music",
                "Pictures",
                "Movies",
                "Trash",
                "Backup"
            ]
        );
    }

    #[test]
    fn a_click_on_a_place_opens_it_and_the_wheel_over_the_panel_scrolls_it() {
        let mut f = fixture();
        let drawn = buffer(&mut f.app, 120, 30);
        let panel = crate::ui::testing::panes(&f.app, 120).panel;
        let (x, y) = find(&drawn, "Pictures", panel).expect("the place");
        f.app.focus = Focus::Preview;
        f.app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(f.app.focus, Focus::List);
        assert_eq!(f.app.pane().back.len(), 1);

        for index in 0..20 {
            fs::create_dir(f.volumes.join(format!("Disk {index:02}"))).unwrap();
        }
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        let top = |app: &mut App| {
            let panel = crate::ui::testing::panes(app, 120).panel;
            let row: String = screen(app, 120, 14)[2]
                .chars()
                .take(panel.end as usize)
                .collect();
            row.trim().to_string()
        };
        assert_eq!(top(&mut f.app), "PLACES");
        for _ in 0..3 {
            f.app.handle_mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 5,
                row: 6,
                modifiers: KeyModifiers::NONE,
            });
        }
        let first = top(&mut f.app);
        assert!(first.ends_with("Pictures"), "{first}");
        // The list's cursor stayed put, and back up the wheel goes.
        assert_eq!(f.app.pane().cursor, 0);
        f.app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 5,
            row: 6,
            modifiers: KeyModifiers::NONE,
        });
        assert!(top(&mut f.app).ends_with("Documents"));
    }
}

#[cfg(test)]
mod fn_key_tests {
    use super::testing::*;
    use super::*;
    use std::fs;

    fn said(app: &App) -> (&str, bool) {
        match &app.message {
            Some(message) => (message.text.as_str(), message.is_error),
            None => ("", false),
        }
    }

    /// The names in a folder on disk, sorted, without the hidden ones.
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

    fn read(path: PathBuf) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// The name under a pane's cursor.
    fn under(app: &App, pane: usize) -> Option<&str> {
        let entry = app.panes()[pane].selected();
        entry.map(|entry| entry.name.as_str())
    }

    fn pane_names(app: &App, pane: usize) -> Vec<&str> {
        let entries = app.panes()[pane].entries.iter();
        entries.map(|entry| entry.name.as_str()).collect()
    }

    /// Puts the active pane's cursor on an item it lists.
    fn select(app: &mut App, name: &str) {
        press(app, &[KeyCode::Home]);
        while under(app, app.active()) != Some(name) {
            let before = app.pane().cursor;
            press(app, &[KeyCode::Down]);
            assert_ne!(app.pane().cursor, before, "{name} isn't listed");
        }
    }

    /// Two panes: the left one on home with the keyboard, the right one
    /// on Documents.
    fn two_panes() -> Fixture {
        let mut f = fixture();
        split(&mut f.app);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter, KeyCode::BackTab]);
        assert_eq!(f.app.active(), 0);
        assert_eq!(f.app.panes()[0].cwd, f.home);
        assert_eq!(f.app.panes()[1].cwd, f.home.join("Documents"));
        f
    }

    #[test]
    fn f5_copies_the_selection_into_the_other_panes_folder_and_selects_it_there() {
        let mut f = two_panes();
        let documents = f.home.join("Documents");
        // Something on the clipboard, which F5 leaves alone.
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Char('y')]);
        // Two marked: they are what is copied.
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::Char('v')]);
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Char('v')]);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(5)]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["Pictures", "notes.txt"]);
        assert_eq!(read(documents.join("Pictures").join("a.jpg")), "jpg");
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(said(&f.app), ("Copied 2 items", false));
        assert!(f.app.message.as_ref().unwrap().undoable);
        // Both panes show the change; what arrived is selected on the right,
        // and the keyboard stays on the left.
        assert_eq!(pane_names(&f.app, 1), ["Pictures", "notes.txt"]);
        assert_eq!(under(&f.app, 1), Some("Pictures"));
        assert_eq!(f.app.active(), 0);

        // Undo takes the copy back.
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert!(on_disk(&documents).is_empty());
        assert_eq!(said(&f.app), ("Undid the copy", false));
        assert!(pane_names(&f.app, 1).is_empty());

        // The clipboard is what `y` took: `p` on the right pastes Pictures.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Char('p')]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["Pictures"]);
        assert_eq!(read(documents.join("Pictures").join("a.jpg")), "jpg");
    }

    #[test]
    fn f6_moves_the_selection_into_the_other_panes_folder_and_selects_it_there() {
        let mut f = two_panes();
        let documents = f.home.join("Documents");
        select(&mut f.app, "Pictures");
        press(&mut f.app, &[KeyCode::Char('y')]);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(6)]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["notes.txt"]);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
        assert_eq!(said(&f.app), ("Moved 1 item.", false));
        assert_eq!(pane_names(&f.app, 0), ["Documents", "Pictures"]);
        assert_eq!(pane_names(&f.app, 1), ["notes.txt"]);
        assert_eq!(under(&f.app, 1), Some("notes.txt"));
        assert_eq!(f.app.active(), 0);

        // Undo brings it back.
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert!(on_disk(&documents).is_empty());
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(said(&f.app), ("Undid the move", false));

        // The clipboard still holds Pictures, copied, not cut.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Char('p')]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["Pictures"]);
        assert!(f.home.join("Pictures").join("a.jpg").exists());
    }

    #[test]
    fn f5_and_f6_with_nothing_selected_or_outside_a_folder_do_nothing() {
        let mut f = fixture();
        split(&mut f.app);
        // The left pane in the empty Documents, the right one on home.
        press(&mut f.app, &[KeyCode::BackTab]);
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.pane().entries.is_empty());
        press(&mut f.app, &[KeyCode::F(5), KeyCode::F(6), KeyCode::F(8)]);
        assert_eq!(f.app.working(), None);
        assert!(f.app.question.is_none() && f.app.dialog.is_none());
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures", "notes.txt"]);
    }

    #[test]
    fn f5_and_f6_meeting_a_name_ask_the_paste_question() {
        let mut f = two_panes();
        let documents = f.home.join("Documents");
        fs::write(documents.join("notes.txt"), "old").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::BackTab]);
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(5)]);
        let asked = f.app.question.as_ref().map(PasteQuestion::text);
        assert_eq!(
            asked.as_deref(),
            Some("notes.txt already exists in Documents.")
        );
        assert_eq!(f.app.working(), None, "nothing is written yet");
        assert_eq!(read(documents.join("notes.txt")), "old");
        // Keep both: the copy under its copy name, selected on the right.
        press(&mut f.app, &[KeyCode::Char('b')]);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&documents), ["notes copy.txt", "notes.txt"]);
        assert_eq!(read(documents.join("notes copy.txt")), "hello");
        assert_eq!(read(documents.join("notes.txt")), "old");
        assert_eq!(under(&f.app, 1), Some("notes copy.txt"));

        // F6 asks too; skip leaves both where they are.
        press(&mut f.app, &[KeyCode::F(6)]);
        let question = f.app.question.as_ref().expect("the question");
        assert!(question.moving);
        press(&mut f.app, &[KeyCode::Char('s')]);
        finish_work(&mut f.app);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(read(documents.join("notes.txt")), "old");
        // Replace: the one there to the Trash, undoable.
        press(&mut f.app, &[KeyCode::F(6), KeyCode::Char('r')]);
        finish_work(&mut f.app);
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(read(f.trash.join("notes.txt")), "old");
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(read(documents.join("notes.txt")), "old");
    }

    #[test]
    fn f7_asks_for_a_folder_name_in_the_active_panes_folder() {
        let mut f = two_panes();
        press(&mut f.app, &[KeyCode::Tab, KeyCode::F(7)]);
        let documents = f.home.join("Documents");
        let prompt = f.app.prompt.as_ref().expect("the prompt");
        assert_eq!(prompt.asking, Asking::NewFolder(documents.clone()));
        assert_eq!(prompt.input.text(), "New Folder");
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(documents.join("New Folder").is_dir());
        assert_eq!(under(&f.app, 1), Some("New Folder"));
        assert_eq!(said(&f.app), ("Folder created", false));
    }

    #[test]
    fn f8_moves_the_selection_to_the_trash_and_undo_brings_it_back() {
        let mut f = two_panes();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(8)]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert!(!f.home.join("notes.txt").exists());
        assert_eq!(said(&f.app), ("Moved 1 item to Trash", false));
        assert_eq!(pane_names(&f.app, 0), ["Documents", "Pictures"]);
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(said(&f.app), ("Undid the move to Trash", false));
    }

    #[test]
    fn f9_and_f10_make_an_empty_file_under_the_next_free_name_and_ask_its_name() {
        let mut f = two_panes();
        press(&mut f.app, &[KeyCode::F(9)]);
        let made = f.home.join("new.md");
        assert_eq!(read(made.clone()), "");
        assert_eq!(under(&f.app, 0), Some("new.md"));
        let prompt = f.app.prompt.as_ref().expect("the rename prompt");
        assert_eq!(prompt.asking, Asking::Rename(made.clone()));
        assert_eq!(prompt.input.text(), "new.md");
        assert_eq!(prompt.input.selection(), Some((0, 3)), "the name, not .md");
        press(&mut f.app, &[KeyCode::Esc]);

        // Taken: the next free name.
        press(&mut f.app, &[KeyCode::F(9), KeyCode::Esc]);
        fs::write(f.home.join("new 3.md"), "kept").unwrap();
        press(&mut f.app, &[KeyCode::F(9), KeyCode::Esc]);
        assert_eq!(read(f.home.join("new 3.md")), "kept", "never written over");
        assert_eq!(read(f.home.join("new 4.md")), "");
        assert_eq!(under(&f.app, 0), Some("new 4.md"));
        press(&mut f.app, &[KeyCode::F(10)]);
        assert_eq!(read(f.home.join("new.txt")), "");
        let prompt = f.app.prompt.as_ref().expect("the rename prompt");
        assert_eq!(prompt.asking, Asking::Rename(f.home.join("new.txt")));
        // Typing names it.
        press(&mut f.app, &[KeyCode::Char('t'), KeyCode::Enter]);
        assert!(f.home.join("t.txt").exists());
        assert!(!f.home.join("new.txt").exists());
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert!(f.home.join("new.txt").exists());

        // Undo removes the new file, to the Trash.
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert!(!f.home.join("new.txt").exists());
        assert_eq!(said(&f.app), ("Undid the new file", false));
        assert!(f.trash.join("new.txt").exists());
        // The other pane didn't move.
        assert_eq!(f.app.panes()[1].cwd, f.home.join("Documents"));
    }

    #[test]
    fn outside_two_panes_f5_to_f10_do_nothing_and_ctrl_r_refreshes_everywhere() {
        let mut f = fixture();
        fs::write(f.home.join("late.txt"), "late").unwrap();
        select(&mut f.app, "notes.txt");
        for number in 5..=10 {
            f.app.handle_key(key(KeyCode::F(number)));
            assert_eq!(f.app.working(), None, "F{number}");
            assert!(
                f.app.prompt.is_none() && f.app.question.is_none(),
                "F{number}"
            );
            assert!(f.app.dialog.is_none() && f.app.menu.is_none(), "F{number}");
        }
        // F5 read nothing again: late.txt isn't listed yet.
        assert_eq!(names(&f.app), ["Documents", "Pictures", "notes.txt"]);
        assert_eq!(
            on_disk(&f.home),
            ["Documents", "Pictures", "late.txt", "notes.txt"]
        );
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert_eq!(
            names(&f.app),
            ["Documents", "Pictures", "late.txt", "notes.txt"]
        );
        // Shift+F10 still opens the menu.
        f.app.handle_key(shift(KeyCode::F(10)));
        assert!(f.app.menu.is_some());
        press(&mut f.app, &[KeyCode::Esc]);

        // In two panes Ctrl+R reads again, from a pane and from the panel.
        split(&mut f.app);
        fs::write(f.home.join("later.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert!(names(&f.app).contains(&"later.txt".to_string()));
        fs::write(f.home.join("latest.txt"), "").unwrap();
        press(&mut f.app, &[KeyCode::BackTab, KeyCode::BackTab]);
        assert_eq!(f.app.focus, Focus::Places);
        // F5 from the panel does nothing at all.
        press(&mut f.app, &[KeyCode::F(5)]);
        assert_eq!(f.app.working(), None);
        assert!(!names(&f.app).contains(&"latest.txt".to_string()));
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert!(names(&f.app).contains(&"latest.txt".to_string()));
        assert_eq!(f.app.focus, Focus::Places);
        // And Shift+F10 opens the menu in two panes too.
        press(&mut f.app, &[KeyCode::Esc]);
        f.app.handle_key(shift(KeyCode::F(10)));
        assert!(f.app.menu.is_some());
    }

    #[test]
    fn f2_in_two_panes_still_asks_for_the_new_name() {
        let mut f = two_panes();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(2)]);
        let prompt = f.app.prompt.as_ref().expect("the rename prompt");
        assert_eq!(prompt.asking, Asking::Rename(f.home.join("notes.txt")));
    }

    fn left_click(app: &mut App, at: (u16, u16)) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.0,
            row: at.1,
            modifiers: KeyModifiers::NONE,
        });
    }

    /// A click on an item of the bar, as the last frame drew it.
    fn click_bar(app: &mut App, command: FnKey) {
        crate::ui::testing::buffer(app, 120, 30);
        let rect = app.hits.rect_of(&Spot::FnKey(command)).expect("on the bar");
        left_click(app, (rect.x + rect.width / 2, rect.y));
    }

    /// The colours the bar is drawn in, from a 120 by 30 frame: the last
    /// row of the panes' area, across the panes' columns.
    fn bar_colours(app: &mut App) -> Vec<ratatui::style::Color> {
        let drawn = crate::ui::testing::buffer(app, 120, 30);
        let files = crate::ui::testing::panes(app, 120).files;
        files.map(|x| drawn[(x, 27)].fg).collect()
    }

    fn message(app: &App) -> Option<String> {
        app.message.as_ref().map(|message| message.text.clone())
    }

    #[test]
    fn f8_in_the_trash_does_nothing_and_its_item_is_muted() {
        let mut f = two_panes();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(8)]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        // The left pane in the Trash, on what was moved there.
        let trash = f.trash.clone();
        f.app.navigate(&trash);
        select(&mut f.app, "notes.txt");
        assert!(!f.app.can_run(FnKey::Trash));
        // F8 Delete all muted; the other items as before.
        let drawn = crate::ui::testing::buffer(&mut f.app, 120, 30);
        let (x, y) = crate::ui::testing::find(&drawn, "F8 Delete", 0..120).expect("F8");
        assert_eq!(y, 27);
        for dx in 0.."F8 Delete".len() as u16 {
            assert_eq!(drawn[(x + dx, y)].fg, crate::theme::MUTED, "cell {dx}");
        }
        let (x, _) = crate::ui::testing::find(&drawn, "F5 Copy", 0..120).expect("F5");
        assert_eq!(drawn[(x, y)].fg, crate::theme::ink(crate::theme::ACCENT));
        // The key and a click on the item: no card, no message, nothing moved.
        let said = message(&f.app);
        press(&mut f.app, &[KeyCode::F(8)]);
        click_bar(&mut f.app, FnKey::Trash);
        assert!(f.app.dialog.is_none());
        assert_eq!(message(&f.app), said);
        assert_eq!(f.app.working(), None);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
        assert_eq!(pane_names(&f.app, 0), ["notes.txt"]);
        // The Delete key still asks to delete it for good.
        press(&mut f.app, &[KeyCode::Delete]);
        assert!(matches!(f.app.dialog, Some(Dialog::Delete { .. })));
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(read(f.trash.join("notes.txt")), "hello");
    }

    #[test]
    fn what_f5_brought_is_selected_in_the_pane_it_was_sent_to_after_tab() {
        let mut f = two_panes();
        let documents = f.home.join("Documents");
        fs::write(documents.join("a.txt"), "a").unwrap();
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::BackTab]);
        select(&mut f.app, "notes.txt");
        let left = f.app.panes()[0].cursor;
        press(&mut f.app, &[KeyCode::F(5), KeyCode::Tab]);
        assert_eq!(f.app.active(), 1);
        finish_work(&mut f.app);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(under(&f.app, 1), Some("notes.txt"));
        assert_eq!(f.app.panes()[0].cursor, left);
        assert_eq!(under(&f.app, 0), Some("notes.txt"));
    }

    #[test]
    fn what_f5_brought_is_selected_only_in_the_pane_it_was_sent_to() {
        let mut f = two_panes();
        let documents = f.home.join("Documents");
        fs::write(documents.join("a.txt"), "a").unwrap();
        select(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(5)]);
        // Before it lands, the left pane goes into Documents too.
        select(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(under(&f.app, 0), Some("a.txt"));
        finish_work(&mut f.app);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(pane_names(&f.app, 0), ["a.txt", "notes.txt"]);
        assert_eq!(under(&f.app, 0), Some("a.txt"), "the left pane stays");
        assert_eq!(under(&f.app, 1), Some("notes.txt"));
    }

    #[test]
    fn what_f5_brought_is_not_selected_in_a_tab_opened_meanwhile() {
        let mut f = two_panes();
        let documents = f.home.join("Documents");
        fs::write(documents.join("a.txt"), "a").unwrap();
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::BackTab]);
        select(&mut f.app, "notes.txt");
        let left = f.app.panes()[0].cursor;
        press(&mut f.app, &[KeyCode::F(5)]);
        // A new tab on Documents before it lands.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Char('t')]);
        assert_eq!(f.app.current, 1);
        assert_eq!(f.app.pane().cwd, documents);
        assert_eq!(under(&f.app, 0), Some("a.txt"));
        finish_work(&mut f.app);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert_eq!(pane_names(&f.app, 0), ["a.txt", "notes.txt"]);
        assert_eq!(under(&f.app, 0), Some("a.txt"), "the new tab's pane stays");
        // Back in the first tab: there it is selected, the left pane as it was.
        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!(f.app.current, 0);
        assert_eq!(under(&f.app, 1), Some("notes.txt"));
        assert_eq!(f.app.panes()[0].cursor, left);
    }

    /// Nothing marked in any pane of the tab showing.
    fn nothing_marked(app: &App) -> bool {
        app.panes().iter().all(|pane| pane.marks.is_empty())
    }

    /// Two panes with Documents holding `a.txt` and listing it on the
    /// right, the left pane's cursor on notes.txt.
    fn two_panes_on_notes() -> Fixture {
        let mut f = two_panes();
        fs::write(f.home.join("Documents").join("a.txt"), "a").unwrap();
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::BackTab]);
        select(&mut f.app, "notes.txt");
        assert_eq!(under(&f.app, 1), Some("a.txt"));
        f
    }

    #[test]
    fn what_f5_brought_is_selected_nowhere_when_its_tab_was_closed() {
        let mut f = two_panes_on_notes();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::F(5)]);
        // Another tab on Documents, in two panes like the first, then the
        // first tab closed before the copy lands.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Char('t')]);
        split(&mut f.app);
        assert_eq!(f.app.current, 1);
        assert_eq!(f.app.panes()[1].cwd, documents);
        press(&mut f.app, &[KeyCode::Char('1'), KeyCode::Char('w')]);
        assert_eq!(f.app.tabs.len(), 1);
        assert_eq!(f.app.current, 0);
        assert_eq!(f.app.active(), 1);
        let cursors: Vec<_> = f.app.panes().iter().map(|pane| pane.cursor).collect();
        finish_work(&mut f.app);
        assert_eq!(f.app.working(), None);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(on_disk(&documents), ["a.txt", "notes.txt"]);
        // The tab left lists the copy in both its panes, and neither moved.
        assert_eq!(pane_names(&f.app, 0), ["a.txt", "notes.txt"]);
        assert_eq!(pane_names(&f.app, 1), ["a.txt", "notes.txt"]);
        let now: Vec<_> = f.app.panes().iter().map(|pane| pane.cursor).collect();
        assert_eq!(now, cursors);
        assert_eq!(under(&f.app, 0), Some("a.txt"));
        assert_eq!(under(&f.app, 1), Some("a.txt"));
        assert!(nothing_marked(&f.app));
        assert_eq!(said(&f.app), ("Copied 1 item", false));
    }

    #[test]
    fn what_f5_brought_is_selected_nowhere_when_the_split_was_closed() {
        let mut f = two_panes_on_notes();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::F(5)]);
        // Back to one pane before the copy lands.
        press(&mut f.app, &[KeyCode::Char('|')]);
        assert_eq!(f.app.panes().len(), 1);
        let left = f.app.panes()[0].cursor;
        finish_work(&mut f.app);
        assert_eq!(f.app.working(), None);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        assert_eq!(on_disk(&documents), ["a.txt", "notes.txt"]);
        // The one pane left stays on home, where it was.
        assert_eq!(f.app.panes().len(), 1);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(f.app.panes()[0].cursor, left);
        assert_eq!(under(&f.app, 0), Some("notes.txt"));
        assert!(nothing_marked(&f.app));
        assert_eq!(said(&f.app), ("Copied 1 item", false));
    }

    #[test]
    fn what_f5_brought_is_selected_nowhere_when_its_pane_left_the_folder() {
        let mut f = two_panes_on_notes();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::F(5)]);
        // The right pane up to home, which lists a notes.txt of its own,
        // before the copy lands; the keyboard back on the left.
        press(
            &mut f.app,
            &[KeyCode::Tab, KeyCode::Backspace, KeyCode::BackTab],
        );
        assert_eq!(f.app.panes()[1].cwd, f.home);
        assert_eq!(under(&f.app, 1), Some("Documents"));
        let cursors: Vec<_> = f.app.panes().iter().map(|pane| pane.cursor).collect();
        finish_work(&mut f.app);
        assert_eq!(f.app.working(), None);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(on_disk(&documents), ["a.txt", "notes.txt"]);
        // Neither pane moved: the right one stays on Documents, not on
        // home's notes.txt.
        assert_eq!(f.app.panes()[1].cwd, f.home);
        let now: Vec<_> = f.app.panes().iter().map(|pane| pane.cursor).collect();
        assert_eq!(now, cursors);
        assert_eq!(under(&f.app, 0), Some("notes.txt"));
        assert_eq!(under(&f.app, 1), Some("Documents"));
        assert_eq!(f.app.active(), 0);
        assert!(nothing_marked(&f.app));
        // Going into Documents, the copy is there.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Enter]);
        assert_eq!(pane_names(&f.app, 1), ["a.txt", "notes.txt"]);
    }

    #[test]
    fn what_f5_brought_is_selected_nowhere_when_its_pane_turned_to_a_find() {
        // A find from home, which the pane went up to, and one from
        // Documents itself: neither is the folder the copy was sent to.
        for up in [true, false] {
            let mut f = two_panes_on_notes();
            let documents = f.home.join("Documents");
            // Found first when the find is in Documents.
            fs::write(documents.join("notes.md"), "md").unwrap();
            let copy = documents.join("notes.txt");
            press(&mut f.app, &[KeyCode::F(5)]);
            // The copy on disk, not yet taken in.
            let deadline = Instant::now() + Duration::from_secs(20);
            while fs::read_to_string(&copy).ok().as_deref() != Some("hello") {
                assert!(Instant::now() < deadline, "the copy never landed");
                std::thread::sleep(Duration::from_millis(2));
            }
            press(&mut f.app, &[KeyCode::Tab]);
            if up {
                press(&mut f.app, &[KeyCode::Backspace]);
            }
            let left = f.app.panes()[0].cursor;
            f.app.handle_key(key(KeyCode::Char('f')));
            type_text(&mut f.app, "notes");
            finish_search(&mut f.app);
            finish_work(&mut f.app);
            finish_search(&mut f.app);
            assert_eq!(f.app.working(), None, "up {up}");
            assert_eq!(read(copy.clone()), "hello", "up {up}");
            assert_eq!(read(f.home.join("notes.txt")), "hello", "up {up}");
            // The find lists the copy; its cursor stays on the first found,
            // and the left pane where it was.
            let found = f.app.panes()[1].entries.iter();
            let found: Vec<_> = found.map(|entry| entry.path.clone()).collect();
            let mut listed = vec![documents.join("notes.md"), copy.clone()];
            if up {
                listed.insert(0, f.home.join("notes.txt"));
            }
            assert_eq!(found, listed, "up {up}");
            assert_eq!(f.app.panes()[1].cursor, 0, "up {up}");
            assert_eq!(f.app.panes()[0].cursor, left, "up {up}");
            assert_eq!(under(&f.app, 0), Some("notes.txt"), "up {up}");
            assert!(nothing_marked(&f.app), "up {up}");
        }
    }

    #[test]
    fn what_f6_moved_is_selected_in_the_pane_it_was_sent_to_after_tab() {
        let mut f = two_panes_on_notes();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::F(6), KeyCode::Tab]);
        assert_eq!(f.app.active(), 1);
        finish_work(&mut f.app);
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
        assert_eq!(on_disk(&documents), ["a.txt", "notes.txt"]);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        assert_eq!(said(&f.app), ("Moved 1 item.", false));
        // Selected on the right, which has the keyboard now.
        assert_eq!(f.app.active(), 1);
        assert_eq!(pane_names(&f.app, 1), ["a.txt", "notes.txt"]);
        assert_eq!(under(&f.app, 1), Some("notes.txt"));
        // The left pane no longer lists it; its cursor on what is left.
        assert_eq!(pane_names(&f.app, 0), ["Documents", "Pictures"]);
        assert!(f.app.panes()[0].cursor < f.app.panes()[0].entries.len());
        assert_eq!(under(&f.app, 0), Some("Pictures"));
        assert!(nothing_marked(&f.app));
    }

    #[test]
    fn what_f6_moved_is_selected_in_its_pane_once_its_tab_shows_again() {
        let mut f = two_panes_on_notes();
        let documents = f.home.join("Documents");
        press(&mut f.app, &[KeyCode::F(6)]);
        // A new tab on home before the move lands.
        press(&mut f.app, &[KeyCode::Char('t')]);
        assert_eq!(f.app.current, 1);
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(under(&f.app, 0), Some("Documents"));
        finish_work(&mut f.app);
        assert_eq!(on_disk(&f.home), ["Documents", "Pictures"]);
        assert_eq!(on_disk(&documents), ["a.txt", "notes.txt"]);
        assert_eq!(read(documents.join("notes.txt")), "hello");
        // The new tab's pane lists what is left, its cursor where it was.
        assert_eq!(pane_names(&f.app, 0), ["Documents", "Pictures"]);
        assert_eq!(under(&f.app, 0), Some("Documents"));
        assert!(nothing_marked(&f.app));
        // Back in the first tab: selected on the right, the left pane's
        // cursor on what is left.
        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!(f.app.current, 0);
        assert_eq!(f.app.active(), 0);
        assert_eq!(pane_names(&f.app, 1), ["a.txt", "notes.txt"]);
        assert_eq!(under(&f.app, 1), Some("notes.txt"));
        assert_eq!(pane_names(&f.app, 0), ["Documents", "Pictures"]);
        assert!(f.app.panes()[0].cursor < f.app.panes()[0].entries.len());
        assert_eq!(under(&f.app, 0), Some("Pictures"));
        assert!(nothing_marked(&f.app));
    }

    #[test]
    fn while_a_menu_or_the_settings_are_open_the_bar_is_muted_and_its_clicks_run_nothing() {
        let mut f = two_panes();
        select(&mut f.app, "notes.txt");
        assert!(FnKey::ALL.iter().all(|command| f.app.can_run(*command)));
        let accent = crate::theme::ink(crate::theme::ACCENT);
        assert!(bar_colours(&mut f.app).contains(&accent));

        // The menu.
        f.app.handle_key(shift(KeyCode::F(10)));
        assert!(f.app.menu.is_some());
        assert!(FnKey::ALL.iter().all(|command| !f.app.can_run(*command)));
        assert!(!bar_colours(&mut f.app).contains(&accent));
        // A click on the bar only closes it, as one anywhere outside it does.
        click_bar(&mut f.app, FnKey::NewMarkdown);
        assert!(f.app.menu.is_none() && f.app.prompt.is_none());
        assert!(!f.home.join("new.md").exists());

        // The settings card.
        press(&mut f.app, &[KeyCode::Char(',')]);
        assert!(f.app.settings.is_some());
        assert!(FnKey::ALL.iter().all(|command| !f.app.can_run(*command)));
        assert!(!bar_colours(&mut f.app).contains(&accent));
        click_bar(&mut f.app, FnKey::NewMarkdown);
        assert!(f.app.settings.is_none() && f.app.prompt.is_none());
        assert!(!f.home.join("new.md").exists());

        // The key sheet. It covers the panes' area, the bar's row with it,
        // all but the end of F10's share.
        press(&mut f.app, &[KeyCode::Char('?')]);
        assert!(f.app.help_visible);
        assert!(FnKey::ALL.iter().all(|command| !f.app.can_run(*command)));
        assert!(!bar_colours(&mut f.app).contains(&accent));
        // A click on F8, under the sheet, runs nothing and leaves it open.
        click_bar(&mut f.app, FnKey::Trash);
        assert!(f.app.help_visible);
        assert_eq!(read(f.home.join("notes.txt")), "hello");
        // One on F10, right of the sheet, only closes it, as one outside
        // it does.
        click_bar(&mut f.app, FnKey::NewText);
        assert!(!f.app.help_visible && f.app.prompt.is_none());
        assert!(!f.home.join("new.txt").exists());
        assert_eq!(read(f.home.join("notes.txt")), "hello");

        // Nothing open: lit again.
        assert!(bar_colours(&mut f.app).contains(&accent));
    }

    #[test]
    fn redo_of_f9_brings_back_an_empty_file_and_of_f7_a_folder() {
        let mut f = two_panes();
        let made = f.home.join("new.md");
        press(&mut f.app, &[KeyCode::F(9), KeyCode::Esc]);
        assert!(made.is_file());
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert!(!made.exists());
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert!(made.is_file(), "a file, not a folder");
        assert_eq!(read(made.clone()), "");
        assert!(!said(&f.app).1);
        assert!(pane_names(&f.app, 0).contains(&"new.md"));

        let folder = f.home.join("New Folder");
        press(&mut f.app, &[KeyCode::F(7), KeyCode::Enter]);
        assert!(folder.is_dir());
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert!(!folder.exists());
        f.app.handle_key(ctrl(KeyCode::Char('y')));
        assert!(folder.is_dir());
        assert!(made.is_file());
    }
}

/// What F5, F6 and F8 must never do to the files, whatever the panes show:
/// lose them, write over them, or put a folder inside itself.
#[cfg(test)]
mod fn_key_safety_tests {
    use super::testing::*;
    use super::*;
    use std::fs;

    /// Every file and folder under `root`, hidden ones too, with contents;
    /// the Trash left out, where undo and Replace put things.
    fn disk(root: &Path) -> Vec<String> {
        fn walk(root: &Path, at: &Path, out: &mut Vec<String>) {
            let mut items: Vec<_> = (fs::read_dir(at).unwrap())
                .map(|entry| entry.unwrap().path())
                .collect();
            items.sort();
            for path in items {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if name == ".Trash" {
                    continue;
                }
                if fs::symlink_metadata(&path).unwrap().is_dir() {
                    out.push(format!("{name}/"));
                    walk(root, &path, out);
                } else {
                    out.push(format!(
                        "{name}={}",
                        fs::read_to_string(&path).unwrap_or_default()
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out
    }

    fn said(app: &App) -> &str {
        app.message
            .as_ref()
            .map_or("", |message| message.text.as_str())
    }

    /// The cursor on an item of the active pane, by name.
    fn point_at(app: &mut App, name: &str) {
        press(app, &[KeyCode::Home]);
        while app.selected().map(|entry| entry.name.clone()).as_deref() != Some(name) {
            let before = app.pane().cursor;
            press(app, &[KeyCode::Down]);
            assert_ne!(app.pane().cursor, before, "{name} isn't listed");
        }
    }

    /// Two panes: the left one on home with the keyboard, the right one on
    /// `dir` under home.
    fn two_panes(dir: &str) -> Fixture {
        let mut f = fixture();
        split(&mut f.app);
        for part in dir.split('/').filter(|part| !part.is_empty()) {
            point_at(&mut f.app, part);
            press(&mut f.app, &[KeyCode::Enter]);
        }
        press(&mut f.app, &[KeyCode::BackTab]);
        assert_eq!(f.app.active(), 0);
        f
    }

    #[test]
    fn with_both_panes_on_one_folder_nothing_is_lost_or_written_over() {
        let mut f = two_panes("");
        let home = f.home.clone();
        let start = disk(&home);
        // Copying onto itself asks; Replace and Skip leave the files alone.
        for answer in ['r', 's'] {
            point_at(&mut f.app, "notes.txt");
            press(&mut f.app, &[KeyCode::F(5)]);
            assert!(f.app.question.is_some());
            press(&mut f.app, &[KeyCode::Char(answer)]);
            finish_work(&mut f.app);
            assert_eq!(disk(&home), start, "answered {answer}");
        }
        // Keep both makes a copy, which undo takes away.
        point_at(&mut f.app, "notes.txt");
        press(&mut f.app, &[KeyCode::F(5), KeyCode::Char('b')]);
        finish_work(&mut f.app);
        assert!(home.join("notes copy.txt").exists());
        press(&mut f.app, &[KeyCode::Char('z')]);
        assert_eq!(disk(&home), start);
        // Moving into the folder it is in is refused, a file or a folder.
        for name in ["notes.txt", "Pictures"] {
            point_at(&mut f.app, name);
            press(&mut f.app, &[KeyCode::F(6)]);
            finish_work(&mut f.app);
            assert_eq!(said(&f.app), "It is already in this folder.", "{name}");
            assert_eq!(disk(&home), start, "{name}");
        }
    }

    #[test]
    fn a_folder_is_never_copied_or_moved_into_itself() {
        for dir in ["Pictures", "Pictures/Trip"] {
            for key in [5, 6] {
                let mut f = two_panes(dir);
                let home = f.home.clone();
                fs::write(home.join("Pictures/Trip/t.txt"), "t").unwrap();
                f.app.handle_key(ctrl(KeyCode::Char('r')));
                let start = disk(&home);
                point_at(&mut f.app, "Pictures");
                press(&mut f.app, &[KeyCode::F(key)]);
                if f.app.question.is_some() {
                    press(&mut f.app, &[KeyCode::Char('r')]);
                }
                finish_work(&mut f.app);
                assert_eq!(
                    said(&f.app),
                    "Can't put Pictures inside itself.",
                    "F{key} {dir}"
                );
                assert_eq!(disk(&home), start, "F{key} into {dir}");
            }
        }
    }

    #[test]
    fn where_the_keys_have_nothing_to_do_the_files_stay_as_they_were() {
        // Two panes, the active one an empty folder: nothing to copy, move or trash.
        let mut f = fixture();
        let home = f.home.clone();
        split(&mut f.app);
        point_at(&mut f.app, "Documents");
        press(&mut f.app, &[KeyCode::Enter]);
        let start = disk(&home);
        for key in [5, 6, 8] {
            press(&mut f.app, &[KeyCode::F(key)]);
            finish_work(&mut f.app);
        }
        assert_eq!(disk(&home), start);
        assert!(f.app.question.is_none() && f.app.dialog.is_none());
        // One pane, in every view, a file under the cursor: F5 to F10 do nothing.
        let mut f = fixture();
        let home = f.home.clone();
        point_at(&mut f.app, "notes.txt");
        let start = disk(&home);
        for view in ['1', '2', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            for key in 5..=10 {
                press(&mut f.app, &[KeyCode::F(key)]);
                finish_work(&mut f.app);
                let asking = f.app.prompt.is_some() || f.app.question.is_some();
                assert!(!asking && f.app.dialog.is_none(), "F{key} in view {view}");
            }
            assert_eq!(disk(&home), start, "view {view}");
        }
    }
}
