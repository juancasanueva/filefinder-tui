//! Application state and what each key does to it.

use std::io;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::format::DateStyle;
use crate::model::{self, Entry, SortMode};
use crate::places::{self, Place, Section};
use crate::system::System;

// Grid and columns are drawn by the next work unit.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
    Columns,
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

pub struct Pane {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    /// First visible row; the view keeps the cursor on screen with it.
    pub offset: usize,
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
}

pub struct Config {
    pub home: PathBuf,
    /// A folder to open, or a file to select in its folder.
    pub start: PathBuf,
    pub date_style: DateStyle,
}

pub struct App {
    pub home: PathBuf,
    pub panes: Vec<Pane>,
    pub active: usize,
    pub focus: Focus,
    pub view: ViewMode,
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
    system: Box<dyn System>,
}

impl App {
    pub fn new(config: Config, system: Box<dyn System>) -> Self {
        let mut app = Self {
            sidebar: vec![places::home_section(&config.home)],
            panes: vec![Pane::new(config.home.clone(), Vec::new())],
            home: config.home,
            active: 0,
            focus: Focus::Files,
            view: ViewMode::List,
            sort: SortMode::Name,
            show_hidden: false,
            details_visible: true,
            sidebar_cursor: 0,
            help_visible: false,
            message: None,
            should_quit: false,
            date_style: config.date_style,
            page_rows: 10,
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
        let plain = !alt && !key.modifiers.contains(KeyModifiers::CONTROL);
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
            _ => match self.focus {
                Focus::Sidebar => self.sidebar_key(key),
                Focus::Files => self.files_key(key),
                Focus::Details => self.details_key(key),
            },
        }
    }

    fn files_key(&mut self, key: KeyEvent) {
        let page = self.page_rows.max(1) as isize;
        match key.code {
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
                self.message = None;
                true
            }
            Err(error) => {
                self.fail(read_error(path, &error));
                false
            }
        }
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
        };
        Fixture {
            app: App::new(config, Box::new(system.clone())),
            system,
            home,
            _dir: dir,
        }
    }

    pub fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
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
}
