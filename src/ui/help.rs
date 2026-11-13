//! The `?` overlay: every shortcut that works. Whatever the size of the
//! window, all of them can be reached: side by side in two columns when it is
//! short and wide, and by scrolling when they still don't fit.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{pad, pad_left};
use crate::app::App;
use crate::theme;

const SHORTCUTS: &[(&str, &str)] = &[
    ("← ↑ → ↓", "Move the selection"),
    ("→ / ←", "In list and columns: into a folder / back out"),
    (
        "Tab / Shift+Tab",
        "Move between the sidebar, the panes and the details",
    ),
    ("⏎", "Open"),
    ("Space", "Preview the selected file"),
    ("Backspace", "Go to the parent folder"),
    ("Alt+← / Alt+→", "Back / forward"),
    ("Alt+Home", "Go home"),
    ("Ctrl+L", "Type a path (Tab completes)"),
    ("/", "Search this folder and below"),
    ("Ctrl+C / X / V", "Copy, cut, paste"),
    ("Ctrl+Shift+C", "Copy the path as text"),
    ("F2 / Ctrl+Shift+D", "Rename / duplicate"),
    ("Ctrl+Shift+N", "New folder"),
    ("Ctrl+P", "Pin or unpin the folder"),
    ("Delete in the sidebar", "Unpin the folder"),
    ("Delete (fn+⌫) / ⌘⌫", "Move to the trash"),
    ("Shift+Delete", "Delete permanently (asks first)"),
    (
        "Ctrl+Z / Ctrl+Shift+Z",
        "Undo / redo the last change to files (or Ctrl+Y)",
    ),
    (
        "Escape",
        "Stop the file work under way, or dismiss what went wrong",
    ),
    ("Ctrl+1 / 2 / 3", "Switch the view: grid, list, columns"),
    (
        "Ctrl+= / Ctrl+- / Ctrl+0",
        "Bigger, smaller or normal tiles",
    ),
    ("Ctrl+I", "Show or hide the details pane"),
    ("S / Shift+S", "Sort: name, kind, newest, oldest, size"),
    ("Ctrl+H", "Show hidden files"),
    ("F5", "Reload"),
    ("?", "Show these shortcuts"),
    ("Q", "Close the window"),
];

const KEY_WIDTH: usize = 26;
/// The longest line: its keys, and the 56 cells of what they do.
const LINE_WIDTH: usize = KEY_WIDTH + 56;
/// Between two columns.
const GAP: usize = 4;
/// The border and the air left and right of the lines.
const SIDES: usize = 8;

const TITLE: &str = "Keyboard shortcuts";

/// One line of a column: the keys, or nothing where the line above goes on.
type Row = (&'static str, String);

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let (area_width, area_height) = (area.width as usize, area.height as usize);
    // With room to spare: a margin around the panel and air above and below
    // the lines, which leaves this many rows for them.
    let roomy_rows = area_height.saturating_sub(8);
    let one_width = (LINE_WIDTH + SIDES).min(area_width.saturating_sub(4));
    let one = column(SHORTCUTS, one_width.saturating_sub(SIDES));
    // Two columns when one is too tall and both fit whole, side by side.
    let two_width = 2 * LINE_WIDTH + GAP + SIDES;
    let (columns, width) = if one.len() > roomy_rows && area_width >= two_width + 4 {
        let (left, right) = SHORTCUTS.split_at(SHORTCUTS.len().div_ceil(2));
        let columns = vec![column(left, LINE_WIDTH), column(right, LINE_WIDTH)];
        (columns, two_width)
    } else {
        (vec![one], one_width)
    };

    // A window still too short gives up the margin and the air, then scrolls.
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0);
    let (air, rows) = if tallest <= roomy_rows {
        (1, roomy_rows)
    } else {
        (0, area_height.saturating_sub(4))
    };
    let showing = tallest.min(rows);
    let last = tallest - showing;
    app.help_scroll = app.help_scroll.min(last);
    app.help_rows = showing;
    let first = app.help_scroll;

    let height = (showing + 4 + 2 * air).min(area_height);
    let overlay = Rect {
        x: area.x + ((area_width - width) / 2) as u16,
        y: area.y + ((area_height - height) / 2) as u16,
        width: width as u16,
        height: height as u16,
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme::rule())
        .padding(Padding::new(3, 3, air as u16, air as u16))
        .style(theme::base().bg(theme::DARK_BACKGROUND));

    let inner_width = width.saturating_sub(SIDES);
    let more = match (first > 0, first < last) {
        (true, true) => "↑↓ more",
        (true, false) => "↑ more",
        (false, true) => "↓ more",
        (false, false) => "",
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(TITLE, theme::title()),
            Span::styled(
                pad_left(more, inner_width.saturating_sub(TITLE.len())),
                theme::dim(),
            ),
        ]),
        Line::default(),
    ];
    let column_width = match columns.len() {
        1 => inner_width,
        _ => LINE_WIDTH + GAP,
    };
    for row in first..first + showing {
        let mut spans = Vec::new();
        for column in &columns {
            let (keys, does) = match column.get(row) {
                Some((keys, does)) => (*keys, does.as_str()),
                None => ("", ""),
            };
            spans.push(Span::styled(pad(keys, KEY_WIDTH), theme::accent()));
            spans.push(Span::styled(
                pad(does, column_width.saturating_sub(KEY_WIDTH)),
                theme::base().bg(theme::DARK_BACKGROUND),
            ));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Clear, overlay);
    frame.render_widget(Paragraph::new(lines).block(block), overlay);
}

/// The lines of a column `width` cells wide: what the keys do goes on under
/// itself where it is too long for one line.
fn column(shortcuts: &[(&'static str, &str)], width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for (keys, does) in shortcuts {
        let wrapped = wrap(does, width.saturating_sub(KEY_WIDTH));
        let keys = std::iter::once(*keys).chain(std::iter::repeat(""));
        rows.extend(keys.zip(wrapped));
    }
    rows
}

/// Breaks text between words into lines of at most `width` cells. A word
/// longer than that has a line to itself.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::testing::{fixture, press};
    use crate::ui::testing::screen;
    use ratatui::crossterm::event::KeyCode;

    fn show(app: &mut App, width: u16, height: u16) -> String {
        screen(app, width, height).join("\n")
    }

    /// Opens the shortcuts in a window of a size and reads them to the end,
    /// one line down at a time: every screen that showed on the way.
    fn read_through(app: &mut App, width: u16, height: u16) -> Vec<String> {
        press(app, &[KeyCode::Char('?')]);
        let mut screens = vec![show(app, width, height)];
        loop {
            press(app, &[KeyCode::Down]);
            let next = show(app, width, height);
            if screens.last() == Some(&next) {
                return screens;
            }
            screens.push(next);
        }
    }

    #[test]
    fn at_24_rows_every_shortcut_can_be_reached() {
        let mut f = fixture();
        let screens = read_through(&mut f.app, 120, 24);
        for (keys, does) in SHORTCUTS {
            let line = format!("{keys:<26}{does}");
            assert!(screens.iter().any(|shown| shown.contains(&line)), "{line}");
        }
        // It doesn't all fit, and says so: more below at first, above at the end.
        assert!(screens.len() > 2);
        let (first, last) = (&screens[0], &screens[screens.len() - 1]);
        assert!(first.contains("Keyboard shortcuts") && first.contains("↓ more"));
        assert!(!first.contains("↑ more") && !first.contains("↑↓ more"));
        assert!(screens[1].contains("↑↓ more"));
        assert!(last.contains("↑ more") && !last.contains("↓ more"));
        assert!(last.contains("Keyboard shortcuts"), "the title stays put");
    }

    #[test]
    fn in_a_narrow_window_what_the_keys_do_wraps_and_every_shortcut_can_be_reached() {
        let mut f = fixture();
        let screens = read_through(&mut f.app, 80, 24);
        assert!(screens.len() > 2);
        for (keys, does) in SHORTCUTS {
            let mut words = does.split(' ');
            let start = format!("{keys:<26}{}", words.next().unwrap());
            let whole = |shown: &&String| {
                shown.contains(&start) && does.split(' ').all(|word| shown.contains(word))
            };
            assert!(screens.iter().any(|shown| whole(&shown)), "{keys}: {does}");
        }
        assert!(screens[0].contains("↓ more"));
        // Too long for one line here, it goes on under itself.
        let wrapped = screens.iter().any(|shown| {
            shown.contains("Stop the file work under way, or dismiss")
                && !shown.contains("dismiss what went wrong")
                && shown.contains("what went wrong")
        });
        assert!(wrapped);
    }

    #[test]
    fn a_window_tall_enough_shows_them_all_at_once_as_before() {
        let mut f = fixture();
        let screens = read_through(&mut f.app, 120, 40);
        assert_eq!(screens.len(), 1, "nothing to scroll");
        assert!(!screens[0].contains("more"));
        for (keys, does) in SHORTCUTS {
            let line = format!("{keys:<26}{does}");
            assert!(screens[0].contains(&line), "{line}");
        }
    }

    #[test]
    fn a_short_window_that_is_wide_enough_shows_two_columns() {
        let mut f = fixture();
        let screens = read_through(&mut f.app, 200, 24);
        assert_eq!(screens.len(), 1, "it all fits side by side");
        assert!(!screens[0].contains("more"));
        for (keys, does) in SHORTCUTS {
            let line = format!("{keys:<26}{does}");
            assert!(screens[0].contains(&line), "{line}");
        }
        // The second half starts next to the first shortcut.
        let (first, half) = (SHORTCUTS[0], SHORTCUTS[SHORTCUTS.len().div_ceil(2)]);
        let row = |shown: &str, keys: &str| {
            let wanted = format!("{keys:<26}");
            shown.lines().position(|line| line.contains(&wanted))
        };
        assert!(row(&screens[0], first.0).is_some());
        assert_eq!(row(&screens[0], first.0), row(&screens[0], half.0));
        // With room for one column, it is one column.
        let tall = show(&mut f.app, 200, 50);
        assert_ne!(row(&tall, first.0), row(&tall, half.0));
    }

    #[test]
    fn the_overlay_scrolls_by_lines_and_pages_and_keeps_every_key_to_itself() {
        let mut f = fixture();
        let at = f.app.pane().cursor;
        let (first, second) = (SHORTCUTS[0], SHORTCUTS[1]);
        let last = SHORTCUTS[SHORTCUTS.len() - 1];
        let has =
            |shown: &str, (keys, does): (&str, &str)| shown.contains(&format!("{keys:<26}{does}"));
        press(&mut f.app, &[KeyCode::Char('?')]);
        let top = show(&mut f.app, 120, 24);
        assert!(has(&top, first) && !has(&top, last));

        press(&mut f.app, &[KeyCode::Down]);
        let one_down = show(&mut f.app, 120, 24);
        assert!(!has(&one_down, first) && has(&one_down, second));
        press(&mut f.app, &[KeyCode::Up]);
        assert_eq!(show(&mut f.app, 120, 24), top);
        press(&mut f.app, &[KeyCode::Up, KeyCode::PageUp]);
        assert_eq!(
            show(&mut f.app, 120, 24),
            top,
            "the top is as far as it goes"
        );

        press(&mut f.app, &[KeyCode::PageDown]);
        let end = show(&mut f.app, 120, 24);
        assert!(
            has(&end, last) && !has(&end, second),
            "a page reaches further"
        );
        press(&mut f.app, &[KeyCode::PageDown, KeyCode::Down]);
        assert_eq!(
            show(&mut f.app, 120, 24),
            end,
            "the end is as far as it goes"
        );
        press(&mut f.app, &[KeyCode::PageUp]);
        assert_eq!(show(&mut f.app, 120, 24), top);

        // None of that reached the files underneath, and nothing else does.
        press(
            &mut f.app,
            &[KeyCode::Enter, KeyCode::Char('q'), KeyCode::End],
        );
        assert_eq!(f.app.pane().cursor, at);
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(f.app.help_visible && !f.app.should_quit);

        // Escape closes it, and so does `?`; opened again it starts at the top.
        press(&mut f.app, &[KeyCode::Down, KeyCode::Esc]);
        assert!(!f.app.help_visible);
        press(&mut f.app, &[KeyCode::Char('?')]);
        assert_eq!(show(&mut f.app, 120, 24), top);
        press(&mut f.app, &[KeyCode::Char('?')]);
        assert!(!f.app.help_visible);
    }

    #[test]
    fn a_window_too_small_for_anything_is_still_drawn() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        for (width, height) in [(3, 2), (20, 5), (40, 10), (179, 12)] {
            show(&mut f.app, width, height);
        }
    }
}
