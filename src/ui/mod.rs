//! Drawing: one function per region of the window.

mod columns;
mod details;
mod dialog;
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

use crate::app::{App, Focus, Pane, ViewMode};
use crate::model::Entry;
use crate::theme;
use crate::thumbs::Thumbs;

const SIDEBAR_WIDTH: u16 = 24;
const DETAILS_WIDTH: u16 = 34;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme::base()), area);
    if let Some(thumbs) = &mut app.thumbs {
        thumbs.cover(app.help_visible || app.dialog.is_some());
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
        help::draw(frame, app, area);
    }
    if let Some(dialog) = &app.dialog {
        dialog::draw(frame, dialog, area);
    }
}

fn draw_files(frame: &mut Frame, app: &mut App, area: Rect) {
    // Under the view: what file work is under way, then what was last said.
    let [view, working, message] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(u16::from(app.working().is_some())),
        Constraint::Length(u16::from(app.message.is_some())),
    ])
    .areas(area);
    if app.panes.len() > 1 {
        // Two panes side by side with a hairline between them, each under
        // its own path: the top bar only has room for the active one's.
        let left = view.width.saturating_sub(1).div_ceil(2);
        let [left, rule, right] = Layout::horizontal([
            Constraint::Length(left),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(view);
        let hairline = Block::new()
            .borders(Borders::LEFT)
            .border_style(theme::rule());
        frame.render_widget(hairline, rule);
        for (pane, area) in [left, right].into_iter().enumerate() {
            let [path, files] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
            topbar::draw_pane_path(frame, app, pane, path);
            draw_pane(frame, app, pane, files);
        }
    } else {
        draw_pane(frame, app, app.active, view);
    }

    if let Some(label) = app.working() {
        let text = fit(&format!("  {label}"), working.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(text)).style(theme::working()),
            working,
        );
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

/// One pane's items in the view that is on.
fn draw_pane(frame: &mut Frame, app: &mut App, pane: usize, area: Rect) {
    let focused = app.focus == Focus::Files && app.active == pane;
    // Lists show one blank row of air on top; the grid counts its own rows.
    app.panes[pane].page_rows = area.height.saturating_sub(1).max(1) as usize;
    match app.view {
        ViewMode::Grid => grid::draw(frame, app, pane, area, focused),
        ViewMode::List => list::draw(frame, app, pane, area, focused),
        ViewMode::Columns => columns::draw(frame, app, pane, area, focused),
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

/// What a view says in place of the items of a pane that has none.
pub fn empty_text(pane: &Pane) -> String {
    match pane.results() {
        // Still looking: it may find some yet.
        Some(search) if search.is_running() => String::new(),
        Some(search) => format!("Nothing matches “{}”", search.input.text()),
        None => "No items".to_string(),
    }
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

    /// Draws the app at a size and returns where the terminal's cursor was left.
    pub fn cursor(app: &mut App, width: u16, height: u16) -> (u16, u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let position = terminal.get_cursor_position().unwrap();
        (position.x, position.y)
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
    use crate::app::testing::{ctrl, finish_work, fixture, press, shift, split};
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
            ("Ctrl+C / X / V", "Copy, cut, paste"),
            ("Ctrl+Shift+C", "Copy the path as text"),
            ("F2 / Ctrl+Shift+D", "Rename / duplicate"),
            ("Ctrl+Shift+N", "New folder"),
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
        ] {
            let line = format!("{keys:<26}{does}");
            assert!(all.contains(&line), "{line} is missing from\n{all}");
        }
    }

    #[test]
    fn the_help_overlay_lists_the_sidebar_shortcuts() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let all = screen(&mut f.app, 120, 40).join("\n");
        for (keys, does) in [
            ("Ctrl+P", "Pin or unpin the folder"),
            ("Delete in the sidebar", "Unpin the folder"),
        ] {
            let line = format!("{keys:<26}{does}");
            assert!(all.contains(&line), "{line} is missing from\n{all}");
        }
        assert!(
            all.contains("Close the window"),
            "the last one fits:\n{all}"
        );
    }

    #[test]
    fn the_band_says_what_file_work_is_under_way_and_then_how_it_went() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        press(&mut f.app, &[KeyCode::Home, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        // Refused meanwhile: said in a row of its own, next to what is going on.
        press(&mut f.app, &[KeyCode::F(2)]);
        let during = buffer(&mut f.app, 120, 24);
        let (x, working) = find(&during, "Copying 1 item…", 24..86).expect("the work");
        assert_eq!(during[(x, working)].fg, theme::ACCENT);
        let wait = "Try again once the work under way is done.";
        let (_, notice) = find(&during, wait, 24..86).expect("the notice");
        assert_eq!((working, notice), (22, 23), "the two bottom rows");

        finish_work(&mut f.app);
        let after = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&after, "Copying", 0..120), None);
        assert_eq!(
            find(&after, "Copied 1 item.", 24..86).map(|(_, y)| y),
            Some(23)
        );
    }

    #[test]
    fn a_picture_that_arrives_under_a_dialog_waits_for_it_to_go() {
        use crate::app::testing::{pictures, settle};
        use crate::thumbs::testing::painted;
        let (mut f, _thumbnailer) = pictures();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        buffer(&mut f.app, 120, 24);
        press(&mut f.app, &[KeyCode::F(2)]);
        settle(&mut f.app);
        // The dialog ends at column 95; the preview goes on to 110.
        let covered = buffer(&mut f.app, 120, 24);
        assert!(find(&covered, "Rename", 0..120).is_some());
        assert!(!painted(&covered[(105, 5)]));
        press(&mut f.app, &[KeyCode::Esc]);
        let clear = buffer(&mut f.app, 120, 24);
        assert!(painted(&clear[(105, 5)]));
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

    #[test]
    fn the_help_overlay_lists_the_path_and_search_shortcuts() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let all = screen(&mut f.app, 120, 40).join("\n");
        for (keys, does) in [
            ("Ctrl+L", "Type a path (Tab completes)"),
            ("/", "Search this folder and below"),
        ] {
            let line = format!("{keys:<26}{does}");
            assert!(all.contains(&line), "{line} is missing from\n{all}");
        }
        assert!(
            all.contains("Close the window"),
            "the last one fits:\n{all}"
        );
    }

    #[test]
    fn a_search_that_found_nothing_says_so_in_every_view() {
        use crate::app::testing::{finish_search, type_text};
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "zebra");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Tab]);
        for view in ['2', '1', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let drawn = buffer(&mut f.app, 120, 24);
            let said = find(&drawn, "Nothing matches “zebra”", 24..86);
            assert!(said.is_some(), "view {view}");
            assert_eq!(find(&drawn, "No items", 0..120), None, "view {view}");
        }
        // An empty folder still says what it said.
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Enter]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert!(find(&drawn, "No items", 24..86).is_some());
    }

    #[test]
    fn what_a_search_found_shows_in_each_view_and_in_a_single_column() {
        use crate::app::testing::{finish_search, type_text};
        let mut f = fixture();
        let trip = f.home.join("Pictures").join("Trip");
        std::fs::write(trip.join("day.jpg"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Right, KeyCode::Char('/')],
        );
        type_text(&mut f.app, "jpg");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Tab]);

        // The home column steps out: what was found is the only one.
        let columns = buffer(&mut f.app, 130, 24);
        assert_eq!(find(&columns, "a.jpg", 24..96), Some((28, 3)));
        assert_eq!(find(&columns, "day.jpg", 24..96), Some((28, 4)));
        assert_eq!(find(&columns, "notes.txt", 24..96), None);
        assert_eq!(columns[(24, 3)].bg, theme::SELECTION, "the cursor");

        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let list = buffer(&mut f.app, 130, 24);
        assert_eq!(find(&list, "a.jpg", 24..96), Some((28, 3)));
        assert_eq!(find(&list, "day.jpg", 24..96), Some((28, 4)));
        assert!(find(&list, "3 B", 24..96).is_some(), "with its size");

        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let grid = buffer(&mut f.app, 130, 24);
        let (a, day) = (find(&grid, "a.jpg", 24..96), find(&grid, "day.jpg", 24..96));
        let ((ax, ay), (dx, dy)) = (a.expect("the first"), day.expect("the second"));
        assert!(ay == dy && ax < dx, "side by side, in the order found");

        // Left, the folder is back with the ones above it.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Esc]);
        let columns = buffer(&mut f.app, 130, 24);
        assert_eq!(find(&columns, "notes.txt", 24..96), Some((28, 5)));
        assert_eq!(find(&columns, "a.jpg", 24..96), Some((55, 4)));
    }

    #[test]
    fn two_panes_share_the_file_area_each_under_its_own_path() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down]);
        f.app.tick();
        let single = screen(&mut f.app, 120, 24);
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        f.app.tick();
        let right = buffer(&mut f.app, 120, 24);
        let colors = |drawn: &ratatui::buffer::Buffer, x, y| (drawn[(x, y)].fg, drawn[(x, y)].bg);
        let lit = (theme::ACCENT, theme::SELECTION);
        let unlit = (theme::BRIGHT_FOREGROUND, theme::LIGHTER_BACKGROUND);

        // The 62 cells between the sidebar and the details: 31, a hairline, 30.
        for y in 2..24 {
            assert_eq!(right[(55, y)].symbol(), "│", "row {y}");
            assert_eq!(right[(55, y)].fg, theme::RULE);
        }
        // Each pane's path on its first row: bright where the keyboard is.
        assert_eq!(find(&right, "~", 24..55), Some((26, 2)));
        assert_eq!(right[(26, 2)].fg, theme::DIM);
        assert_eq!(find(&right, "~ / Pictures", 56..86), Some((58, 2)));
        assert_eq!(right[(58, 2)].fg, theme::DIM);
        assert_eq!(right[(62, 2)].fg, theme::BRIGHT_FOREGROUND);
        // Under it the files, each pane's own, with its own cursor.
        assert_eq!(find(&right, "Documents", 24..55), Some((28, 4)));
        assert_eq!(find(&right, "Pictures", 24..55), Some((28, 5)));
        assert_eq!(find(&right, "notes.txt", 24..55), Some((28, 6)));
        assert_eq!(find(&right, "Trip", 56..86), Some((60, 4)));
        assert_eq!(find(&right, "a.jpg", 56..86), Some((60, 5)));
        assert_eq!(colors(&right, 28, 5), unlit);
        assert_eq!(colors(&right, 60, 5), lit);
        assert_eq!(right[(28, 4)].bg, theme::BACKGROUND);
        // The top bar, the sidebar and the details are the right pane's.
        assert_eq!(find(&right, "~ / Pictures", 0..60), Some((2, 0)));
        let sidebar = |drawn: &ratatui::buffer::Buffer, y: u16| -> String {
            (0..23).map(|x| drawn[(x, y)].symbol()).collect()
        };
        let place = (2..24).find(|y| sidebar(&right, *y).contains("Pictures"));
        let place = place.expect("the place");
        assert_eq!(right[(2, place)].bg, theme::SELECTION);
        assert_eq!(find(&right, "a.jpg", 86..120), Some((89, 10)));

        // With the keyboard in the left pane they are the left pane's.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.tick();
        let left = buffer(&mut f.app, 120, 24);
        assert_eq!(left[(26, 2)].fg, theme::BRIGHT_FOREGROUND);
        assert_eq!(left[(62, 2)].fg, theme::DIM);
        assert_eq!(colors(&left, 28, 5), lit);
        assert_eq!(colors(&left, 60, 5), unlit);
        assert!(sidebar(&left, place).contains("Pictures"));
        assert_eq!(left[(2, place)].bg, theme::BACKGROUND);
        let bar: String = (0..60).map(|x| left[(x, 0)].symbol()).collect();
        assert_eq!(bar.trim(), "~");
        assert_eq!(find(&left, "Pictures", 86..120), Some((89, 10)));
        assert_eq!(find(&left, "Folder · 2 items", 86..120), Some((89, 11)));

        // With the keyboard in the details neither pane's cursor is lit.
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Tab]);
        let away = buffer(&mut f.app, 120, 24);
        assert_eq!(colors(&away, 28, 5), unlit);
        assert_eq!(colors(&away, 60, 5), unlit);

        // Closed, the window is what it was before.
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        f.app.tick();
        assert_eq!(screen(&mut f.app, 120, 24), single);
    }

    #[test]
    fn every_selected_item_is_drawn_selected_in_each_view() {
        let mut f = fixture();
        f.app.handle_key(shift(KeyCode::Down));
        let colors = |drawn: &ratatui::buffer::Buffer, x, y| (drawn[(x, y)].fg, drawn[(x, y)].bg);
        let lit = (theme::ACCENT, theme::SELECTION);
        let unlit = (theme::BRIGHT_FOREGROUND, theme::LIGHTER_BACKGROUND);
        let plain = (theme::FOREGROUND, theme::BACKGROUND);

        // Documents and Pictures, not notes.txt: rows in the list and the columns.
        for view in ['2', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let drawn = buffer(&mut f.app, 120, 24);
            assert_eq!(colors(&drawn, 28, 3), lit, "view {view}");
            assert_eq!(colors(&drawn, 28, 4), lit, "view {view}");
            assert_eq!(colors(&drawn, 28, 5), plain, "view {view}");
        }
        // Tiles in the grid: a frame around each, and the name in its color.
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let grid = buffer(&mut f.app, 120, 24);
        for (corner, name) in [(26, 31), (46, 51)] {
            assert_eq!(grid[(corner, 2)].symbol(), "┌");
            assert_eq!(grid[(corner, 2)].fg, theme::ACCENT);
            assert_eq!(grid[(name, 10)].fg, theme::ACCENT);
        }
        assert_eq!(grid[(66, 2)].symbol(), " ");
        assert_eq!(grid[(71, 10)].fg, theme::FOREGROUND);

        // With the keyboard elsewhere all of them go grey.
        press(&mut f.app, &[KeyCode::Tab]);
        let grid = buffer(&mut f.app, 120, 24);
        for (corner, name) in [(26, 31), (46, 51)] {
            assert_eq!(grid[(corner, 2)].symbol(), "┌");
            assert_eq!(grid[(corner, 2)].fg, theme::BRIGHT_FOREGROUND);
            assert_eq!(grid[(name, 10)].fg, theme::BRIGHT_FOREGROUND);
        }
        for view in ['2', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let drawn = buffer(&mut f.app, 120, 24);
            assert_eq!(colors(&drawn, 28, 3), unlit, "view {view}");
            assert_eq!(colors(&drawn, 28, 4), unlit, "view {view}");
            assert_eq!(colors(&drawn, 28, 5), plain, "view {view}");
        }
    }

    #[test]
    fn the_help_overlay_lists_the_split_and_selection_shortcuts() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let all = screen(&mut f.app, 120, 40).join("\n");
        for (keys, does) in [
            (
                "Ctrl+Alt+N / Ctrl+W",
                "Split into two panes / close the right one",
            ),
            (
                "Tab / Shift+Tab",
                "Move between the sidebar, the panes and the details",
            ),
            ("Shift+arrows", "Select a range"),
            ("Ctrl+A", "Select everything"),
        ] {
            let line = format!("{keys:<26}{does}");
            assert!(all.contains(&line), "{line} is missing from\n{all}");
        }
        assert!(
            all.contains("Close the window"),
            "the last one fits:\n{all}"
        );
    }
}
