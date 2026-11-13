//! Drawing: one function per region of the window.

mod columns;
mod details;
mod grid;
mod help;
mod list;
mod sidebar;
mod topbar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_image::Image;
use ratatui_image::protocol::Protocol;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, ViewMode};
use crate::model::Entry;
use crate::theme;
use crate::thumbs::Thumbs;

const SIDEBAR_WIDTH: u16 = 24;
const DETAILS_WIDTH: u16 = 34;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme::base()), area);
    if let Some(thumbs) = &mut app.thumbs {
        thumbs.cover(app.help_visible);
    }

    let [top, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    topbar::draw(frame, app, top);

    // Narrow terminals give the room to the files: details go first, then the sidebar.
    let details_width = if app.details_visible && area.width >= 90 {
        DETAILS_WIDTH
    } else {
        0
    };
    let sidebar_width = if area.width >= 56 { SIDEBAR_WIDTH } else { 0 };
    let [side, files, details] = Layout::horizontal([
        Constraint::Length(sidebar_width),
        Constraint::Min(0),
        Constraint::Length(details_width),
    ])
    .areas(body);

    if side.width > 0 {
        let block = Block::new()
            .borders(Borders::RIGHT)
            .border_style(theme::rule());
        let inner = block.inner(side);
        frame.render_widget(block, side);
        sidebar::draw(frame, app, inner);
    }
    draw_files(frame, app, files);
    // After the files: the selected item's big picture is asked for last, so
    // it is the first one made.
    if details.width > 0 {
        let block = Block::new()
            .borders(Borders::LEFT)
            .border_style(theme::rule());
        let inner = block.inner(details);
        frame.render_widget(block, details);
        details::draw(frame, app, inner);
    }

    if app.help_visible {
        help::draw(frame, area);
    }
}

fn draw_files(frame: &mut Frame, app: &mut App, area: Rect) {
    let band = u16::from(app.message.is_some());
    let [view, message] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(band)]).areas(area);
    let focused = app.focus == Focus::Files;
    // Lists show one blank row of air on top; the grid counts its own rows.
    app.page_rows = view.height.saturating_sub(1).max(1) as usize;
    match app.view {
        ViewMode::Grid => grid::draw(frame, app, view, focused),
        ViewMode::List => list::draw(frame, app, view, focused),
        ViewMode::Columns => columns::draw(frame, app, view, focused),
    }

    if let Some(note) = &app.message {
        let style = if note.is_error {
            theme::error()
        } else {
            theme::notice()
        };
        let text = fit(&format!("  {}", note.text), message.width as usize);
        frame.render_widget(Paragraph::new(Line::from(text)).style(style), message);
    }
}

/// Draws a file's picture in the middle of `rect`, as much of it as is inside
/// `visible` (the bottom edge of a view may cut `rect` short). False when the
/// file has no picture to show, at least not yet: the caller draws a stand-in.
/// Nothing is read or decoded here; a picture that isn't ready is asked for.
pub fn picture(
    frame: &mut Frame,
    thumbs: Option<&mut Thumbs>,
    entry: &Entry,
    rect: Rect,
    visible: Rect,
) -> bool {
    let Some(thumbs) = thumbs else { return false };
    let size = Size::new(rect.width, rect.height);
    let Some(protocol) = thumbs.get(&entry.path, entry.modified, size) else {
        return false;
    };
    let size = protocol.size();
    let whole = Rect {
        x: rect.x + rect.width.saturating_sub(size.width) / 2,
        y: rect.y + rect.height.saturating_sub(size.height) / 2,
        width: size.width.min(rect.width),
        height: size.height.min(rect.height),
    };
    let showing = whole.intersection(visible);
    if showing == whole {
        frame.render_widget(Image::new(protocol), whole);
    } else if matches!(protocol, Protocol::Kitty(_) | Protocol::Halfblocks(_)) {
        // These two can show the top of a picture; the others show all or nothing.
        if !showing.is_empty() {
            frame.render_widget(Image::new(protocol).allow_clipping(true), showing);
        }
    } else {
        return false;
    }
    true
}

/// Where a view has to start for the cursor's row to be among the `rows` it shows.
pub fn scroll(offset: usize, cursor: usize, rows: usize) -> usize {
    if cursor < offset {
        cursor
    } else if rows > 0 && cursor >= offset + rows {
        cursor + 1 - rows
    } else {
        offset
    }
}

/// A row of a list: the selected one is lit where the keyboard is, grey elsewhere.
pub fn row_style(selected: bool, focused: bool) -> Style {
    match (selected, focused) {
        (true, true) => theme::selected(),
        (true, false) => theme::selected_inactive(),
        _ => theme::base(),
    }
}

/// The glyph in front of a row: folders in accent, files dim, the lit row all one color.
pub fn icon_style(row: Style, is_dir: bool, lit: bool) -> Style {
    if lit {
        row
    } else if is_dir {
        row.fg(theme::ACCENT)
    } else {
        row.fg(theme::DIM)
    }
}

/// Shortens text to a width in cells, ending in an ellipsis when it had to cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Fits text and pads it with spaces to exactly `width` cells.
pub fn pad(text: &str, width: usize) -> String {
    let fitted = fit(text, width);
    let gap = width.saturating_sub(fitted.width());
    format!("{fitted}{}", " ".repeat(gap))
}

/// Like [`pad`], with the text against the right edge.
pub fn pad_left(text: &str, width: usize) -> String {
    let fitted = fit(text, width);
    let gap = width.saturating_sub(fitted.width());
    format!("{}{fitted}", " ".repeat(gap))
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::ops::Range;

    /// Draws the app at a size and returns the cells, for tests that look at colors.
    pub fn buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Draws the app at a size and returns the screen as lines of text.
    pub fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let buffer = buffer(app, width, height);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// The first cell, reading down the screen, where a piece of text starts
    /// within a range of columns.
    pub fn find(buffer: &Buffer, text: &str, columns: Range<u16>) -> Option<(u16, u16)> {
        let cells: Vec<&str> = text.split_inclusive(|_| true).collect();
        let last = columns.end.min(buffer.area.width);
        (0..buffer.area.height).find_map(|y| {
            columns
                .clone()
                .find(|x| {
                    let end = x + cells.len() as u16;
                    end <= last
                        && cells
                            .iter()
                            .zip(*x..end)
                            .all(|(cell, x)| buffer[(x, y)].symbol() == *cell)
                })
                .map(|x| (x, y))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::screen;
    use super::testing::{buffer, find};
    use super::*;
    use crate::app::testing::{ctrl, fixture, press};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn fit_cuts_with_an_ellipsis_and_pad_fills_the_width() {
        assert_eq!(fit("harbour-dusk.jpg", 20), "harbour-dusk.jpg");
        assert_eq!(fit("harbour-dusk.jpg", 8), "harbour…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad_left("ab", 4), "  ab");
        assert_eq!(pad("abcdef", 4), "abc…");
    }

    #[test]
    fn the_window_shows_breadcrumbs_sidebar_files_and_details() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        let lines = screen(&mut f.app, 120, 24);
        let all = lines.join("\n");
        assert!(lines[0].contains("~ / Pictures"), "{}", lines[0]);
        assert!(lines[1].starts_with("────"), "{}", lines[1]);
        assert!(all.contains("HOME"));
        assert!(all.contains("Documents"));
        assert!(all.contains("Trip"));
        // The selected file: in the list and named in the details pane.
        assert!(all.matches("a.jpg").count() >= 2, "{all}");
        assert!(all.contains("JPEG image · 3 B"), "{all}");
        assert!(all.contains("Modified"));
        assert!(all.contains("Open"));
    }

    #[test]
    fn narrow_windows_drop_the_details_then_the_sidebar() {
        let mut f = fixture();
        let medium = screen(&mut f.app, 70, 20).join("\n");
        assert!(medium.contains("HOME") && !medium.contains("Modified"));
        let narrow = screen(&mut f.app, 40, 20).join("\n");
        assert!(!narrow.contains("HOME") && narrow.contains("notes.txt"));
    }

    #[test]
    fn an_empty_folder_says_so() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        let all = screen(&mut f.app, 100, 20).join("\n");
        assert!(all.contains("No items"), "{all}");
        assert!(all.contains("Nothing selected"), "{all}");
    }

    #[test]
    fn the_help_overlay_lists_the_shortcuts() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let all = screen(&mut f.app, 120, 40).join("\n");
        assert!(all.contains("Keyboard shortcuts"));
        assert!(all.contains("Go to the parent folder"));
        for (keys, does) in [
            ("Ctrl+1 / 2 / 3", "Switch the view: grid, list, columns"),
            ("→ / ←", "In list and columns: into a folder / back out"),
            (
                "Ctrl+= / Ctrl+- / Ctrl+0",
                "Bigger, smaller or normal tiles",
            ),
            ("Ctrl+I", "Show or hide the details pane"),
            ("Ctrl+H", "Show hidden files"),
            ("Space", "Preview the selected file"),
        ] {
            let line = format!("{keys:<26}{does}");
            assert!(all.contains(&line), "{line} is missing from\n{all}");
        }
    }

    #[test]
    fn ctrl_1_2_3_draw_the_grid_the_list_and_the_columns() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let grid = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&grid, "▦", 0..120).map(|(_, y)| y), Some(0));
        assert_eq!(find(&grid, "notes.txt", 24..86), Some((71, 10)));

        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let list = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&list, "≡", 0..120).map(|(_, y)| y), Some(0));
        assert_eq!(find(&list, "notes.txt", 24..86), Some((28, 5)));
        assert_eq!(find(&list, "5 B", 24..86).map(|(_, y)| y), Some(5));

        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let columns = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&columns, "▥", 0..120).map(|(_, y)| y), Some(0));
        assert_eq!(find(&columns, "notes.txt", 24..86), Some((28, 5)));
        assert_eq!(find(&columns, "5 B", 24..86), None);
        assert_eq!(find(&columns, "›", 24..86), Some((48, 3)));
    }

    #[test]
    fn the_top_bar_lights_the_hidden_and_details_toggles_that_are_on() {
        let mut f = fixture();
        let before = buffer(&mut f.app, 120, 24);
        let (hidden, _) = find(&before, "◉", 0..120).unwrap();
        let (details, _) = find(&before, "◨", 0..120).unwrap();
        assert_eq!(before[(hidden, 0)].bg, theme::BACKGROUND);
        assert_eq!(before[(details, 0)].bg, theme::SELECTION);
        assert!(find(&before, "Modified", 86..120).is_some());
        assert_eq!(find(&before, ".hidden", 24..86), None);

        f.app.handle_key(ctrl(KeyCode::Char('h')));
        f.app.handle_key(ctrl(KeyCode::Char('i')));
        let after = buffer(&mut f.app, 120, 24);
        assert_eq!(after[(hidden, 0)].bg, theme::SELECTION);
        assert_eq!(after[(details, 0)].bg, theme::BACKGROUND);
        assert_eq!(find(&after, "Modified", 0..120), None);
        assert_eq!(find(&after, ".hidden", 24..120), Some((28, 5)));
    }
}
