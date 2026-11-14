//! The Settings card: flea's sections and rows (its window's Settings; its
//! terminal interface has none), for what this app already does. Every row
//! reads the app's own state, so the card never says something it isn't.

use crate::app::{App, Detail, ViewMode};
use crate::model::SortKey;

/// The app's name, as About says it.
pub const NAME: &str = "filefinder-tui";
/// What it is, under its name.
pub const TAGLINE: &str = "A keyboard-first terminal file manager for macOS";
/// Where "Report an issue" goes.
pub const ISSUES: &str = "https://github.com/juancasanueva/filefinder-tui/issues";

/// A section of the card, in the rail's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    View,
    About,
}

pub const SECTIONS: [Section; 2] = [Section::View, Section::About];

impl Section {
    pub fn name(self) -> &'static str {
        match self {
            Section::View => "View",
            Section::About => "About",
        }
    }
}

/// What a row is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    View,
    Preview,
    Places,
    Detail,
    SortBy,
    Order,
    Hidden,
    /// Whether one of the list's columns shows, by its place among them.
    Column(usize),
    Space,
    Version,
    BasedOn,
    Keys,
    Issue,
}

/// How a row answers the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One of several values: `h`/`l` step it, Enter steps it on.
    Choice,
    /// On or off: Space or Enter.
    Check,
    /// Enter does something.
    Action,
    /// Only says something.
    Fact,
}

impl Setting {
    pub fn kind(self) -> Kind {
        match self {
            Setting::View | Setting::Detail | Setting::SortBy | Setting::Order | Setting::Space => {
                Kind::Choice
            }
            Setting::Preview | Setting::Places | Setting::Hidden | Setting::Column(_) => {
                Kind::Check
            }
            Setting::Keys | Setting::Issue => Kind::Action,
            Setting::Version | Setting::BasedOn => Kind::Fact,
        }
    }
}

/// What a row shows on its right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Choice(&'static str),
    Check(bool),
    Text(String),
}

/// A line of a section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A heading over the rows under it, upper case.
    Heading(&'static str),
    Item {
        setting: Setting,
        label: &'static str,
        value: Value,
        /// A few muted words after the label.
        caption: Option<&'static str>,
    },
}

/// The card while it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub section: Section,
    /// Whether the rail has the keyboard rather than the rows.
    pub in_rail: bool,
    /// The row with the keyboard, among the section's items.
    pub cursor: usize,
    /// Whether something was changed since it opened.
    pub saved: bool,
}

impl Settings {
    /// Each opening starts on View with the keyboard in the rows.
    pub fn new() -> Self {
        Settings {
            section: Section::View,
            in_rail: false,
            cursor: 0,
            saved: false,
        }
    }
}

/// The views "Last-used view" steps through, in the buttons' order.
pub const VIEWS: [ViewMode; 4] = [
    ViewMode::List,
    ViewMode::Columns,
    ViewMode::Grid,
    ViewMode::Dual,
];
/// What the columns view's detail column can show, in `I`'s order.
pub const DETAILS: [Detail; 4] = [Detail::Size, Detail::Modified, Detail::Mode, Detail::Owner];
/// What "Sort by" steps through, in `s`'s order.
pub const KEYS: [SortKey; 4] = [
    SortKey::Name,
    SortKey::Size,
    SortKey::Modified,
    SortKey::Kind,
];

fn view_name(view: ViewMode) -> &'static str {
    match view {
        ViewMode::List => "List",
        ViewMode::Columns => "Columns",
        ViewMode::Grid => "Grid",
        ViewMode::Dual => "Dual pane",
    }
}

fn item(setting: Setting, label: &'static str, value: Value) -> Row {
    Row::Item {
        setting,
        label,
        value,
        caption: None,
    }
}

/// A check for whether one of the list's columns shows.
fn column(app: &App, index: usize) -> Row {
    let title = crate::ui::list::COLUMNS[index].title();
    item(
        Setting::Column(index),
        title,
        Value::Check(app.columns[index]),
    )
}

/// A section's lines as the app stands.
pub fn rows(app: &App, section: Section) -> Vec<Row> {
    match section {
        Section::View => vec![
            Row::Heading("VIEW"),
            item(
                Setting::View,
                "Last-used view",
                Value::Choice(view_name(app.view())),
            ),
            item(
                Setting::Preview,
                "Preview pane",
                Value::Check(app.preview_visible),
            ),
            item(
                Setting::Places,
                "Places panel",
                Value::Check(app.places_visible),
            ),
            Row::Item {
                setting: Setting::Detail,
                label: "Detail column",
                value: Value::Choice(app.detail.label()),
                caption: Some("columns view"),
            },
            Row::Heading("LIST VIEW COLUMNS"),
            column(app, 0),
            column(app, 1),
            column(app, 2),
            column(app, 3),
            column(app, 4),
            Row::Heading("SORTING"),
            item(
                Setting::SortBy,
                "Sort by",
                Value::Choice(app.sort.key.label()),
            ),
            item(
                Setting::Order,
                "Order",
                Value::Choice(if app.sort.reversed {
                    "Descending"
                } else {
                    "Ascending"
                }),
            ),
            item(
                Setting::Hidden,
                "Show hidden files",
                Value::Check(app.show_hidden),
            ),
            Row::Heading("OPENING"),
            item(
                Setting::Space,
                "Space previews in",
                Value::Choice(app.space_look.label()),
            ),
        ],
        Section::About => vec![
            item(
                Setting::Version,
                "Version",
                Value::Text(env!("CARGO_PKG_VERSION").to_string()),
            ),
            item(
                Setting::BasedOn,
                "Based on",
                Value::Text("flea by GM · MIT".to_string()),
            ),
            item(
                Setting::Keys,
                "Keyboard sheet",
                Value::Text("?".to_string()),
            ),
            item(
                Setting::Issue,
                "Report an issue",
                Value::Text("Open".to_string()),
            ),
        ],
    }
}

/// The rows the keyboard can be on, in order.
pub fn items(rows: &[Row]) -> Vec<Setting> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Item { setting, .. } => Some(*setting),
            Row::Heading(_) => None,
        })
        .collect()
}

/// `index` stepped by `step` among `count`, going round.
pub fn wrap(index: usize, step: isize, count: usize) -> usize {
    (index as isize + step).rem_euclid(count as isize) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{click_preview, fixture, launch, launch_one_pane, press};
    use crate::app::{Focus, SpaceLook};
    use ratatui::crossterm::event::KeyCode;
    use std::path::PathBuf;

    /// Steps the open card's keyboard to a row with `j` or `k`.
    fn reach(app: &mut crate::app::App, setting: Setting) {
        let open = app.settings.expect("the settings are open");
        let target = items(&rows(app, open.section))
            .iter()
            .position(|s| *s == setting)
            .expect("the row");
        let (key, times) = match target >= open.cursor {
            true => (KeyCode::Char('j'), target - open.cursor),
            false => (KeyCode::Char('k'), open.cursor - target),
        };
        for _ in 0..times {
            press(app, &[key]);
        }
        assert_eq!(app.settings.map(|s| s.cursor), Some(target));
    }

    #[test]
    fn comma_opens_the_settings_on_view_with_the_keyboard_in_the_rows_and_esc_closes_them() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char(',')]);
        assert_eq!(f.app.settings, Some(Settings::new()));
        // Keys that would do something else wait while it is open.
        press(&mut f.app, &[KeyCode::Char('q')]);
        assert!(!f.app.should_quit);
        press(&mut f.app, &[KeyCode::Esc]);
        assert_eq!(f.app.settings, None);
        // From the places panel and the preview too.
        press(&mut f.app, &[KeyCode::BackTab, KeyCode::Char(',')]);
        assert!(f.app.settings.is_some());
        press(&mut f.app, &[KeyCode::Esc]);
        click_preview(&mut f.app);
        assert_eq!(f.app.focus, Focus::Preview);
        press(&mut f.app, &[KeyCode::Char(',')]);
        assert!(f.app.settings.is_some());
    }

    #[test]
    fn the_view_rows_change_the_app_like_their_keys_and_are_remembered() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        press(&mut app, &[KeyCode::Char(',')]);
        let items = items(&rows(&app, Section::View));
        assert_eq!(
            items,
            [
                Setting::View,
                Setting::Preview,
                Setting::Places,
                Setting::Detail,
                Setting::Column(0),
                Setting::Column(1),
                Setting::Column(2),
                Setting::Column(3),
                Setting::Column(4),
                Setting::SortBy,
                Setting::Order,
                Setting::Hidden,
                Setting::Space
            ]
        );
        // Last-used view: `l` and `h` step it, going round to Dual pane.
        assert_eq!(app.view(), ViewMode::List);
        press(&mut app, &[KeyCode::Char('l')]);
        assert_eq!(app.view(), ViewMode::Columns);
        press(&mut app, &[KeyCode::Left, KeyCode::Char('h')]);
        assert_eq!(app.view(), ViewMode::Dual);
        assert_eq!(app.panes().len(), 2);
        press(&mut app, &[KeyCode::Right]);
        assert_eq!((app.view(), app.panes().len()), (ViewMode::List, 1));
        // Preview pane and Places panel: Space or Enter.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char(' ')]);
        assert!(!app.preview_visible);
        press(&mut app, &[KeyCode::Enter]);
        assert!(app.preview_visible);
        // `h` and `l` leave a check as it is.
        press(&mut app, &[KeyCode::Char('l')]);
        assert!(app.preview_visible);
        press(&mut app, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert!(!app.places_visible);
        // Detail column, Sort by, Order.
        reach(&mut app, Setting::Detail);
        press(&mut app, &[KeyCode::Char('l')]);
        assert_eq!(app.detail, Detail::Modified);
        // Space leaves a choice as it is.
        press(&mut app, &[KeyCode::Char(' ')]);
        assert_eq!(app.detail, Detail::Modified);
        reach(&mut app, Setting::SortBy);
        press(&mut app, &[KeyCode::Char('l')]);
        assert_eq!((app.sort.key, app.sort.reversed), (SortKey::Size, false));
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Enter]);
        assert_eq!((app.sort.key, app.sort.reversed), (SortKey::Size, true));
        // Show hidden files.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char(' ')]);
        assert!(app.show_hidden);
        // Space previews in, the last row: `j` stays on it.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('l')]);
        assert_eq!(app.space_look, SpaceLook::QuickLook);
        press(&mut app, &[KeyCode::Char('j')]);
        assert_eq!(app.settings.map(|s| s.cursor), Some(12));
        assert_eq!(app.settings.map(|s| s.saved), Some(true));
        // All of it is there at the next launch, hidden files included.
        let again = launch(&f.home, Some(state.path()));
        assert!(again.preview_visible);
        assert!(!again.places_visible);
        assert_eq!(again.detail, Detail::Modified);
        assert_eq!((again.sort.key, again.sort.reversed), (SortKey::Size, true));
        assert!(again.show_hidden);
        assert_eq!(again.space_look, SpaceLook::QuickLook);
        // `.` is remembered as well now.
        press(&mut app, &[KeyCode::Esc, KeyCode::Char('.')]);
        assert!(!launch(&f.home, Some(state.path())).show_hidden);
    }

    #[test]
    fn space_previews_in_the_terminal_or_in_macos_quick_look_as_chosen() {
        let mut f = fixture();
        let notes = f.home.join("notes.txt");
        // At first, as since the flea port: the card in the terminal.
        assert_eq!(f.app.space_look, SpaceLook::Terminal);
        press(&mut f.app, &[KeyCode::End, KeyCode::Char(' ')]);
        assert!(f.app.look.is_some());
        assert!(f.system.previewed.borrow().is_empty());
        press(&mut f.app, &[KeyCode::Esc]);
        // The row says so, under its own heading, and `l` changes it.
        let view = rows(&f.app, Section::View);
        let at = view
            .iter()
            .position(|row| *row == Row::Heading("OPENING"))
            .unwrap();
        assert_eq!(
            view[at + 1],
            Row::Item {
                setting: Setting::Space,
                label: "Space previews in",
                value: Value::Choice("The terminal"),
                caption: None,
            }
        );
        press(&mut f.app, &[KeyCode::Char(',')]);
        reach(&mut f.app, Setting::Space);
        press(&mut f.app, &[KeyCode::Char('l')]);
        let view = rows(&f.app, Section::View);
        assert!(view.contains(&Row::Item {
            setting: Setting::Space,
            label: "Space previews in",
            value: Value::Choice("macOS Quick Look"),
            caption: None,
        }));
        // Then Space is what it was before the port: macOS's window, and
        // again closes it.
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Char(' ')]);
        assert!(f.app.look.is_none());
        assert_eq!(*f.system.previewed.borrow(), [notes.clone()]);
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert_eq!(*f.system.previewed.borrow(), [notes.clone(), notes]);
        // And back.
        press(&mut f.app, &[KeyCode::Char(',')]);
        reach(&mut f.app, Setting::Space);
        press(
            &mut f.app,
            &[KeyCode::Char('h'), KeyCode::Esc, KeyCode::Char(' ')],
        );
        assert!(f.app.look.is_some());
    }

    #[test]
    fn the_columns_rows_choose_the_list_s_columns_and_are_remembered() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        // All five at first, under their heading, right after the view's rows.
        assert_eq!(app.columns, [true; 5]);
        let view = rows(&app, Section::View);
        let at = view
            .iter()
            .position(|row| *row == Row::Heading("LIST VIEW COLUMNS"))
            .unwrap();
        assert!(matches!(
            view[at - 1],
            Row::Item {
                setting: Setting::Detail,
                ..
            }
        ));
        let labels: Vec<_> = view[at + 1..at + 6]
            .iter()
            .map(|row| match row {
                Row::Item { label, value, .. } => (*label, value.clone()),
                Row::Heading(_) => panic!("a heading among the columns"),
            })
            .collect();
        assert_eq!(
            labels,
            ["Size", "User", "Permissions", "Kind", "Modified"].map(|l| (l, Value::Check(true)))
        );
        // Space hides one, Enter another.
        press(&mut app, &[KeyCode::Char(',')]);
        reach(&mut app, Setting::Column(1));
        press(&mut app, &[KeyCode::Char(' ')]);
        assert_eq!(app.columns, [true, false, true, true, true]);
        reach(&mut app, Setting::Column(4));
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.columns, [true, false, true, true, false]);
        // Remembered by name.
        let text = std::fs::read_to_string(state.path().join("state")).unwrap();
        assert!(text.contains("columns=size,permissions,kind\n"), "{text}");
        assert_eq!(launch(&f.home, Some(state.path())).columns, app.columns);
        // None at all is remembered as none.
        for column in [0, 2, 3] {
            reach(&mut app, Setting::Column(column));
            press(&mut app, &[KeyCode::Char(' ')]);
        }
        assert_eq!(launch(&f.home, Some(state.path())).columns, [false; 5]);
    }

    #[test]
    fn the_rail_goes_to_about_whose_rows_open_the_key_sheet_and_the_issues() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char(','), KeyCode::Tab]);
        assert_eq!(f.app.settings.map(|s| s.in_rail), Some(true));
        // `j` in the rail is the next section, stopping at the last.
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char('j')]);
        let open = f.app.settings.unwrap();
        assert_eq!((open.section, open.cursor), (Section::About, 0));
        let about = rows(&f.app, Section::About);
        assert_eq!(
            about[0],
            Row::Item {
                setting: Setting::Version,
                label: "Version",
                value: Value::Text(env!("CARGO_PKG_VERSION").to_string()),
                caption: None,
            }
        );
        // Enter goes into the rows; the issues page opens.
        press(
            &mut f.app,
            &[KeyCode::Enter, KeyCode::Char('j'), KeyCode::Char('j')],
        );
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Enter]);
        assert_eq!(*f.system.opened.borrow(), [PathBuf::from(ISSUES)]);
        // The key sheet takes the card's place.
        press(&mut f.app, &[KeyCode::Char('k'), KeyCode::Enter]);
        assert!(f.app.help_visible);
        assert_eq!(f.app.settings, None);
        // `k` in the rail goes back to View; each opening starts there.
        press(
            &mut f.app,
            &[KeyCode::Esc, KeyCode::Char(','), KeyCode::Tab],
        );
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char('k')]);
        assert_eq!(f.app.settings.map(|s| s.section), Some(Section::View));
    }
}
