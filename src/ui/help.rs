//! The `?` sheet: the card "Keyboard Shortcuts" between the window's rules,
//! with every key the app has in flea's four groups, each group whole, in
//! up to three columns four cells apart. It scrolls when they don't fit.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::card::{self, ESC};
use super::{fit, pad, pad_left, words};
use crate::app::{App, HitMap, Spot};
use crate::theme;

pub const TITLE: &str = "Keyboard Shortcuts";

/// The groups, as flea heads them.
pub const GROUPS: [&str; 4] = ["MOVE", "LOOK", "FIND", "CHANGE"];

/// Each key the app has: its group, the keys as flea's sheet spells them,
/// and what they do in flea's words where flea has them.
pub const ROWS: &[(usize, &str, &str)] = &[
    (0, "j / down", "Cursor down"),
    (0, "k / up", "Cursor up"),
    (0, "g / home", "First item"),
    (0, "G / end", "Last item"),
    (0, "ctrl-d / pagedown", "Page down"),
    (0, "ctrl-u / pageup", "Page up"),
    (0, "enter", "Open"),
    (0, "h / left / backspace", "Parent folder"),
    (0, "l / right", "Browse in"),
    (0, "H", "Back"),
    (0, "L", "Forward"),
    (0, "tab / shift-tab", "Cycle focus"),
    (0, "t", "New tab"),
    (0, "O", "Open in new tab"),
    (0, "w / ctrl-w", "Close tab"),
    (0, "]", "Next tab"),
    (0, "[", "Previous tab"),
    (0, "{", "Move tab left"),
    (0, "}", "Move tab right"),
    (0, "escape", "Close or go back"),
    (0, "1–9", "Select tab"),
    (0, "q", "Quit"),
    (1, "?", "Keyboard Shortcuts"),
    (1, "space", "Quick Look"),
    (1, ".", "Hidden files"),
    (1, "I", "Detail column"),
    (1, "alt-p", "Preview column"),
    (1, "ctrl-b", "Places panel"),
    (1, ",", "Settings"),
    (1, "ctrl-opt-1", "List view"),
    (1, "ctrl-opt-2", "Columns view"),
    (1, "ctrl-opt-3", "Grid view"),
    (1, "ctrl-opt-4 / | / alt-1", "Dual pane"),
    (1, "+", "Zoom in"),
    (1, "-", "Zoom out"),
    (2, "/", "Filter"),
    (2, "f / ctrl-f", "Find"),
    (2, "ctrl-g", "Search contents"),
    (2, "o", "Reveal"),
    (2, "s", "Sort"),
    (2, "S", "Reverse sort"),
    (2, ": / ctrl-l", "Go to folder"),
    (2, "ctrl-r / f5", "Reload"),
    (3, "y / ctrl-c", "Copy"),
    (3, "x / ctrl-x", "Cut"),
    (3, "p / ctrl-v", "Paste"),
    (3, "M", "Move here"),
    (3, "U", "Duplicate"),
    (3, "r / f2", "Rename"),
    (3, "dd / delete", "Move to Trash"),
    (3, "shift-delete", "Delete permanently"),
    (3, "z / ctrl-z", "Undo"),
    (3, "ctrl-shift-z / ctrl-y", "Redo"),
    (3, "ctrl-shift-n / N", "New folder"),
    (3, "E", "Edit in $EDITOR"),
    (3, "v", "Select"),
    (3, "J / shift-down", "Extend down"),
    (3, "K / shift-up", "Extend up"),
    (3, "V", "Invert selection"),
    (3, "ctrl-a", "Select all"),
    (3, "T", "Tags"),
    (3, "ctrl-shift-c", "Copy path"),
    (3, "Y", "Copy folder path"),
    (3, "m / menu", "Menu"),
    (3, "alt-t", "Open terminal"),
];

/// At most three columns, four cells apart.
const COLUMNS: usize = 3;
const GAP: usize = 4;

/// A line of the sheet: a group's heading, a key beside its words, or on a
/// screen too narrow for that, a key or its words on a line of their own.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sheet {
    Heading(&'static str),
    Binding(&'static str, &'static str),
    Key(String),
    Words(String),
}

fn groups() -> Vec<Vec<Sheet>> {
    let group = |index: usize| {
        let mut lines = vec![Sheet::Heading(GROUPS[index])];
        let rows = ROWS.iter().filter(|(group, _, _)| *group == index);
        lines.extend(rows.map(|(_, keys, does)| Sheet::Binding(keys, does)));
        lines
    };
    (0..GROUPS.len()).map(group).collect()
}

/// Groups fill a column in order, and the next group starts a new column
/// when it would not fit whole.
fn pack(groups: &[Vec<Sheet>], capacity: usize) -> Vec<Vec<Sheet>> {
    let mut columns: Vec<Vec<Sheet>> = Vec::new();
    for group in groups {
        let full =
            |column: &Vec<Sheet>| !column.is_empty() && column.len() + group.len() > capacity;
        if columns.last().is_none_or(full) {
            columns.push(Vec::new());
        }
        if let Some(column) = columns.last_mut() {
            column.extend(group.iter().cloned());
        }
    }
    columns
}

/// A column's key width and words width.
fn widths(column: &[Sheet]) -> (usize, usize) {
    let key = column
        .iter()
        .map(|line| match line {
            Sheet::Binding(keys, _) => keys.width(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let words = column
        .iter()
        .map(|line| match line {
            Sheet::Binding(_, does) => does.width(),
            Sheet::Heading(text) => text.width().saturating_sub(key + 2),
            Sheet::Key(text) | Sheet::Words(text) => text.width().saturating_sub(key + 2),
        })
        .max()
        .unwrap_or(0);
    (key, words)
}

fn total(columns: &[Vec<Sheet>]) -> usize {
    let each = columns.iter().map(|column| {
        let (key, words) = widths(column);
        key + 2 + words
    });
    each.sum::<usize>() + GAP * columns.len().saturating_sub(1)
}

/// The most columns, up to three, that fit `room` cells with the fewest
/// rows, `rows` being what the card shows; a screen too narrow for one
/// column of keys beside their words has each on lines of their own.
fn sheet(rows: usize, room: usize) -> Vec<Vec<Sheet>> {
    let groups = groups();
    for count in (1..=COLUMNS).rev() {
        let mut capacity = rows.max(1);
        while pack(&groups, capacity).len() > count {
            capacity += 1;
        }
        let columns = pack(&groups, capacity);
        if total(&columns) <= room {
            return columns;
        }
    }
    let wrap = |text: &str| words(text, room);
    let stacked = groups.iter().flatten().flat_map(|line| match line {
        Sheet::Binding(keys, does) => {
            let keys = wrap(keys).into_iter().map(Sheet::Key);
            keys.chain(wrap(does).into_iter().map(Sheet::Words))
                .collect()
        }
        other => vec![other.clone()],
    });
    vec![stacked.collect()]
}

/// Draws the sheet over `area`, the whole window, from the row under the
/// top rule to the one over the bottom rule. Scrolling is kept in range.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, hits: &mut HitMap) {
    let outer_height = area.height.saturating_sub(4);
    // Inside the frame: the title, the rule, the rows and a blank last row
    // whenever there is room for one.
    let inside = outer_height.saturating_sub(2) as usize;
    let rows = if inside > 4 {
        inside - 3
    } else {
        inside.saturating_sub(2)
    };
    let room = card::room(area.width) as usize;
    let columns = sheet(rows, room);
    let heading = TITLE.width() + 2 + ESC.width();
    let need = total(&columns).max(heading).min(room) as u16;
    let air = card::air(area.width);
    let width = need + 2 * air + 2;
    let outer = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + 2,
        width: width.min(area.width),
        height: outer_height,
    };
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0);
    let limit = tallest.saturating_sub(rows);
    app.help_scroll = app.help_scroll.min(limit);
    app.help_rows = rows;
    let card = card::placed(outer, air, rows as u16, &[], app.help_scroll);
    card::draw_frame(frame, &card, TITLE);
    hits.add(card.outer, Spot::Help);

    let (mut x, end) = (card.body.x as usize, card.body.right() as usize);
    for column in &columns {
        let (key, does) = widths(column);
        let width = (key + 2 + does).min(end.saturating_sub(x));
        let lines: Vec<Line> = column
            .iter()
            .skip(card.top)
            .take(rows)
            .map(|line| sheet_line(line, key, does, width))
            .collect();
        let cells = Rect::new(x as u16, card.body.y, width as u16, card.body.height);
        frame.render_widget(Paragraph::new(lines), cells);
        x += key + 2 + does + GAP;
        if x >= end {
            break;
        }
    }
}

/// One line of a column: a heading in the metadata ink; keys right-aligned
/// in it, two cells, then their words in the foreground.
fn sheet_line(line: &Sheet, key: usize, does: usize, width: usize) -> Line<'static> {
    match line {
        Sheet::Heading(text) => Line::from(Span::styled(fit(text, width), theme::muted())),
        Sheet::Key(text) => Line::from(Span::styled(fit(text, width), theme::muted())),
        Sheet::Words(text) => Line::from(Span::styled(fit(text, width), theme::base())),
        Sheet::Binding(keys, words) => {
            let key = key.min(width);
            Line::from(vec![
                Span::styled(pad_left(keys, key), theme::muted()),
                Span::raw("  "),
                Span::styled(
                    pad(words, does.min(width.saturating_sub(key + 2))),
                    theme::base(),
                ),
            ])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::testing::{fixture, press};
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};

    fn show(app: &mut App, width: u16, height: u16) -> String {
        screen(app, width, height).join("\n")
    }

    /// Where a heading starts.
    fn heading(drawn: &Buffer, name: &str) -> (u16, u16) {
        find(drawn, name, 0..drawn.area.width).unwrap_or_else(|| panic!("{name}"))
    }

    #[test]
    fn the_sheet_is_a_card_from_row_3_to_the_bottom_rule_with_flea_s_groups_in_columns() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let drawn = buffer(&mut f.app, 140, 40);
        // From the row under the top rule to the one over the bottom rule.
        let (x, y) = find(&drawn, "┌", 0..140).expect("the frame");
        assert_eq!(y, 2);
        assert_eq!(drawn[(x, 37)].symbol(), "└");
        assert_eq!(drawn[(x, y)].fg, theme::MUTED);
        let title = find(&drawn, TITLE, 0..140).expect("the title");
        assert_eq!(title, (x + 3, 3));
        assert!(drawn[title].modifier.contains(Modifier::BOLD));
        assert_eq!(drawn[title].bg, Color::Reset);
        assert!(find(&drawn, "esc", title.0..140).is_some());
        assert_eq!(drawn[(title.0, 4)].fg, theme::CARD_RULE);

        // Three columns at 140 across: Move; Look and Find; Change.
        let (move_x, look_x, find_x, change_x) = (
            heading(&drawn, "MOVE"),
            heading(&drawn, "LOOK"),
            heading(&drawn, "FIND"),
            heading(&drawn, "CHANGE"),
        );
        assert_eq!(move_x, (title.0, 5));
        assert_eq!(look_x.1, 5);
        assert_eq!(find_x, (look_x.0, 5 + 14), "under Look's heading and keys");
        assert_eq!(change_x.1, 5);
        assert!(move_x.0 < look_x.0 && look_x.0 < change_x.0);
        assert_eq!(
            drawn[move_x].fg,
            theme::MUTED,
            "headings in the metadata ink"
        );
        // Four cells between columns: Move's widest is its keys, 20 cells,
        // two of space and its widest words, "Close or go back", 16.
        assert_eq!(look_x.0, move_x.0 + 20 + 2 + 16 + 4);
        // Keys right-aligned in the metadata ink, words in the foreground.
        let quit = find(&drawn, "Quit", 0..140).expect("Quit");
        assert_eq!(drawn[(quit.0 - 3, quit.1)].symbol(), "q");
        assert_eq!(drawn[(quit.0 - 3, quit.1)].fg, theme::MUTED);
        assert_eq!(drawn[quit].fg, theme::FOREGROUND);
        let parent = find(&drawn, "Parent folder", 0..140).expect("Parent folder");
        assert_eq!(parent.0, quit.0);
        assert_eq!(
            find(&drawn, "h / left / backspace", 0..140),
            Some((move_x.0, parent.1))
        );
        // Behind it, everything dimmed.
        assert_eq!(drawn[(1, 0)].fg, theme::DIMMED);
    }

    #[test]
    fn every_key_the_app_has_is_on_the_sheet_once() {
        let keys: Vec<&str> = ROWS.iter().map(|(_, keys, _)| *keys).collect();
        for wanted in [
            "j / down",
            "space",
            "/",
            "f / ctrl-f",
            "ctrl-g",
            "o",
            "T",
            "I",
            "alt-p",
            "ctrl-opt-1",
            "ctrl-opt-2",
            "ctrl-opt-3",
            "ctrl-opt-4 / | / alt-1",
            "alt-t",
            ",",
            "t",
            "w / ctrl-w",
            "1–9",
            "dd / delete",
            "shift-delete",
            "m / menu",
            "ctrl-shift-c",
            "Y",
            "E",
            "M",
            "U",
            "tab / shift-tab",
            "ctrl-b",
        ] {
            assert_eq!(
                keys.iter().filter(|keys| **keys == wanted).count(),
                1,
                "{wanted}"
            );
        }
        assert_eq!(keys.len(), ROWS.len());
        let says = |wanted: &str| ROWS.iter().find(|(_, keys, _)| *keys == wanted);
        assert_eq!(
            says("tab / shift-tab"),
            Some(&(0, "tab / shift-tab", "Cycle focus"))
        );
        assert_eq!(says("ctrl-b"), Some(&(1, "ctrl-b", "Places panel")));
        let words: Vec<&str> = ROWS.iter().map(|(_, _, does)| *does).collect();
        for flea in [
            "Close or go back",
            "Quick Look",
            "Go to folder",
            "Delete permanently",
        ] {
            assert!(words.contains(&flea), "{flea}");
        }
    }

    #[test]
    fn narrower_windows_get_two_columns_then_one_then_keys_over_their_words() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let two = buffer(&mut f.app, 120, 40);
        assert_eq!(heading(&two, "LOOK").0, heading(&two, "MOVE").0);
        assert_eq!(heading(&two, "FIND").1, 5);
        assert_eq!(heading(&two, "CHANGE").0, heading(&two, "FIND").0);
        let one = buffer(&mut f.app, 50, 40);
        let move_x = heading(&one, "MOVE");
        assert_eq!(move_x.1, 5);
        assert_eq!(heading(&one, "LOOK"), (move_x.0, 5 + 23), "under Move");
        assert!(find(&one, "CHANGE", 0..50).is_none(), "further down");
        let narrow = show(&mut f.app, 24, 40);
        assert!(narrow.contains("j / down"));
        assert!(!narrow.contains("j / down  Cursor down"));
        assert!(narrow.contains("Cursor down"));
    }

    #[test]
    fn the_sheet_scrolls_and_keeps_every_key_to_itself_until_it_closes() {
        let mut f = fixture();
        let at = f.app.pane().cursor;
        press(&mut f.app, &[KeyCode::Char('?')]);
        let top = show(&mut f.app, 60, 16);
        // Seven rows a page: the frame, the title, the rule and a blank
        // last row aside.
        assert_eq!(f.app.help_rows, 7);
        press(&mut f.app, &[KeyCode::Char('j')]);
        assert_eq!(f.app.help_scroll, 1);
        let one_down = show(&mut f.app, 60, 16);
        assert_ne!(one_down, top);
        press(&mut f.app, &[KeyCode::Char('k')]);
        assert_eq!(show(&mut f.app, 60, 16), top);
        press(&mut f.app, &[KeyCode::Up, KeyCode::PageUp]);
        assert_eq!(
            show(&mut f.app, 60, 16),
            top,
            "the top is as far as it goes"
        );
        press(&mut f.app, &[KeyCode::PageDown, KeyCode::Down]);
        show(&mut f.app, 60, 16);
        assert_eq!(f.app.help_scroll, 8);
        // Paged to the end: the last of Change's keys shows.
        for _ in 0..20 {
            press(&mut f.app, &[KeyCode::PageDown]);
        }
        let end = show(&mut f.app, 60, 16);
        assert!(end.contains("Open terminal"));
        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(
            show(&mut f.app, 60, 16),
            end,
            "the end is as far as it goes"
        );

        // Nothing reaches the files underneath.
        press(
            &mut f.app,
            &[KeyCode::End, KeyCode::Char('d'), KeyCode::Char(' ')],
        );
        assert_eq!(f.app.pane().cursor, at);
        assert!(f.app.help_visible && f.app.look.is_none());
        // Escape, Enter, `q` and `?` close it, and `q` quits nothing.
        press(&mut f.app, &[KeyCode::Esc]);
        assert!(!f.app.help_visible);
        for close in [KeyCode::Enter, KeyCode::Char('q'), KeyCode::Char('?')] {
            press(&mut f.app, &[KeyCode::Char('?')]);
            assert_eq!(show(&mut f.app, 60, 16), top, "opened again at the top");
            press(&mut f.app, &[close]);
            assert!(!f.app.help_visible, "{close:?}");
        }
        assert!(!f.app.should_quit);
        assert_eq!(f.app.pane().cwd, f.home);
    }

    #[test]
    fn a_window_too_small_for_anything_is_still_drawn() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        for (width, height) in [(3, 2), (20, 5), (12, 6), (40, 7), (179, 12)] {
            show(&mut f.app, width, height);
        }
    }
}
