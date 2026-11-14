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
            Setting::View | Setting::Detail | Setting::SortBy | Setting::Order => Kind::Choice,
            Setting::Preview | Setting::Places | Setting::Hidden => Kind::Check,
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
    use crate::app::Focus;
    use crate::app::testing::{click_preview, fixture, launch, launch_one_pane, press};
    use ratatui::crossterm::event::KeyCode;
    use std::path::PathBuf;

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
                Setting::SortBy,
                Setting::Order,
                Setting::Hidden
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
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('l')]);
        assert_eq!(app.detail, Detail::Modified);
        // Space leaves a choice as it is.
        press(&mut app, &[KeyCode::Char(' ')]);
        assert_eq!(app.detail, Detail::Modified);
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('l')]);
        assert_eq!((app.sort.key, app.sort.reversed), (SortKey::Size, false));
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Enter]);
        assert_eq!((app.sort.key, app.sort.reversed), (SortKey::Size, true));
        // Show hidden files, the last row: `j` stays on it.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char(' ')]);
        assert!(app.show_hidden);
        press(&mut app, &[KeyCode::Char('j')]);
        assert_eq!(app.settings.map(|s| s.cursor), Some(6));
        assert_eq!(app.settings.map(|s| s.saved), Some(true));
        // All of it is there at the next launch, hidden files included.
        let again = launch(&f.home, Some(state.path()));
        assert!(again.preview_visible);
        assert!(!again.places_visible);
        assert_eq!(again.detail, Detail::Modified);
        assert_eq!((again.sort.key, again.sort.reversed), (SortKey::Size, true));
        assert!(again.show_hidden);
        // `.` is remembered as well now.
        press(&mut app, &[KeyCode::Esc, KeyCode::Char('.')]);
        assert!(!launch(&f.home, Some(state.path())).show_hidden);
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
