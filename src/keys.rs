//! The browsing commands' keys: what each command is called in the keys
//! file, the keys it has until the user says otherwise, and the file
//! itself, `keys` in the state folder, which the app writes with every
//! command and its default keys when it is missing and reads at launch.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The keys file's name in the state folder.
pub const FILE: &str = "keys";

/// A browsing command a key can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    PaneCopy,
    PaneMove,
    PaneNewFolder,
    PaneTrash,
    PaneNewMarkdown,
    PaneNewText,
    Trash,
    TrashNow,
    DeleteForever,
    Copy,
    Cut,
    Paste,
    MoveHere,
    Duplicate,
    CopyFolderPath,
    CopyPath,
    Rename,
    NewFolder,
    Undo,
    Redo,
    Edit,
    Tags,
    Pin,
    Terminal,
    Select,
    InvertSelection,
    SelectAll,
    ExtendNext,
    ExtendPrevious,
    ExtendDown,
    ExtendUp,
    ExtendRight,
    ExtendLeft,
    Down,
    Up,
    GridLeft,
    GridRight,
    Parent,
    BrowseIn,
    Open,
    Back,
    Forward,
    First,
    Last,
    PageDown,
    PageUp,
    FocusNext,
    FocusPrevious,
    Escape,
    Help,
    QuickLook,
    Hidden,
    Detail,
    Filter,
    Find,
    Reveal,
    Sort,
    ReverseSort,
    GoTo,
    Refresh,
    Preview,
    Places,
    Menu,
    ListView,
    ColumnsView,
    GridView,
    DualPane,
    ZoomIn,
    ZoomOut,
    NewTab,
    OpenInNewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    MoveTabRight,
    MoveTabLeft,
    Tab1,
    Tab2,
    Tab3,
    Tab4,
    Tab5,
    Tab6,
    Tab7,
    Tab8,
    Tab9,
    Quit,
}

/// Every command in the order the file lists them, which is also the order
/// they are tried in when they share a key: the group heading the file
/// writes above it (empty to stay in the group above), its name in the
/// file, its default keys, and what it does.
pub const COMMANDS: &[(Command, &str, &str, &str, &str)] = &[
    (
        Command::PaneCopy,
        "Two panes: these run only while two panes show (the function key bar)",
        "pane_copy",
        "f5",
        "Copy to the other pane",
    ),
    (
        Command::PaneMove,
        "",
        "pane_move",
        "f6",
        "Move to the other pane",
    ),
    (
        Command::PaneNewFolder,
        "",
        "pane_new_folder",
        "f7",
        "New folder",
    ),
    (Command::PaneTrash, "", "pane_trash", "f8", "Move to Trash"),
    (
        Command::PaneNewMarkdown,
        "",
        "pane_new_markdown",
        "f9",
        "New Markdown file",
    ),
    (
        Command::PaneNewText,
        "",
        "pane_new_text",
        "f10",
        "New text file",
    ),
    (
        Command::Trash,
        "Changing files",
        "trash",
        "d",
        "Move to Trash (pressed twice)",
    ),
    (
        Command::TrashNow,
        "",
        "trash_now",
        "delete",
        "Move to Trash at once (in the Trash: delete permanently)",
    ),
    (
        Command::DeleteForever,
        "",
        "delete_forever",
        "shift-delete",
        "Delete permanently",
    ),
    (Command::Copy, "", "copy", "y, ctrl-c", "Copy"),
    (Command::Cut, "", "cut", "x, ctrl-x", "Cut"),
    (Command::Paste, "", "paste", "p, ctrl-v", "Paste"),
    (Command::MoveHere, "", "move_here", "M", "Move here"),
    (Command::Duplicate, "", "duplicate", "U", "Duplicate"),
    (
        Command::CopyFolderPath,
        "",
        "copy_folder_path",
        "Y",
        "Copy folder path",
    ),
    (
        Command::CopyPath,
        "",
        "copy_path",
        "ctrl-shift-c",
        "Copy path",
    ),
    (Command::Rename, "", "rename", "r, f2", "Rename"),
    (
        Command::NewFolder,
        "",
        "new_folder",
        "N, ctrl-shift-n",
        "New folder",
    ),
    (Command::Undo, "", "undo", "z, ctrl-z", "Undo"),
    (Command::Redo, "", "redo", "ctrl-shift-z, ctrl-y", "Redo"),
    (Command::Edit, "", "edit", "E", "Edit in $EDITOR"),
    (Command::Tags, "", "tags", "T", "Tags"),
    (Command::Pin, "", "pin", "ctrl-p", "Pin folder"),
    (
        Command::Terminal,
        "",
        "terminal",
        "ctrl-t, alt-t",
        "Open terminal",
    ),
    (Command::Select, "Selecting", "select", "v", "Select"),
    (
        Command::InvertSelection,
        "",
        "invert_selection",
        "V",
        "Invert selection",
    ),
    (Command::SelectAll, "", "select_all", "ctrl-a", "Select all"),
    (
        Command::ExtendNext,
        "",
        "extend_next",
        "J",
        "Extend to the next item",
    ),
    (
        Command::ExtendPrevious,
        "",
        "extend_previous",
        "K",
        "Extend to the previous item",
    ),
    (
        Command::ExtendDown,
        "",
        "extend_down",
        "shift-down",
        "Extend down",
    ),
    (Command::ExtendUp, "", "extend_up", "shift-up", "Extend up"),
    (
        Command::ExtendRight,
        "",
        "extend_right",
        "shift-right",
        "Extend right (grid)",
    ),
    (
        Command::ExtendLeft,
        "",
        "extend_left",
        "shift-left",
        "Extend left (grid)",
    ),
    (Command::Down, "Moving", "down", "j, down", "Cursor down"),
    (Command::Up, "", "up", "k, up", "Cursor up"),
    (
        Command::GridLeft,
        "",
        "grid_left",
        "left",
        "Cursor left (grid)",
    ),
    (
        Command::GridRight,
        "",
        "grid_right",
        "right",
        "Cursor right (grid)",
    ),
    (
        Command::Parent,
        "",
        "parent",
        "h, left, backspace",
        "Parent folder",
    ),
    (Command::BrowseIn, "", "browse_in", "l, right", "Browse in"),
    (Command::Open, "", "open", "enter", "Open"),
    (Command::Back, "", "back", "H", "Back"),
    (Command::Forward, "", "forward", "L", "Forward"),
    (Command::First, "", "first", "g, home", "First item"),
    (Command::Last, "", "last", "G, end", "Last item"),
    (
        Command::PageDown,
        "",
        "page_down",
        "ctrl-d, pagedown",
        "Page down",
    ),
    (Command::PageUp, "", "page_up", "ctrl-u, pageup", "Page up"),
    (Command::FocusNext, "", "focus_next", "tab", "Next focus"),
    (
        Command::FocusPrevious,
        "",
        "focus_previous",
        "shift-tab",
        "Previous focus",
    ),
    (Command::Escape, "", "escape", "esc", "Close or go back"),
    (Command::Help, "Looking", "help", "?", "Keyboard Shortcuts"),
    (Command::QuickLook, "", "quick_look", "space", "Quick Look"),
    (Command::Hidden, "", "hidden", ".", "Hidden files"),
    (Command::Detail, "", "detail", "I", "Detail column"),
    (Command::Filter, "", "filter", "/", "Filter"),
    (Command::Find, "", "find", "f, ctrl-f", "Find"),
    (Command::Reveal, "", "reveal", "o", "Reveal"),
    (Command::Sort, "", "sort", "s", "Sort"),
    (
        Command::ReverseSort,
        "",
        "reverse_sort",
        "S",
        "Reverse sort",
    ),
    (Command::GoTo, "", "go_to", ":, ctrl-l", "Go to folder"),
    (Command::Refresh, "", "refresh", "ctrl-r", "Reload"),
    (Command::Preview, "", "preview", "alt-p", "Preview column"),
    (Command::Places, "", "places", "ctrl-b", "Places panel"),
    (Command::Menu, "", "menu", "m, menu, shift-f10", "Menu"),
    (
        Command::ListView,
        "Views and panes",
        "list_view",
        "ctrl-1, ctrl-alt-1",
        "List view",
    ),
    (
        Command::ColumnsView,
        "",
        "columns_view",
        "ctrl-2, ctrl-alt-2",
        "Columns view",
    ),
    (
        Command::GridView,
        "",
        "grid_view",
        "ctrl-3, ctrl-alt-3",
        "Grid view",
    ),
    (
        Command::DualPane,
        "",
        "dual_pane",
        "ctrl-4, ctrl-alt-4, |, alt-1",
        "Dual pane",
    ),
    (Command::ZoomIn, "", "zoom_in", "+", "Zoom in"),
    (Command::ZoomOut, "", "zoom_out", "-", "Zoom out"),
    (Command::NewTab, "Tabs", "new_tab", "t", "New tab"),
    (
        Command::OpenInNewTab,
        "",
        "open_in_new_tab",
        "O",
        "Open in new tab",
    ),
    (Command::CloseTab, "", "close_tab", "w, ctrl-w", "Close tab"),
    (Command::NextTab, "", "next_tab", "]", "Next tab"),
    (
        Command::PreviousTab,
        "",
        "previous_tab",
        "[",
        "Previous tab",
    ),
    (
        Command::MoveTabRight,
        "",
        "move_tab_right",
        "}",
        "Move tab right",
    ),
    (
        Command::MoveTabLeft,
        "",
        "move_tab_left",
        "{",
        "Move tab left",
    ),
    (Command::Tab1, "", "tab_1", "1", "Select tab 1"),
    (Command::Tab2, "", "tab_2", "2", "Select tab 2"),
    (Command::Tab3, "", "tab_3", "3", "Select tab 3"),
    (Command::Tab4, "", "tab_4", "4", "Select tab 4"),
    (Command::Tab5, "", "tab_5", "5", "Select tab 5"),
    (Command::Tab6, "", "tab_6", "6", "Select tab 6"),
    (Command::Tab7, "", "tab_7", "7", "Select tab 7"),
    (Command::Tab8, "", "tab_8", "8", "Select tab 8"),
    (Command::Tab9, "", "tab_9", "9", "Select tab 9"),
    (Command::Quit, "", "quit", "q, Q", "Quit"),
];

/// What the file says above the commands.
const HEADER: &str = "\
# filefinder-tui keys: the browsing commands and the keys that run them.
# Edit this file and restart filefinder-tui to use your keys.
#
# One command per line:  name = key, key, ...
# A key is one character (M is Shift+m) or a name: up down left right home
# end pageup pagedown enter esc tab backspace delete insert space comma hash
# menu f1 to f24. Put ctrl-, alt- (or opt-) and shift- before it:
# ctrl-shift-z.
# A key with Shift that runs nothing here runs what the key without Shift
# does (shift-enter opens), except for a plain letter: M is not m.
# A # after a space starts a comment.
# `name =` with nothing after it leaves the command without keys; a command
# left out of this file keeps its default keys.
# When commands share a key, the first one that can run does, in the order
# the app writes them below (moving lines around changes nothing): left
# moves the cursor in the grid and goes to the parent folder elsewhere.
# The places panel, the preview, Quick Look, menus, prompts, the find and
# filter fields and `,` (settings) keep their own keys.
";

impl Command {
    fn entry(
        self,
    ) -> &'static (
        Command,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ) {
        COMMANDS
            .iter()
            .find(|entry| entry.0 == self)
            .expect("every command is listed")
    }

    /// Its name in the file.
    pub fn name(self) -> &'static str {
        self.entry().2
    }

    /// Its default keys as the file spells them.
    fn defaults(self) -> &'static str {
        self.entry().3
    }

    /// Whether it runs only sometimes (in the grid, or while two panes
    /// show), so a key it shares with another command is no clash.
    pub fn conditional(self) -> bool {
        matches!(
            self,
            Command::PaneCopy
                | Command::PaneMove
                | Command::PaneNewFolder
                | Command::PaneTrash
                | Command::PaneNewMarkdown
                | Command::PaneNewText
                | Command::ExtendRight
                | Command::ExtendLeft
                | Command::GridLeft
                | Command::GridRight
        )
    }

    /// Whether it is the window's rather than the files': its keys reach
    /// it even while the places panel has the keyboard.
    pub fn window(self) -> bool {
        matches!(
            self,
            Command::FocusNext
                | Command::FocusPrevious
                | Command::Places
                | Command::Refresh
                | Command::ListView
                | Command::ColumnsView
                | Command::GridView
                | Command::DualPane
                | Command::Preview
                | Command::Help
                | Command::Quit
                | Command::Hidden
                | Command::Sort
                | Command::ReverseSort
                | Command::Detail
                | Command::NewTab
                | Command::CloseTab
                | Command::NextTab
                | Command::PreviousTab
                | Command::MoveTabRight
                | Command::MoveTabLeft
                | Command::Tab1
                | Command::Tab2
                | Command::Tab3
                | Command::Tab4
                | Command::Tab5
                | Command::Tab6
                | Command::Tab7
                | Command::Tab8
                | Command::Tab9
        )
    }

    fn named(name: &str) -> Option<Command> {
        COMMANDS
            .iter()
            .find(|entry| entry.2.eq_ignore_ascii_case(name))
            .map(|entry| entry.0)
    }
}

/// A key with the modifiers held with it. A letter is always small, with
/// Shift for its capital; other characters never carry Shift, since it is
/// what it takes to type some of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// The keys known by name, as the file spells them, first spelling first.
const NAMES: &[(&str, KeyCode)] = &[
    ("up", KeyCode::Up),
    ("down", KeyCode::Down),
    ("left", KeyCode::Left),
    ("right", KeyCode::Right),
    ("home", KeyCode::Home),
    ("end", KeyCode::End),
    ("pageup", KeyCode::PageUp),
    ("pgup", KeyCode::PageUp),
    ("pagedown", KeyCode::PageDown),
    ("pgdn", KeyCode::PageDown),
    ("enter", KeyCode::Enter),
    ("return", KeyCode::Enter),
    ("esc", KeyCode::Esc),
    ("escape", KeyCode::Esc),
    ("tab", KeyCode::Tab),
    ("backspace", KeyCode::Backspace),
    ("delete", KeyCode::Delete),
    ("del", KeyCode::Delete),
    ("insert", KeyCode::Insert),
    ("ins", KeyCode::Insert),
    ("space", KeyCode::Char(' ')),
    ("comma", KeyCode::Char(',')),
    ("hash", KeyCode::Char('#')),
    ("menu", KeyCode::Menu),
];

impl Chord {
    fn new(code: KeyCode, ctrl: bool, alt: bool, shift: bool) -> Chord {
        let (code, shift) = match code {
            KeyCode::Char(c) if c.is_ascii_alphabetic() => (
                KeyCode::Char(c.to_ascii_lowercase()),
                shift || c.is_ascii_uppercase(),
            ),
            KeyCode::Char(c) => (KeyCode::Char(c), false),
            KeyCode::BackTab => (KeyCode::Tab, true),
            code => (code, shift),
        };
        Chord {
            code,
            ctrl,
            alt,
            shift,
        }
    }

    /// The chord a key press is; none with Command (Super) or the like
    /// held, which no command takes.
    pub fn of(key: KeyEvent) -> Option<Chord> {
        let held = key.modifiers;
        if held.intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META) {
            return None;
        }
        Some(Chord::new(
            key.code,
            held.contains(KeyModifiers::CONTROL),
            held.contains(KeyModifiers::ALT),
            held.contains(KeyModifiers::SHIFT),
        ))
    }

    /// The same chord without Shift, to try when the one with it runs
    /// nothing; a plain letter has none, its capital being another key.
    pub fn unshifted(self) -> Option<Chord> {
        let letter = matches!(self.code, KeyCode::Char(c) if c.is_ascii_alphabetic());
        (self.shift && !(letter && !self.ctrl && !self.alt)).then_some(Chord {
            shift: false,
            ..self
        })
    }

    /// Reads a key as the file spells it.
    pub fn parse(text: &str) -> Option<Chord> {
        let mut rest = text.trim();
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        // A single character is the key itself, even `-`.
        while rest.chars().count() > 1 {
            let lower = rest.to_ascii_lowercase();
            let prefix = ["ctrl-", "control-", "alt-", "opt-", "option-", "shift-"]
                .into_iter()
                .find(|prefix| lower.starts_with(prefix));
            let Some(prefix) = prefix else { break };
            match prefix {
                "ctrl-" | "control-" => ctrl = true,
                "shift-" => shift = true,
                _ => alt = true,
            }
            rest = &rest[prefix.len()..];
        }
        let mut chars = rest.chars();
        let code = match (chars.next(), chars.next()) {
            (None, _) => return None,
            (Some(c), None) => KeyCode::Char(c),
            _ => {
                let lower = rest.to_ascii_lowercase();
                match NAMES.iter().find(|(name, _)| *name == lower) {
                    Some((_, code)) => *code,
                    None => {
                        let number = lower.strip_prefix('f')?.parse::<u8>().ok()?;
                        if !(1..=24).contains(&number) {
                            return None;
                        }
                        KeyCode::F(number)
                    }
                }
            }
        };
        Some(Chord::new(code, ctrl, alt, shift))
    }

    /// The chord as the file and the key sheet spell it: a capital for a
    /// shifted letter on its own, `ctrl-shift-z` with Control or Option.
    pub fn spell(self) -> String {
        let key = match self.code {
            KeyCode::Char(c)
                if self.shift && !self.ctrl && !self.alt && c.is_ascii_alphabetic() =>
            {
                return c.to_ascii_uppercase().to_string();
            }
            KeyCode::F(number) => format!("f{number}"),
            KeyCode::Char(c) => match NAMES.iter().find(|(_, code)| *code == KeyCode::Char(c)) {
                Some((name, _)) => name.to_string(),
                None => c.to_string(),
            },
            code => match NAMES.iter().find(|(_, named)| *named == code) {
                Some((name, _)) => name.to_string(),
                None => format!("{code:?}").to_ascii_lowercase(),
            },
        };
        let mut spelled = String::new();
        if self.ctrl {
            spelled.push_str("ctrl-");
        }
        if self.alt {
            spelled.push_str("alt-");
        }
        if self.shift {
            spelled.push_str("shift-");
        }
        spelled + &key
    }
}

/// A line up to its comment: a `#` at its start or after a space.
fn without_comment(line: &str) -> &str {
    let mut before = ' ';
    for (at, c) in line.char_indices() {
        if c == '#' && before.is_whitespace() {
            return &line[..at];
        }
        before = c;
    }
    line
}

/// Reads keys as the file spells them, one after another with commas.
fn chords(text: &str) -> Vec<Chord> {
    text.split(',').filter_map(Chord::parse).collect()
}

/// Every command's keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    keys: HashMap<Command, Vec<Chord>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let keys = COMMANDS
            .iter()
            .map(|(command, _, _, defaults, _)| (*command, chords(defaults)))
            .collect();
        Keymap { keys }
    }
}

impl Keymap {
    /// The keymap a keys file sets, and what in it could not be read, line
    /// by line. A command the file leaves out keeps its default keys.
    pub fn read(text: &str) -> (Keymap, Vec<String>) {
        let mut keymap = Keymap::default();
        let mut problems = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = without_comment(line).trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, keys)) = line.split_once('=') else {
                problems.push(format!("keys line {number}: no \"=\" (name = key, key)"));
                continue;
            };
            let name = name.trim();
            let Some(command) = Command::named(name) else {
                problems.push(format!("keys line {number}: unknown command \"{name}\""));
                continue;
            };
            let mut read = Vec::new();
            let mut written = 0;
            for key in keys.split(',').map(str::trim).filter(|key| !key.is_empty()) {
                written += 1;
                match Chord::parse(key) {
                    Some(chord) if !read.contains(&chord) => read.push(chord),
                    Some(_) => {}
                    None => problems.push(format!("keys line {number}: unknown key \"{key}\"")),
                }
            }
            // Keys that were all unreadable keep the defaults rather than
            // leave the command without one.
            if written == 0 || !read.is_empty() {
                keymap.keys.insert(command, read);
            }
        }
        problems.extend(keymap.clashes());
        (keymap, problems)
    }

    /// Two commands that always run sharing a key: the first one wins.
    fn clashes(&self) -> Vec<String> {
        let mut said = Vec::new();
        let mut owner: HashMap<Chord, Command> = HashMap::new();
        for (command, ..) in COMMANDS.iter().filter(|entry| !entry.0.conditional()) {
            for chord in self.keys(*command) {
                match owner.get(chord) {
                    Some(first) => said.push(format!(
                        "keys: {} is both {} and {}; {} wins",
                        chord.spell(),
                        first.name(),
                        command.name(),
                        first.name()
                    )),
                    None => {
                        owner.insert(*chord, *command);
                    }
                }
            }
        }
        said
    }

    /// A command's keys.
    pub fn keys(&self, command: Command) -> &[Chord] {
        self.keys.get(&command).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether a command has the keys it has by default.
    pub fn is_default(&self, command: Command) -> bool {
        self.keys(command) == chords(command.defaults())
    }

    /// The commands a chord runs, in the order to try them.
    pub fn commands(&self, chord: Chord) -> impl Iterator<Item = Command> + '_ {
        COMMANDS
            .iter()
            .map(|entry| entry.0)
            .filter(move |command| self.keys(*command).contains(&chord))
    }

    /// A command's keys as the key sheet spells them, `/` between them;
    /// `unbound` without any. Trash's are pressed twice: `dd`.
    pub fn spell(&self, command: Command) -> String {
        let keys = self.keys(command);
        if keys.is_empty() {
            return "unbound".to_string();
        }
        let spelled: Vec<String> = keys
            .iter()
            .map(|chord| match (command, chord.spell()) {
                (Command::Trash, key) if key.chars().count() == 1 => format!("{key}{key}"),
                (Command::Trash, key) => format!("{key} {key}"),
                (_, key) => key,
            })
            .collect();
        spelled.join(" / ")
    }

    /// What shows for a few commands' keys: `default`, today's words, while
    /// they all have their default keys; the keys the file gives them, with
    /// what `default` says after them in brackets, once it changes one.
    pub fn show(&self, commands: &[Command], default: &str) -> String {
        if commands.iter().all(|command| self.is_default(*command)) {
            return default.to_string();
        }
        let bound: Vec<String> = commands
            .iter()
            .filter(|command| !self.keys(**command).is_empty())
            .map(|command| self.spell(*command))
            .collect();
        let keys = match bound.is_empty() {
            true => "unbound".to_string(),
            false => bound.join(" / "),
        };
        match default.find(" (") {
            Some(at) => format!("{keys}{}", &default[at..]),
            None => keys,
        }
    }

    /// The key a short label names a command by: `default` while it has
    /// its default keys, else its first function key, else its first key,
    /// with a function key's F a capital; nothing when it has none.
    pub fn first(&self, command: Command, default: &str) -> String {
        if self.is_default(command) {
            return default.to_string();
        }
        let keys = self.keys(command);
        let key = keys
            .iter()
            .find(|chord| matches!(chord.code, KeyCode::F(_)))
            .or(keys.first());
        let Some(chord) = key else {
            return String::new();
        };
        let mut spelled = chord.spell();
        // A function key's spelling ends in its `f` and number.
        if let (KeyCode::F(_), Some(at)) = (chord.code, spelled.rfind('f')) {
            spelled.replace_range(at..=at, "F");
        }
        spelled
    }
}

/// The keys file with every command at its default keys.
pub fn default_file() -> String {
    let mut text = HEADER.to_string();
    for (_, group, name, defaults, does) in COMMANDS {
        if !group.is_empty() {
            text.push_str(&format!("\n# {group}\n"));
        }
        let line = format!("{name} = {defaults}");
        text.push_str(&format!("{line:<39} # {does}\n"));
    }
    text
}

/// The keymap in the state folder's keys file, and what in it could not be
/// read. Without the file, the defaults, and the file written with them for
/// the user to edit; a file that can't be read leaves the defaults too.
pub fn load(dir: &Path) -> (Keymap, Vec<String>) {
    let path = dir.join(FILE);
    match fs::read_to_string(&path) {
        Ok(text) => Keymap::read(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Nowhere to write it is no reason not to start.
            let _ = fs::create_dir_all(dir).and_then(|()| fs::write(&path, default_file()));
            (Keymap::default(), Vec::new())
        }
        Err(error) => (
            Keymap::default(),
            vec![format!("keys: can't read {}: {error}", path.display())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        Chord::parse(text).unwrap_or_else(|| panic!("{text} reads"))
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Chord {
        Chord::of(KeyEvent::new(code, modifiers)).expect("a chord")
    }

    #[test]
    fn keys_read_as_the_terminal_sends_them() {
        let none = KeyModifiers::NONE;
        let shift = KeyModifiers::SHIFT;
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(chord("j"), press(KeyCode::Char('j'), none));
        // A capital is Shift and the letter, however the terminal sends it.
        assert_eq!(chord("M"), press(KeyCode::Char('M'), none));
        assert_eq!(chord("M"), press(KeyCode::Char('M'), shift));
        assert_eq!(chord("shift-m"), press(KeyCode::Char('m'), shift));
        // Shift is what it takes to type `?`, and no part of it.
        assert_eq!(chord("?"), press(KeyCode::Char('?'), shift));
        assert_eq!(chord("shift-?"), chord("?"));
        assert_eq!(
            chord("ctrl-shift-z"),
            press(KeyCode::Char('Z'), ctrl | shift)
        );
        assert_eq!(
            chord("ctrl-shift-z"),
            press(KeyCode::Char('z'), ctrl | shift)
        );
        assert_eq!(
            chord("Ctrl-Alt-1"),
            press(KeyCode::Char('1'), ctrl | KeyModifiers::ALT)
        );
        assert_eq!(chord("opt-p"), chord("alt-p"));
        assert_eq!(chord("shift-tab"), press(KeyCode::BackTab, shift));
        assert_eq!(chord("shift-tab"), press(KeyCode::BackTab, none));
        assert_eq!(chord("F10"), press(KeyCode::F(10), none));
        assert_eq!(chord("shift-f10"), press(KeyCode::F(10), shift));
        assert_eq!(chord("PageDown"), press(KeyCode::PageDown, none));
        assert_eq!(chord("space"), press(KeyCode::Char(' '), none));
        assert_eq!(chord("comma"), press(KeyCode::Char(','), none));
        assert_eq!(chord("-"), press(KeyCode::Char('-'), none));
        assert_eq!(chord("ctrl--"), press(KeyCode::Char('-'), ctrl));
        assert_eq!(
            Chord::of(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::SUPER)),
            None
        );
        for bad in [
            "",
            "ctrl-",
            "shift-",
            "jj",
            "f0",
            "f25",
            "hyper-x",
            "ctrl-pageupp",
        ] {
            assert_eq!(Chord::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_key_spells_as_the_file_reads_it() {
        for text in [
            "j",
            "M",
            "?",
            "ctrl-shift-z",
            "ctrl-alt-1",
            "alt-p",
            "shift-tab",
            "f10",
            "shift-f10",
            "pagedown",
            "space",
            "comma",
            "-",
            "ctrl--",
            "shift-down",
            "esc",
        ] {
            assert_eq!(chord(text).spell(), text);
            assert_eq!(chord(&chord(text).spell()), chord(text));
        }
        assert_eq!(chord("alt-P").spell(), "alt-shift-p");
    }

    #[test]
    fn shift_falls_back_except_on_a_plain_letter() {
        assert_eq!(chord("shift-enter").unshifted(), Some(chord("enter")));
        assert_eq!(chord("ctrl-shift-v").unshifted(), Some(chord("ctrl-v")));
        assert_eq!(chord("M").unshifted(), None);
        assert_eq!(chord("enter").unshifted(), None);
    }

    #[test]
    fn the_default_file_reads_back_as_the_defaults_without_a_problem() {
        let (keymap, problems) = Keymap::read(&default_file());
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(keymap, Keymap::default());
        for (command, _, name, defaults, _) in COMMANDS {
            assert!(keymap.is_default(*command), "{name}");
            assert!(!keymap.keys(*command).is_empty(), "{name} = {defaults}");
            assert_eq!(Command::named(name), Some(*command));
        }
    }

    #[test]
    fn the_file_changes_only_the_commands_it_names() {
        let text = "# mine\n\ncopy = c # mine too\nQUIT = ctrl-q, q\nfind = ctrl-#, hash\n";
        let (keymap, problems) = Keymap::read(text);
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(keymap.keys(Command::Copy), &[chord("c")]);
        assert_eq!(keymap.keys(Command::Quit), &[chord("ctrl-q"), chord("q")]);
        assert_eq!(
            keymap.keys(Command::Find),
            &[chord("ctrl-#"), chord("hash")]
        );
        assert_eq!(chord("hash").spell(), "hash");
        assert!(!keymap.is_default(Command::Copy));
        assert!(keymap.is_default(Command::Cut));
        assert_eq!(keymap.spell(Command::Quit), "ctrl-q / q");
        assert_eq!(
            keymap.commands(chord("c")).collect::<Vec<_>>(),
            [Command::Copy]
        );
        assert_eq!(keymap.commands(chord("y")).count(), 0);
    }

    #[test]
    fn nothing_after_the_equals_leaves_a_command_without_keys() {
        let (keymap, problems) = Keymap::read("quit =\n");
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(keymap.keys(Command::Quit), &[]);
        assert_eq!(keymap.spell(Command::Quit), "unbound");
    }

    #[test]
    fn what_cannot_be_read_is_named_by_its_line_and_keeps_the_defaults() {
        let text = "copy = c, ctrl-foo\nfoo = x\njust words\ncut = nope\n";
        let (keymap, problems) = Keymap::read(text);
        assert_eq!(
            problems,
            [
                "keys line 1: unknown key \"ctrl-foo\"",
                "keys line 2: unknown command \"foo\"",
                "keys line 3: no \"=\" (name = key, key)",
                "keys line 4: unknown key \"nope\"",
            ]
        );
        assert_eq!(keymap.keys(Command::Copy), &[chord("c")]);
        assert!(keymap.is_default(Command::Cut));
    }

    #[test]
    fn two_commands_on_one_key_are_said_and_the_first_wins() {
        let (keymap, problems) = Keymap::read("quit = y\n");
        assert_eq!(problems, ["keys: y is both copy and quit; copy wins"]);
        assert_eq!(keymap.commands(chord("y")).next(), Some(Command::Copy));
        // Left in the grid and elsewhere is no clash.
        assert_eq!(Keymap::default().clashes(), Vec::<String>::new());
    }

    #[test]
    fn displays_keep_today_s_words_until_the_file_changes_a_command() {
        let defaults = Keymap::default();
        assert_eq!(defaults.show(&[Command::Copy], "y / ctrl-c"), "y / ctrl-c");
        assert_eq!(defaults.first(Command::PaneCopy, "F5"), "F5");
        let text = "copy = c, ctrl-shift-c\npane_copy = shift-f3, f4\ntrash = Z, ctrl-k\n\
                    trash_now =\nmenu = shift-f11\nquit =\nredo = ctrl-y\n";
        let (keymap, _) = Keymap::read(text);
        assert_eq!(
            keymap.show(&[Command::Copy], "y / ctrl-c"),
            "c / ctrl-shift-c"
        );
        // A bracket after the words stays: what the keys need.
        assert_eq!(
            keymap.show(&[Command::PaneCopy], "f5 (two panes)"),
            "shift-f3 / f4 (two panes)"
        );
        // Trash's keys are pressed twice; one without keys drops out.
        assert_eq!(
            keymap.show(&[Command::Trash, Command::TrashNow], "dd / delete"),
            "ZZ / ctrl-k ctrl-k"
        );
        assert_eq!(keymap.show(&[Command::Quit], "q"), "unbound");
        // A short label: a function key first, its F a capital.
        assert_eq!(keymap.first(Command::PaneCopy, "F5"), "shift-F3");
        assert_eq!(keymap.first(Command::Menu, "m"), "shift-F11");
        assert_eq!(keymap.first(Command::Trash, "d"), "Z");
        assert_eq!(keymap.first(Command::TrashNow, "Delete"), "");
        assert_eq!(keymap.first(Command::Redo, "ctrl-shift-z"), "ctrl-y");
        assert_eq!(keymap.first(Command::Cut, "x"), "x", "left at its defaults");
    }

    #[test]
    fn a_missing_file_is_written_with_the_defaults_and_read_next_time() {
        let dir = tempfile::tempdir().expect("a folder");
        let state = dir.path().join("filefinder-tui");
        let (keymap, problems) = load(&state);
        assert_eq!((keymap, problems), (Keymap::default(), Vec::new()));
        let written = fs::read_to_string(state.join(FILE)).expect("the file");
        assert_eq!(written, default_file());
        assert!(written.contains("\ncopy = y, ctrl-c"));
        fs::write(state.join(FILE), "copy = c\n").expect("edited");
        let (keymap, _) = load(&state);
        assert_eq!(keymap.keys(Command::Copy), &[chord("c")]);
    }

    #[test]
    fn a_file_that_cannot_be_read_leaves_the_defaults_and_says_so() {
        let dir = tempfile::tempdir().expect("a folder");
        // A folder where the file should be.
        fs::create_dir(dir.path().join(FILE)).expect("a folder");
        let (keymap, problems) = load(dir.path());
        assert_eq!(keymap, Keymap::default());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("keys: can't read"), "{problems:?}");
    }
}

#[cfg(test)]
mod app_tests {
    use std::fs;

    use ratatui::crossterm::event::KeyCode;

    use super::{FILE, default_file};
    use crate::app::Focus;
    use crate::app::testing::{ctrl, fixture, launch, launch_one_pane, names, press, shift, split};
    use crate::ui::testing::screen;

    /// The app launched over the fixture with this keys file.
    fn with_keys(home: &std::path::Path, state: &std::path::Path, text: &str) -> crate::app::App {
        fs::write(state.join(FILE), text).expect("written");
        launch_one_pane(home, Some(state))
    }

    #[test]
    fn launching_writes_the_keys_file_with_every_default_and_says_nothing() {
        let f = fixture();
        let state = tempfile::tempdir().expect("a folder");
        let app = launch(&f.home, Some(state.path()));
        let written = fs::read_to_string(state.path().join(FILE)).expect("the keys file");
        assert_eq!(written, default_file());
        assert_eq!(app.message, None);
    }

    #[test]
    fn the_keys_file_changes_what_the_keys_do_and_leaves_the_rest() {
        let f = fixture();
        let state = tempfile::tempdir().expect("a folder");
        fs::write(state.path().join(FILE), "quit = ctrl-q\ndown = n\n").expect("written");
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(!app.should_quit, "q no longer quits");
        press(&mut app, &[KeyCode::Char('j')]);
        assert_eq!(app.pane().cursor, 0, "j no longer moves down");
        press(&mut app, &[KeyCode::Char('n')]);
        assert_eq!(app.pane().cursor, 1, "n does");
        press(&mut app, &[KeyCode::Char('k')]);
        assert_eq!(app.pane().cursor, 0, "k is still up");
        // The window's keys reach their commands from the places panel too.
        app.focus = Focus::Places;
        app.handle_key(ctrl(KeyCode::Char('q')));
        assert!(app.should_quit, "ctrl-q quits");
    }

    #[test]
    fn what_the_keys_file_gets_wrong_is_said_at_launch_and_keeps_the_defaults() {
        let f = fixture();
        let state = tempfile::tempdir().expect("a folder");
        fs::write(state.path().join(FILE), "foo = x\nquit = nope\n").expect("written");
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        let message = app.message.as_ref().expect("a message");
        assert_eq!(
            message.text,
            "keys line 1: unknown command \"foo\" (and 1 more)"
        );
        assert!(message.is_error);
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(app.should_quit, "q still quits");
    }

    #[test]
    fn shift_on_a_key_that_runs_nothing_with_it_runs_the_key() {
        let f = fixture();
        let mut app = launch_one_pane(&f.home, None);
        let at = names(&app)
            .iter()
            .position(|name| name == "Pictures")
            .expect("Pictures");
        for _ in 0..at {
            press(&mut app, &[KeyCode::Char('j')]);
        }
        // Shift+Right extends in the grid; in the list it browses in.
        app.handle_key(shift(KeyCode::Right));
        assert_eq!(app.pane().cwd, f.home.join("Pictures"));
        app.handle_key(shift(KeyCode::Left));
        assert_eq!(app.pane().cwd, f.home);
        // A capital is a key of its own: Shift+q is not q.
        app.handle_key(shift(KeyCode::Char('Q')));
        assert!(app.should_quit, "Q is bound to quit by default");
    }

    #[test]
    fn the_key_sheet_footer_and_bar_show_the_keys_the_file_gives() {
        let f = fixture();
        let state = tempfile::tempdir().expect("a folder");
        let text = "copy = c\nhelp = F1\npane_copy = f3\n";
        let mut app = with_keys(&f.home, state.path(), text);
        let rows = screen(&mut app, 120, 30);
        assert!(rows[29].contains("F1 keys"), "{}", rows[29]);
        app.handle_key(ratatui::crossterm::event::KeyEvent::from(KeyCode::F(1)));
        assert!(app.help_visible, "F1 opens the sheet");
        let sheet = screen(&mut app, 160, 60).join("\n");
        assert!(sheet.contains("c  Copy"), "{sheet}");
        assert!(sheet.contains("x / ctrl-x  Cut"), "{sheet}");
        assert!(sheet.contains("f1  Keyboard Shortcuts"), "{sheet}");
        assert!(
            sheet.contains("f3 (two panes)  Copy to other pane"),
            "{sheet}"
        );
        assert!(!sheet.contains("y / ctrl-c"), "{sheet}");
        press(&mut app, &[KeyCode::Esc]);
        split(&mut app);
        let rows = screen(&mut app, 120, 30);
        assert!(rows.iter().any(|row| row.contains("F3 Copy")), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains("F6 Move")), "{rows:?}");
        assert!(rows.iter().all(|row| !row.contains("F5 Copy")), "{rows:?}");
    }

    #[test]
    fn the_trash_tip_names_the_keys_the_file_gives() {
        let f = fixture();
        let state = tempfile::tempdir().expect("a folder");
        let mut app = with_keys(&f.home, state.path(), "trash = Z\ntrash_now =\n");
        app.handle_key(shift(KeyCode::Char('Z')));
        let message = app.message.as_ref().expect("the tip");
        assert_eq!(message.text, "Press Z again to move it to Trash.");
        app.handle_key(shift(KeyCode::Char('Z')));
        crate::app::testing::finish_work(&mut app);
        assert!(
            f.trash.read_dir().expect("the trash").next().is_some(),
            "ZZ trashed it"
        );
    }
}
