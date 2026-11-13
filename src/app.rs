//! Application state and what each key does to it.

use std::io;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::format::DateStyle;
use crate::model::{self, Entry, SortMode};
use crate::places::{self, Place, Section};
use crate::state::{self, Saved};
use crate::system::System;

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
    pub message: Option<Message>,
    pub should_quit: bool,
    pub date_style: DateStyle,
    /// Rows the file view showed last time, for Page Up and Page Down.
    pub page_rows: usize,
    /// Tiles across the grid last time, for Up and Down.
    pub grid_columns: usize,
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
            message: None,
            should_quit: false,
            date_style: config.date_style,
            page_rows: 10,
            grid_columns: 1,
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

    pub fn handle_key(&mut self, key: KeyEvent) {
        if self.help_visible {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                self.help_visible = false;
            }
            return;
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = !alt && key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !alt && !ctrl;
        match key.code {
            KeyCode::Char('?') => self.help_visible = true,
            KeyCode::Char('q' | 'Q') if plain => self.should_quit = true,
            KeyCode::Esc => self.message = None,
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
    use std::cell::RefCell;
    use std::fs;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    pub struct FakeSystem {
        pub opened: Rc<RefCell<Vec<PathBuf>>>,
    }

    impl System for FakeSystem {
        fn open(&self, path: &Path) -> io::Result<()> {
            self.opened.borrow_mut().push(path.to_path_buf());
            Ok(())
        }
    }

    pub struct Fixture {
        pub app: App,
        pub system: FakeSystem,
        pub home: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// A home folder with `Documents/`, `Pictures/{Trip/, a.jpg}`, `notes.txt` and `.hidden`.
    pub fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        fs::create_dir(home.join("Documents")).unwrap();
        fs::create_dir_all(home.join("Pictures").join("Trip")).unwrap();
        fs::write(home.join("Pictures").join("a.jpg"), "jpg").unwrap();
        fs::write(home.join("notes.txt"), "hello").unwrap();
        fs::write(home.join(".hidden"), "").unwrap();
        let system = FakeSystem::default();
        let config = Config {
            home: home.clone(),
            start: home.clone(),
            date_style: DateStyle::MonthFirst,
            state_dir: None,
        };
        let mut app = App::new(config, Box::new(system.clone()));
        // The tests written against the list keep it; the others switch with Ctrl+1 or Ctrl+3.
        app.view = ViewMode::List;
        Fixture {
            app,
            system,
            home,
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
}
