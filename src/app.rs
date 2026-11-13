//! Application state and what each key does to it.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::format::DateStyle;
use crate::input::Input;
use crate::model::{self, Entry, SortMode};
use crate::ops::{self, Change, History, Kind, Report};
use crate::places::{self, Place, Section};
use crate::state::{self, Saved};
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

/// A folder in the history, with what was selected there.
#[derive(Clone, Debug)]
struct Visit {
    path: PathBuf,
    selected: Option<String>,
}

/// A folder above the pane's own, as the columns view lists it.
pub struct Ancestor {
    pub entries: Vec<Entry>,
    /// The entry that leads down to the pane's folder, when it is listed.
    pub on_path: Option<usize>,
}

pub struct Pane {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    /// First visible row (of tiles, in the grid); the view keeps the cursor on screen with it.
    pub offset: usize,
    /// The folders from home (or `/`) down to this one's parent. Only the columns
    /// view needs them, so they are listed when a folder is shown in that view.
    pub ancestors: Vec<Ancestor>,
    back: Vec<Visit>,
    forward: Vec<Visit>,
}

impl Pane {
    fn new(cwd: PathBuf, entries: Vec<Entry>) -> Self {
        Self {
            cwd,
            entries,
            cursor: 0,
            offset: 0,
            ancestors: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    fn visit(&self) -> Visit {
        Visit {
            path: self.cwd.clone(),
            selected: self.selected().map(|entry| entry.name.clone()),
        }
    }

    fn show(&mut self, cwd: PathBuf, entries: Vec<Entry>, select: Option<&str>) {
        self.cwd = cwd;
        self.entries = entries;
        self.offset = 0;
        self.cursor = select
            .and_then(|name| self.entries.iter().position(|entry| entry.name == name))
            .unwrap_or(0);
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.entries.len().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
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
    /// Where the view and tile size are remembered; `None` remembers nothing.
    pub state_dir: Option<PathBuf>,
    /// What makes the thumbnails; `None` leaves every file its stand-in.
    pub thumbs: Option<Thumbs>,
    /// Where Delete moves things: the trash folder.
    pub trash_dir: PathBuf,
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
    pub dialog: Option<Dialog>,
    pub message: Option<Message>,
    pub should_quit: bool,
    pub date_style: DateStyle,
    /// Rows the file view showed last time, for Page Up and Page Down.
    pub page_rows: usize,
    /// Tiles across the grid last time, for Up and Down.
    pub grid_columns: usize,
    pub thumbs: Option<Thumbs>,
    /// What was read about the selected item for the details pane.
    pub facts: Option<Facts>,
    clipboard: Option<Clipboard>,
    history: History,
    work: Option<Work>,
    trash_dir: PathBuf,
    state_dir: Option<PathBuf>,
    system: Box<dyn System>,
}

impl App {
    pub fn new(config: Config, system: Box<dyn System>) -> Self {
        let saved = config
            .state_dir
            .as_deref()
            .map(state::load)
            .unwrap_or_default();
        let mut app = Self {
            sidebar: vec![places::home_section(&config.home)],
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
            dialog: None,
            message: None,
            should_quit: false,
            date_style: config.date_style,
            page_rows: 10,
            grid_columns: 1,
            thumbs: config.thumbs,
            facts: None,
            clipboard: None,
            history: History::default(),
            work: None,
            trash_dir: config.trash_dir,
            state_dir: config.state_dir,
            system,
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

    /// The sidebar place the active pane is in: the deepest one holding its folder.
    pub fn current_place(&self) -> Option<usize> {
        let cwd = &self.pane().cwd;
        self.places()
            .enumerate()
            .filter(|(_, place)| cwd.starts_with(&place.path))
            .max_by_key(|(_, place)| place.path.components().count())
            .map(|(index, _)| index)
    }

    /// What the file work under way is doing, in the band's words.
    pub fn working(&self) -> Option<&str> {
        self.work.as_ref().map(|work| work.label.as_str())
    }

    /// What every turn of the event loop does besides keys: takes in the file
    /// work that finished and the thumbnails made since the last one, and
    /// reads what the details pane says about a newly selected item, so that
    /// drawing never has to.
    pub fn tick(&mut self) {
        self.collect_work();
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
        if self.help_visible {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help_visible = false;
            }
            return;
        }
        if let Some(dialog) = self.dialog.take() {
            self.dialog_key(dialog, key);
            return;
        }
        if self.file_key(key) {
            return;
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = !alt && key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !alt && !ctrl;
        match key.code {
            KeyCode::Char('?') => self.help_visible = true,
            KeyCode::Char('q' | 'Q') if plain => self.quit(),
            KeyCode::Esc => self.stop_or_dismiss(),
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Left if alt => self.go_back(),
            KeyCode::Right if alt => self.go_forward(),
            // What Ghostty and Terminal send for Option+arrows on a Mac.
            KeyCode::Char('b') if alt => self.go_back(),
            KeyCode::Char('f') if alt => self.go_forward(),
            KeyCode::Home if alt => self.go_home(),
            KeyCode::F(5) => self.reload(),
            KeyCode::Backspace if plain => self.go_up(),
            KeyCode::Char(' ') if plain => self.preview_selected(),
            KeyCode::Char('s') if plain => self.set_sort(self.sort.step(1)),
            KeyCode::Char('S') if plain => self.set_sort(self.sort.step(-1)),
            KeyCode::Char('1') if ctrl => self.set_view(ViewMode::Grid),
            KeyCode::Char('2') if ctrl => self.set_view(ViewMode::List),
            KeyCode::Char('3') if ctrl => self.set_view(ViewMode::Columns),
            KeyCode::Char('h') if ctrl => self.toggle_hidden(),
            KeyCode::Char('i') if ctrl => self.toggle_details(),
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
            (KeyCode::Delete, _) if bare => self.trash(),
            // A Mac keyboard's own way to say Delete.
            (KeyCode::Backspace, _) if held.contains(KeyModifiers::SUPER) => self.trash(),
            _ => return false,
        }
        true
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
            Dialog::Delete { armed, .. } => {
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
                self.relist(&[], neighbour);
                match deleted.error {
                    Some(error) => self.fail(error.to_string()),
                    None => self.note(&format!("Deleted {}.", ops::count(deleted.done))),
                }
            }
            Dialog::Delete { armed: false, .. } => {}
        }
    }

    /// What the file commands act on. One method for all of them, because the
    /// selection is going to hold several items.
    fn selection(&self) -> Vec<PathBuf> {
        if self.focus == Focus::Sidebar {
            return Vec::new();
        }
        let selected = self.selected().map(|entry| entry.path.clone());
        selected.into_iter().collect()
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
        let Some(clipboard) = &self.clipboard else {
            self.note("There's nothing to paste.");
            return;
        };
        let (paths, cut) = (clipboard.paths.clone(), clipboard.cut);
        let folder = self.pane().cwd.clone();
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

    /// Delete: moves the selection to the trash.
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
                self.relist(&change.origins(), None);
                self.note(&told("Undid", &change));
            }
            Ok(None) => self.note("There's nothing to undo."),
            Err(error) => {
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
                self.relist(&change.landed(), None);
                self.note(&told("Redid", &change));
            }
            Ok(None) => self.note("There's nothing to redo."),
            Err(error) => {
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
    fn settle(&mut self, report: Report, neighbour: Option<String>) {
        let message = outcome(&report);
        let landed = report.change.landed();
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

    /// Lists the folder again and puts the cursor on the first of these that
    /// is in it: one of `paths`, what was selected before, or `neighbour`.
    fn relist(&mut self, paths: &[PathBuf], neighbour: Option<String>) {
        let here = self.pane().visit();
        if !self.show_folder(&here.path, here.selected.as_deref()) {
            return;
        }
        let pane = self.pane_mut();
        let found = paths
            .iter()
            .filter(|path| path.parent() == Some(here.path.as_path()))
            .map(|path| name_of(path))
            .chain(here.selected)
            .chain(neighbour)
            .find_map(|name| pane.entries.iter().position(|entry| entry.name == name));
        if let Some(index) = found {
            pane.cursor = index;
        }
    }

    /// The name the cursor should fall on once these are gone: the next item
    /// that stays, or else the nearest one before.
    fn neighbour(&self, leaving: &[PathBuf]) -> Option<String> {
        let pane = self.pane();
        let stays = |entry: &&Entry| !leaving.contains(&entry.path);
        let after = pane.entries.iter().skip(pane.cursor).find(stays);
        let before = pane.entries.iter().take(pane.cursor).rev().find(stays);
        after.or(before).map(|entry| entry.name.clone())
    }

    /// Escape: stops the file work under way, or dismisses what went wrong.
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
        let columns = if grid { self.grid_columns.max(1) } else { 1 };
        let page = (self.page_rows.max(1) * columns) as isize;
        match key.code {
            // In the grid the arrows only move: Left and Right never change folder.
            KeyCode::Up if grid => self.pane_mut().move_row(false, columns),
            KeyCode::Down if grid => self.pane_mut().move_row(true, columns),
            KeyCode::Left if grid => self.pane_mut().move_cursor(-1),
            KeyCode::Right if grid => self.pane_mut().move_cursor(1),
            KeyCode::Up => self.pane_mut().move_cursor(-1),
            KeyCode::Down => self.pane_mut().move_cursor(1),
            KeyCode::PageUp => self.pane_mut().move_cursor(-page),
            KeyCode::PageDown => self.pane_mut().move_cursor(page),
            KeyCode::Home => self.pane_mut().move_cursor(isize::MIN / 2),
            KeyCode::End => self.pane_mut().move_cursor(isize::MAX / 2),
            KeyCode::Left => self.go_up(),
            KeyCode::Right => {
                if self.selected().is_some_and(|entry| entry.is_dir) {
                    self.open_selected();
                }
            }
            KeyCode::Enter => self.open_selected(),
            _ => {}
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        let last = self.places().count().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1),
            KeyCode::Down => self.sidebar_cursor = (self.sidebar_cursor + 1).min(last),
            KeyCode::Enter | KeyCode::Right => {
                let place = self.places().nth(self.sidebar_cursor);
                let path = place.map(|place| place.path.clone());
                if let Some(path) = path {
                    self.navigate(&path);
                    self.focus = Focus::Files;
                }
            }
            _ => {}
        }
    }

    fn details_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            self.open_selected();
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let mut stops = vec![Focus::Sidebar, Focus::Files];
        if self.details_visible {
            stops.push(Focus::Details);
        }
        let index = stops
            .iter()
            .position(|stop| *stop == self.focus)
            .unwrap_or(1) as isize;
        self.focus = stops[(index + delta).rem_euclid(stops.len() as isize) as usize];
        if self.focus == Focus::Sidebar {
            self.sidebar_cursor = self.current_place().unwrap_or(self.sidebar_cursor);
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
        let from = self.pane().visit();
        if path != from.path && self.show_folder(path, None) {
            let pane = self.pane_mut();
            pane.back.push(from);
            pane.forward.clear();
        }
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

    fn go_home(&mut self) {
        let home = self.home.clone();
        self.navigate(&home);
    }

    fn go_back(&mut self) {
        let from = self.pane().visit();
        let Some(to) = self.pane_mut().back.pop() else {
            return;
        };
        if self.show_folder(&to.path, to.selected.as_deref()) {
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
        if self.show_folder(&to.path, to.selected.as_deref()) {
            self.pane_mut().back.push(from);
        } else {
            self.pane_mut().forward.push(to);
        }
    }

    fn set_view(&mut self, view: ViewMode) {
        self.view = view;
        self.refresh_ancestors();
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
        self.reload();
    }

    fn toggle_details(&mut self) {
        self.details_visible = !self.details_visible;
        if !self.details_visible && self.focus == Focus::Details {
            self.focus = Focus::Files;
        }
    }

    fn set_sort(&mut self, sort: SortMode) {
        self.sort = sort;
        self.reload();
        self.note(sort.tip());
    }

    /// Lists the folder again, keeping the selection on the same item.
    fn reload(&mut self) {
        let here = self.pane().visit();
        self.show_folder(&here.path, here.selected.as_deref());
    }

    /// Lists a folder into the active pane; on failure says why and stays put.
    fn show_folder(&mut self, path: &Path, select: Option<&str>) -> bool {
        match model::list_dir(path, self.show_hidden, self.sort) {
            Ok(entries) => {
                self.pane_mut().show(path.to_path_buf(), entries, select);
                self.refresh_ancestors();
                self.message = None;
                true
            }
            Err(error) => {
                self.fail(read_error(path, &error));
                false
            }
        }
    }

    /// Lists the folders above the active pane's again, when the view shows them.
    fn refresh_ancestors(&mut self) {
        let ancestors = if self.view == ViewMode::Columns {
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

/// How a command over several items went, for the band.
fn outcome(report: &Report) -> Message {
    if let Some(error) = &report.error {
        return Message {
            text: error.to_string(),
            is_error: true,
        };
    }
    let done = ops::count(report.done);
    let text = if report.stopped {
        let asked = ops::count(report.asked);
        format!("Stopped after {} of {asked}.", report.done)
    } else if report.done == 0 && report.skipped > 0 {
        "It is already in this folder.".to_string()
    } else {
        match report.change.kind {
            Kind::Copy => format!("Copied {done}."),
            Kind::Duplicate => format!("Duplicated {done}."),
            Kind::Trash => format!("Moved {done} to the trash."),
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
    }
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
    use crate::thumbs::testing::FakeThumbnailer;
    use std::cell::RefCell;
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
    }

    pub struct Fixture {
        pub app: App,
        pub system: FakeSystem,
        pub home: PathBuf,
        /// The trash the app was given: a folder of the fixture's own.
        pub trash: PathBuf,
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
        let config = Config {
            home: home.clone(),
            start: home.clone(),
            date_style: DateStyle::MonthFirst,
            state_dir: None,
            thumbs,
            trash_dir: trash.clone(),
        };
        let mut app = App::new(config, Box::new(system.clone()));
        // The tests written against the list keep it; the others switch with Ctrl+1 or Ctrl+3.
        app.view = ViewMode::List;
        Fixture {
            app,
            system,
            home,
            trash,
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

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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
        let labels: Vec<_> = f.app.places().map(|place| place.label.clone()).collect();
        assert_eq!(labels, ["Documents", "Pictures"]);
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
        assert_eq!(f.app.grid_columns, 3);
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
        assert_eq!(f.app.page_rows, 2);
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
}
