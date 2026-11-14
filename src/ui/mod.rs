//! Drawing: flea's frame of a header, a rule, the body, a rule and the
//! footer, with one function per region.

mod columns;
mod dialog;
mod footer;
mod grid;
mod help;
mod list;
mod menu;
mod preview;
mod topbar;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui_image::Image;
use ratatui_image::protocol::Protocol;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, HitMap, Pane, Spot, ViewMode};
use crate::model::Entry;
use crate::theme;
use crate::thumbs::Thumbs;

/// The rows the frame keeps for itself: the header, two rules and the footer.
const CHROME_ROWS: u16 = 4;
/// Below this many columns or rows the window says only how big it has to be.
const MIN_COLUMNS: u16 = 12;
const MIN_ROWS: u16 = 5;
pub const TOO_SMALL: &str = "Make the window at least 12 columns wide and 5 rows tall.";

/// Draws the window, and keeps where it drew what a click can land on.
pub fn draw(frame: &mut Frame, app: &mut App) {
    theme::set_depth(app.depth);
    let mut hits = HitMap::default();
    draw_window(frame, app, &mut hits);
    app.hits = hits;
}

/// The widths of the parent's column, the list and the preview, in that
/// order, for a window `columns` wide; the preview's is 0 when it is hidden.
/// A `│` follows each of the first two.
pub fn pane_widths(columns: u16, preview: bool) -> (u16, u16, u16) {
    let columns = columns as usize;
    let left = (columns * 22 / 100).saturating_sub(1);
    let middle = if preview {
        (columns * 40 / 100).saturating_sub(1)
    } else {
        columns.saturating_sub(left + 1)
    };
    let right = if preview {
        columns.saturating_sub(left + middle + 2)
    } else {
        0
    };
    (left as u16, middle as u16, right as u16)
}

fn draw_window(frame: &mut Frame, app: &mut App, hits: &mut HitMap) {
    let area = frame.area();
    if let Some(thumbs) = &mut app.thumbs {
        thumbs.cover(app.help_visible || app.dialog.is_some() || app.menu.is_some());
    }
    if area.width < MIN_COLUMNS || area.height < MIN_ROWS {
        too_small(frame, area);
        return;
    }
    let [header, top_rule, body, bottom_rule, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(area.height - CHROME_ROWS),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    topbar::draw(frame, app, header, hits);

    // The list view of a single pane: the parent's column, the list and the
    // preview. The other views and two panes take the whole body.
    let miller = app.view == ViewMode::List && app.panes.len() == 1;
    let preview = miller && app.preview_visible;
    let (left, middle, right) = pane_widths(area.width, preview);
    let junctions: Vec<u16> = match (miller, preview) {
        (true, true) => vec![left, left + middle + 1],
        (true, false) => vec![left],
        _ => Vec::new(),
    };
    rule(frame, top_rule, &junctions, '┬');
    rule(frame, bottom_rule, &junctions, '┴');

    let item = if miller {
        let column = |x: u16, width: u16| Rect::new(body.x + x, body.y, width, body.height);
        list::draw_parent(frame, app, column(0, left), hits);
        separator(frame, column(left, 1));
        let item = draw_pane(frame, app, app.active, column(left + 1, middle), hits);
        if preview {
            separator(frame, column(left + middle + 1, 1));
            preview::draw(frame, app, column(left + middle + 2, right), hits);
        }
        item
    } else {
        draw_files(frame, app, body, hits)
    };
    footer::draw(frame, app, footer);

    if let Some(menu) = &app.menu {
        // It opens where it was right-clicked open, or else next to the
        // item under the cursor.
        let anchor = match menu.at {
            Some(at) => Rect::new(at.x, at.y, 0, 1),
            None => item,
        };
        menu::draw(frame, menu, anchor, area, hits);
    }
    if app.help_visible {
        help::draw(frame, app, area, hits);
    }
    if let Some(dialog) = &app.dialog {
        dialog::draw(frame, dialog, area, hits);
    }
}

/// The only thing a window too small for anything else shows, word-wrapped
/// and in the middle from top to bottom.
fn too_small(frame: &mut Frame, area: Rect) {
    let lines = words(TOO_SMALL, (area.width as usize).saturating_sub(1).max(1));
    let top = area.height.saturating_sub(lines.len() as u16) / 2;
    let one = lines.len() == 1;
    for (y, line) in (area.y + top..area.bottom()).zip(lines) {
        let row = Rect::new(area.x, y, area.width, 1);
        let (text, alignment) = if one {
            (line, Alignment::Center)
        } else {
            (format!(" {line}"), Alignment::Left)
        };
        let text = match alignment {
            Alignment::Center => centered(&text, area.width as usize),
            _ => fit(&text, area.width as usize),
        };
        frame.render_widget(Paragraph::new(text).style(theme::base()), row);
    }
}

/// Text broken between words into lines of at most `width` cells; a word
/// longer than that is broken where it has to be.
pub fn words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        for c in word.chars() {
            if !line.is_empty() && line.width() + c.width().unwrap_or(0) > width {
                lines.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A rule across the window, with a junction where a pane's line meets it.
fn rule(frame: &mut Frame, area: Rect, junctions: &[u16], junction: char) {
    let text: String = (0..area.width)
        .map(|x| {
            if junctions.contains(&x) {
                junction
            } else {
                '─'
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(text).style(theme::border()), area);
}

/// The line between two panes, as tall as the body.
fn separator(frame: &mut Frame, area: Rect) {
    let lines: Vec<Line> = (0..area.height).map(|_| Line::from("│")).collect();
    frame.render_widget(Paragraph::new(lines).style(theme::border()), area);
}

/// A column of the columns view, and that with the hairline on its right.
const COLUMN_WIDTH: u16 = 26;
const COLUMN_PITCH: u16 = COLUMN_WIDTH + 1;
/// The cells in front of a name in a row: the edge, the glyph and a space.
const NAME_INDENT: u16 = 3;

/// The cells of the item under the cursor of a pane whose view was just
/// drawn in `area`: its row from the name on, or its tile's picture. With
/// nothing under the cursor, the top of the pane.
fn item_cell(app: &App, hits: &HitMap, pane: usize, area: Rect) -> Rect {
    let index = pane;
    let pane = &app.panes[pane];
    let top = Rect::new(area.x + NAME_INDENT, area.y + 1, 0, 0);
    if pane.entries.is_empty() {
        return top;
    }
    match app.view {
        ViewMode::Grid => grid::thumb_rect(area, app.tile, pane.offset, pane.cursor).unwrap_or(top),
        // The row the view drew it on.
        ViewMode::List | ViewMode::Columns => {
            let item = Spot::Item {
                pane: index,
                index: pane.cursor,
            };
            let row = hits.rect_of(&item);
            row.map_or(top, |row| Rect::new(row.x + NAME_INDENT, row.y, 0, 1))
        }
    }
}

/// Draws the panes in the body when the view takes all of it. Returns the
/// cells of the item under the active pane's cursor.
fn draw_files(frame: &mut Frame, app: &mut App, view: Rect, hits: &mut HitMap) -> Rect {
    if app.panes.len() > 1 {
        // Two panes side by side with a line between them, each under its
        // own path: the header only has room for the active one's.
        let left = view.width.saturating_sub(1).div_ceil(2);
        let [left, rule, right] = Layout::horizontal([
            Constraint::Length(left),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(view);
        let hairline = Block::new()
            .borders(Borders::LEFT)
            .border_style(theme::border());
        frame.render_widget(hairline, rule);
        let mut cells = [Rect::default(); 2];
        for (pane, area) in [left, right].into_iter().enumerate() {
            let [path, files] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
            hits.add(area, Spot::Pane(pane));
            topbar::draw_pane_path(frame, app, pane, path, hits);
            cells[pane] = draw_pane(frame, app, pane, files, hits);
        }
        cells[app.active.min(1)]
    } else {
        draw_pane(frame, app, app.active, view, hits)
    }
}

/// One pane's items in the view that is on. Returns the cells of the item
/// under its cursor.
fn draw_pane(frame: &mut Frame, app: &mut App, pane: usize, area: Rect, hits: &mut HitMap) -> Rect {
    let focused = app.focus == Focus::List && app.active == pane;
    // The columns show one blank row of air on top; the list and the grid
    // count their own rows.
    app.panes[pane].page_rows = area.height.saturating_sub(1).max(1) as usize;
    hits.add(area, Spot::Pane(pane));
    match app.view {
        ViewMode::Grid => grid::draw(frame, app, pane, area, focused, hits),
        ViewMode::List => list::draw(frame, app, pane, area, focused, hits),
        ViewMode::Columns => columns::draw(frame, app, pane, area, focused, hits),
    }
    item_cell(app, hits, pane, area)
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

/// flea's captions for an empty folder, which take turns.
pub const CAPTIONS: [&str; crate::app::CAPTIONS] =
    ["NOTHING HERE YET", "A VERY TIDY FOLDER", "QUIET IN HERE"];

/// What a view says in place of the items of a pane that has none.
pub fn empty_text(pane: &Pane, caption: usize) -> String {
    match pane.results() {
        // Still looking: it may find some yet.
        Some(search) if search.is_running() => String::new(),
        Some(search) => format!("NOTHING MATCHES {}", search.input.text()),
        None => CAPTIONS[caption % CAPTIONS.len()].to_string(),
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

/// Text in the middle of `width` cells, padded on both sides.
pub fn centered(text: &str, width: usize) -> String {
    let text = fit(text, width);
    let before = width.saturating_sub(text.width()) / 2;
    pad(&format!("{}{text}", " ".repeat(before)), width)
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
    use super::testing::{buffer, find, screen};
    use super::*;
    use crate::app::testing::{
        ctrl, finish_search, finish_work, fixture, manual_clock, press, shift, split, type_text,
    };
    use crate::icons;
    use crate::theme::Depth;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{
        KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::style::{Color, Modifier};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// The cells of one row from one column to another, as text.
    fn cells(drawn: &Buffer, y: u16, columns: std::ops::Range<u16>) -> String {
        columns.map(|x| drawn[(x, y)].symbol()).collect()
    }

    /// The fixture, in Pictures: Trip and a.jpg in the list, the home
    /// folder's Documents, Pictures and notes.txt left of it.
    fn in_pictures() -> crate::app::testing::Fixture {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        f.app.tick();
        f
    }

    #[test]
    fn fit_cuts_with_an_ellipsis_and_pad_fills_the_width() {
        assert_eq!(fit("harbour-dusk.jpg", 20), "harbour-dusk.jpg");
        assert_eq!(fit("harbour-dusk.jpg", 8), "harbour…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad_left("ab", 4), "  ab");
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(centered("ab", 6), "  ab  ");
    }

    #[test]
    fn panes_take_flea_s_widths_of_the_window() {
        assert_eq!(pane_widths(120, true), (25, 47, 46));
        assert_eq!(pane_widths(80, true), (16, 31, 31));
        assert_eq!(pane_widths(120, false), (25, 94, 0));
        assert_eq!(pane_widths(80, false), (16, 63, 0));
    }

    #[test]
    fn the_frame_is_a_header_a_rule_the_body_a_rule_and_the_footer() {
        let mut f = in_pictures();
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "~/Pictures", 0..120), Some((1, 0)));
        for (y, junction) in [(1, "┬"), (28, "┴")] {
            for x in 0..120 {
                let wanted = if x == 25 || x == 73 { junction } else { "─" };
                assert_eq!(drawn[(x, y)].symbol(), wanted, "{x}, {y}");
                assert_eq!(drawn[(x, y)].fg, theme::BORDER);
            }
        }
        // A line between the panes all the way down the body.
        for y in 2..28 {
            for x in [25, 73] {
                assert_eq!(drawn[(x, y)].symbol(), "│", "{x}, {y}");
                assert_eq!(drawn[(x, y)].fg, theme::BORDER);
            }
        }
        assert_eq!(find(&drawn, "2 items", 0..120), Some((1, 29)));
        // The terminal's own background everywhere outside the fills.
        assert_eq!(drawn[(50, 20)].bg, Color::Reset);

        // Without the preview the list takes its room, and one junction stays.
        f.app
            .handle_key(crate::app::testing::alt(KeyCode::Char('p')));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(25, 1)].symbol(), "┬");
        assert_eq!(drawn[(73, 1)].symbol(), "─");
        assert_eq!(drawn[(73, 10)].symbol(), " ");
        assert_eq!(find(&drawn, "3 B", 0..120), Some((115, 3)));
    }

    #[test]
    fn a_window_too_small_says_only_how_big_it_has_to_be() {
        let mut f = fixture();
        let lines = screen(&mut f.app, 11, 10);
        let wrapped = [
            " Make the",
            " window at",
            " least 12",
            " columns",
            " wide and 5",
            " rows tall.",
        ];
        for (y, line) in lines.iter().enumerate() {
            match y.checked_sub(2).and_then(|at| wrapped.get(at)) {
                Some(text) => assert_eq!(line.trim_end(), *text, "row {y}"),
                None => assert_eq!(line.trim(), "", "row {y}"),
            }
        }
        // On one line it is in the middle both ways.
        let lines = screen(&mut f.app, 120, 4);
        assert_eq!(lines[1].trim(), TOO_SMALL);
        assert_eq!(lines[1].find("Make"), Some(31));
        assert!(lines[0].trim().is_empty() && lines[3].trim().is_empty());
        // At 12 by 5 the window is drawn.
        assert!(!screen(&mut f.app, 12, 5).join("").contains("Make"));
    }

    #[test]
    fn the_parent_column_lists_the_parent_folder_muted_with_this_folder_lifted() {
        let mut f = in_pictures();
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "Documents", 0..25), Some((3, 2)));
        assert_eq!(drawn[(3, 2)].fg, theme::MUTED);
        assert_eq!(drawn[(1, 2)].fg, theme::MUTED, "the glyph");
        // This folder: on the selected fill from edge to edge, no accent edge.
        assert_eq!(cells(&drawn, 3, 3..11), "Pictures");
        assert_eq!(drawn[(3, 3)].fg, theme::FOREGROUND);
        for x in 0..25 {
            assert_eq!(drawn[(x, 3)].bg, theme::SELECTED, "{x}");
        }
        assert_eq!(drawn[(0, 3)].symbol(), " ");
        assert_eq!(find(&drawn, "notes.txt", 0..25), Some((3, 4)));
        assert_eq!(drawn[(3, 4)].fg, theme::MUTED);
        // No sizes there.
        for y in 2..28 {
            let row = cells(&drawn, y, 0..25);
            assert!(
                !row.contains("5 B") && !row.contains("item"),
                "row {y}: {row}"
            );
        }

        // A click on a row there goes into that folder.
        f.app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(f.app.pane().cwd, f.home.join("Documents"));

        // At the root it is empty.
        f.app.navigate(std::path::Path::new("/"));
        let root = buffer(&mut f.app, 120, 30);
        for y in 2..28 {
            assert_eq!(cells(&root, y, 0..25).trim(), "", "row {y}");
        }
    }

    #[test]
    fn list_rows_are_an_edge_a_glyph_the_name_and_the_size() {
        let mut f = in_pictures();
        let drawn = buffer(&mut f.app, 120, 30);
        // Trip, under the cursor in the list that has the keyboard.
        assert_eq!(drawn[(26, 2)].symbol(), "▎");
        assert_eq!(drawn[(26, 2)].fg, theme::ACCENT);
        assert_eq!(drawn[(27, 2)].symbol(), icons::FOLDER);
        assert_eq!(drawn[(27, 2)].fg, theme::FOREGROUND, "lifted");
        assert_eq!(find(&drawn, "Trip", 26..73), Some((29, 2)));
        assert_eq!(find(&drawn, "No items", 26..73), Some((63, 2)));
        for x in 26..73 {
            assert_eq!(drawn[(x, 2)].bg, theme::CURSOR, "{x}");
        }
        // a.jpg: no fill, the glyph and the size in the metadata ink.
        assert_eq!(drawn[(26, 3)].symbol(), " ");
        assert_eq!(drawn[(27, 3)].fg, theme::MUTED);
        assert_eq!(find(&drawn, "a.jpg", 26..73), Some((29, 3)));
        assert_eq!(drawn[(29, 3)].fg, theme::FOREGROUND);
        assert_eq!(find(&drawn, "3 B", 26..73), Some((68, 3)));
        assert_eq!(drawn[(68, 3)].fg, theme::MUTED);
        assert_eq!(drawn[(40, 3)].bg, Color::Reset);

        // With the keyboard in the preview the cursor row loses its edge.
        press(&mut f.app, &[KeyCode::Tab]);
        let away = buffer(&mut f.app, 120, 30);
        assert_eq!(away[(26, 2)].symbol(), " ");
        assert_eq!(away[(26, 2)].bg, theme::SELECTED);

        // A long name is cut with an ellipsis before the size.
        fs::write(
            f.home
                .join("Pictures")
                .join(format!("{}.txt", "x".repeat(60))),
            "",
        )
        .unwrap();
        press(&mut f.app, &[KeyCode::Esc, KeyCode::F(5)]);
        let cut = buffer(&mut f.app, 120, 30);
        let row = cells(&cut, 4, 26..73);
        assert_eq!(row, format!(" {} {}… 0 B  ", icons::TEXT, "x".repeat(37)));
    }

    #[test]
    fn row_colours_go_by_what_the_item_is_first_match_winning() {
        let mut f = in_pictures();
        let pictures = f.home.join("Pictures");
        std::os::unix::fs::symlink("a.jpg", pictures.join("link")).unwrap();
        fs::write(pictures.join("run.sh"), "#!/bin/sh").unwrap();
        fs::set_permissions(pictures.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(pictures.join("secret.txt"), "").unwrap();
        fs::set_permissions(
            pictures.join("secret.txt"),
            fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        // Trip, a.jpg, link, run.sh, secret.txt.
        let drawn = buffer(&mut f.app, 120, 30);
        let ink = |name: &str| {
            let (x, y) = find(&drawn, name, 26..73).unwrap();
            drawn[(x, y)].fg
        };
        assert_eq!(ink("a.jpg"), theme::FOREGROUND);
        assert_eq!(ink("link"), theme::SYMLINK);
        assert_eq!(ink("→ a.jpg"), theme::MUTED);
        assert_eq!(ink("run.sh"), theme::EXECUTABLE);
        assert_eq!(ink("secret.txt"), theme::MUTED);
        // Under the cursor or marked, everything is in the foreground.
        press(&mut f.app, &[KeyCode::Down, KeyCode::Down, KeyCode::Down]);
        f.app.handle_key(shift(KeyCode::Up));
        let marked = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&marked, "run.sh", 26..73).unwrap();
        assert_eq!(
            (marked[(x, y)].fg, marked[(x, y)].bg),
            (theme::FOREGROUND, theme::MARKED)
        );
        let (x, y) = find(&marked, "link", 26..73).unwrap();
        assert_eq!(
            (marked[(x, y)].fg, marked[(x, y)].bg),
            (theme::FOREGROUND, theme::CURSOR)
        );
        assert_eq!(marked[(x + 5, y)].fg, theme::FOREGROUND, "→ lifted too");
        fs::set_permissions(
            pictures.join("secret.txt"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
    }

    #[test]
    fn without_colour_the_cursor_row_is_reverse_video_and_marked_rows_are_underlined() {
        let mut f = in_pictures();
        fs::write(f.home.join("Pictures").join("b.jpg"), "b").unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        f.app.handle_key(shift(KeyCode::Down));
        f.app.depth = Depth::Mono;
        let drawn = buffer(&mut f.app, 120, 30);
        // a.jpg is the cursor's, Trip marked with it; b.jpg neither.
        let at = |name: &str| find(&drawn, name, 26..73).unwrap();
        let (x, y) = at("a.jpg");
        assert!(drawn[(x, y)].modifier.contains(Modifier::REVERSED));
        assert_eq!(drawn[(26, y)].symbol(), " ", "no edge");
        assert_eq!(
            (drawn[(x, y)].fg, drawn[(x, y)].bg),
            (Color::Reset, Color::Reset)
        );
        let (x, y) = at("Trip");
        assert!(drawn[(x, y)].modifier.contains(Modifier::UNDERLINED));
        assert!(!drawn[(x, y)].modifier.contains(Modifier::REVERSED));
        let (x, y) = at("b.jpg");
        assert!(drawn[(x, y)].modifier.is_empty());
        assert_eq!(drawn[(25, 10)].fg, Color::Reset, "no colour anywhere");

        // Sixteen colours: reverse video, and the marked fill.
        f.app.depth = Depth::Sixteen;
        let drawn = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&drawn, "a.jpg", 26..73).unwrap();
        assert!(drawn[(x, y)].modifier.contains(Modifier::REVERSED));
        let (x, y) = find(&drawn, "Trip", 26..73).unwrap();
        assert_eq!(drawn[(x, y)].bg, Color::DarkGray);
    }

    #[test]
    fn in_256_colours_the_palette_is_the_nearest_index() {
        let mut f = in_pictures();
        f.app.depth = Depth::Indexed;
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(29, 3)].fg, Color::Indexed(252), "a.jpg");
        assert_eq!(drawn[(68, 3)].fg, Color::Indexed(246), "its size");
        assert_eq!(drawn[(29, 2)].bg, Color::Indexed(236), "the cursor's fill");
        assert_eq!(
            drawn[(25, 10)].fg,
            Color::Indexed(235),
            "the line between panes"
        );
        assert_eq!(drawn[(26, 2)].symbol(), "▎");
    }

    #[test]
    fn what_a_search_matches_is_bold_and_where_it_was_found_follows_the_name() {
        let mut f = in_pictures();
        fs::write(f.home.join("Pictures").join("Trip").join("day.jpg"), "").unwrap();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "JP");
        finish_search(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&drawn, "a.jpg", 26..73).unwrap();
        let bold = |x: u16| drawn[(x, y)].modifier.contains(Modifier::BOLD);
        assert_eq!(
            (bold(x), bold(x + 1), bold(x + 2), bold(x + 3), bold(x + 4)),
            (false, false, true, true, false)
        );
        let (x, y) = find(&drawn, "day.jpg", 26..73).unwrap();
        assert_eq!(find(&drawn, "day.jpg Trip", 26..73), Some((x, y)));
        assert_eq!(drawn[(x + 8, y)].fg, theme::MUTED);
    }

    #[test]
    fn the_footer_counts_says_the_volume_and_where_the_keys_are() {
        let mut f = in_pictures();
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(cells(&drawn, 29, 0..9), " 2 items ");
        let right = "apfs · 1.2 GB free · ? keys";
        let x = 119 - right.chars().count() as u16;
        assert_eq!(find(&drawn, right, 0..120), Some((x, 29)));
        assert_eq!(drawn[(x, 29)].fg, theme::FOREGROUND);
        assert_eq!(drawn[(x + 19, 29)].fg, theme::MUTED, "the dot");
        assert_eq!(drawn[(x + 21, 29)].fg, theme::MUTED, "? keys");
        assert_eq!(drawn[(119, 29)].symbol(), " ");

        // Several selected: how many of how many, and their size.
        fs::write(f.home.join("Pictures").join("b.jpg"), "four").unwrap();
        press(&mut f.app, &[KeyCode::F(5), KeyCode::Down]);
        f.app.handle_key(shift(KeyCode::Down));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "2 of 3 selected · 7 B", 0..120), Some((1, 29)));

        // Narrow, the runs on the right step aside each when it doesn't fit.
        press(&mut f.app, &[KeyCode::Esc]);
        f.app.tick();
        let at = |app: &mut crate::app::App, width: u16| screen(app, width, 10)[9].clone();
        assert_eq!(
            at(&mut f.app, 40),
            " 3 items    apfs · 1.2 GB free · ? keys "
        );
        assert_eq!(at(&mut f.app, 30), " 3 items   apfs · 1.2 GB free ");
        assert_eq!(at(&mut f.app, 20), " 3 items     ? keys ");
    }

    #[test]
    fn the_footer_says_what_happened_and_that_undo_can_take_it_back() {
        let mut f = fixture();
        let clock = manual_clock(&mut f.app);
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(ctrl(KeyCode::Char('c')));
        press(&mut f.app, &[KeyCode::Home, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        // Under way, the work is said after the count.
        let during = buffer(&mut f.app, 120, 30);
        assert_eq!(
            find(&during, "No items  Copying 1 item…", 0..120),
            Some((1, 29))
        );

        finish_work(&mut f.app);
        let after = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&after, "Copying", 0..120), None);
        assert_eq!(
            find(&after, "1 item  Copied 1 item · z undoes", 0..120),
            Some((1, 29))
        );
        let (x, _) = find(&after, "Copied", 0..120).unwrap();
        assert_eq!(after[(x, 29)].fg, theme::FOREGROUND);
        assert_eq!(after[(x + 14, 29)].fg, theme::MUTED, "the hint");

        // An error in the error colour, gone after eight seconds.
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        finish_work(&mut f.app);
        f.app.navigate(&f.home.join("missing"));
        let failed = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&failed, "missing doesn't exist.", 0..120).unwrap();
        assert_eq!((failed[(x, y)].fg, y), (theme::ERROR, 29));
        clock.set(clock.get() + Duration::from_secs(7));
        f.app.tick();
        assert!(f.app.message.is_some());
        clock.set(clock.get() + Duration::from_secs(1));
        f.app.tick();
        assert!(f.app.message.is_none());
    }

    #[test]
    fn an_empty_folder_takes_turns_with_its_captions_in_the_middle() {
        let mut f = fixture();
        let clock = manual_clock(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.tick();
        let caption = |app: &mut crate::app::App| {
            let drawn = buffer(app, 120, 30);
            // The list's 47 cells and 26 rows: the glyph on row 11 of them,
            // the caption under it.
            let glyph = cells(&drawn, 13, 26..73);
            assert_eq!(glyph.trim(), icons::FOLDER);
            assert_eq!(drawn[(49, 13)].fg, theme::MUTED);
            cells(&drawn, 14, 26..73).trim().to_string()
        };
        assert_eq!(caption(&mut f.app), "NOTHING HERE YET");
        for (millis, said) in [
            (2799, "NOTHING HERE YET"),
            (1, "A VERY TIDY FOLDER"),
            (2800, "QUIET IN HERE"),
            (2800, "NOTHING HERE YET"),
        ] {
            clock.set(clock.get() + Duration::from_millis(millis));
            f.app.tick();
            assert_eq!(caption(&mut f.app), said);
        }
        // Back in it later, it starts again at the first.
        press(&mut f.app, &[KeyCode::Backspace]);
        f.app.tick();
        clock.set(clock.get() + Duration::from_secs(4));
        press(&mut f.app, &[KeyCode::Enter]);
        f.app.tick();
        assert_eq!(caption(&mut f.app), "NOTHING HERE YET");
    }

    #[test]
    fn a_search_that_found_nothing_says_so_in_every_view() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "zebra");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Tab]);
        for view in ['2', '1', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let drawn = buffer(&mut f.app, 120, 30);
            let said = find(&drawn, "NOTHING MATCHES zebra", 0..120);
            assert!(said.is_some(), "view {view}");
            assert_eq!(
                find(&drawn, "NOTHING HERE YET", 0..120),
                None,
                "view {view}"
            );
        }
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let drawn = buffer(&mut f.app, 120, 30);
        let (_, y) = find(&drawn, "NOTHING MATCHES zebra", 26..73).unwrap();
        assert_eq!(cells(&drawn, y - 1, 26..73).trim(), icons::SEARCH);
        // An empty folder still says what it says.
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Enter]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "NOTHING HERE YET", 26..73).is_some());
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
            ("Alt+P", "Show or hide the preview pane"),
            ("Ctrl+H", "Show hidden files"),
            ("Space", "Preview the selected file"),
            ("Alt+⏎", "Everything else, for what is selected"),
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
            ("Ctrl+L", "Type a path (Tab completes)"),
            ("/", "Search this folder and below"),
            (
                "| or Ctrl+Alt+N / Ctrl+W",
                "Split into two panes / close the right one (or Alt+1)",
            ),
            (
                "Tab / Shift+Tab",
                "Move between the list and the preview, or the panes",
            ),
            ("Shift+arrows", "Select a range"),
            ("Ctrl+A", "Select everything"),
        ] {
            let line = format!("{keys:<26}{does}");
            assert!(all.contains(&line), "{line} is missing from\n{all}");
        }
        for gone in ["Ctrl+P", "Ctrl+I", "sidebar", "details pane"] {
            assert!(!all.contains(gone), "{gone}");
        }
        assert!(
            all.contains("Close the window"),
            "the last one fits:\n{all}"
        );
        // Framed in the card's rule, on the terminal's own background.
        let drawn = buffer(&mut f.app, 120, 40);
        let (x, y) = find(&drawn, "┌", 0..120).unwrap();
        assert_eq!(drawn[(x, y)].fg, theme::CARD_RULE);
        let (x, y) = find(&drawn, "Keyboard shortcuts", 0..120).unwrap();
        assert!(drawn[(x, y)].modifier.contains(Modifier::BOLD));
        assert_eq!(drawn[(x, y)].bg, Color::Reset);
    }

    #[test]
    fn a_picture_that_arrives_under_a_dialog_waits_for_it_to_go() {
        use crate::app::testing::{pictures, settle};
        use crate::thumbs::testing::painted;
        let (mut f, _thumbnailer) = pictures();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        press(&mut f.app, &[KeyCode::F(2)]);
        settle(&mut f.app);
        // The dialog ends at column 95; the preview's picture is under its
        // head, from row 5, and goes on past the dialog.
        let preview = |drawn: &Buffer| {
            let cells = (96..120).flat_map(|x| (5..28).map(move |y| (x, y)));
            cells.filter(|at| painted(&drawn[*at])).count()
        };
        let covered = buffer(&mut f.app, 120, 30);
        assert!(find(&covered, "Rename", 0..120).is_some());
        assert_eq!(preview(&covered), 0);
        press(&mut f.app, &[KeyCode::Esc]);
        let clear = buffer(&mut f.app, 120, 30);
        assert!(preview(&clear) > 0);
        let head = (96..120).filter(|x| painted(&clear[(*x, 2)])).count();
        assert_eq!(head, 0, "never over the name");
    }

    #[test]
    fn ctrl_1_2_3_draw_the_grid_the_list_and_the_columns() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let grid = buffer(&mut f.app, 120, 30);
        // Five tiles across the whole body, the names under the pictures.
        assert_eq!(find(&grid, "notes.txt", 0..120), Some((56, 10)));
        assert_eq!(grid[(25, 10)].symbol(), " ", "no parent column");

        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let list = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&list, "notes.txt", 26..73), Some((29, 4)));
        assert_eq!(find(&list, "5 B", 26..73).map(|(_, y)| y), Some(4));
        assert_eq!(list[(25, 10)].symbol(), "│");

        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let columns = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&columns, "notes.txt", 0..120), Some((3, 5)));
        assert_eq!(find(&columns, "5 B", 0..120), None);
        assert_eq!(columns[(26, 3)].symbol(), "│");
    }

    #[test]
    fn what_a_search_found_shows_in_each_view_and_in_a_single_column() {
        let mut f = fixture();
        let trip = f.home.join("Pictures").join("Trip");
        fs::write(trip.join("day.jpg"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Right, KeyCode::Char('/')],
        );
        type_text(&mut f.app, "jpg");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Tab]);

        // The home column steps out: what was found is the only one.
        let columns = buffer(&mut f.app, 130, 30);
        assert_eq!(find(&columns, "a.jpg", 0..130), Some((3, 3)));
        assert_eq!(find(&columns, "day.jpg", 0..130), Some((3, 4)));
        assert_eq!(find(&columns, "notes.txt", 0..130), None);
        assert_eq!(columns[(0, 3)].bg, theme::CURSOR, "the cursor");

        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let list = buffer(&mut f.app, 130, 30);
        let (x, y) = find(&list, "a.jpg", 0..130).unwrap();
        assert_eq!(find(&list, "day.jpg", 0..130), Some((x, y + 1)));
        assert!(find(&list, "3 B", 0..130).is_some(), "with its size");

        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let grid = buffer(&mut f.app, 130, 30);
        let (a, day) = (find(&grid, "a.jpg", 0..130), find(&grid, "day.jpg", 0..130));
        let ((ax, ay), (dx, dy)) = (a.expect("the first"), day.expect("the second"));
        assert!(ay == dy && ax < dx, "side by side, in the order found");

        // Left, the folder is back with the ones above it.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Esc]);
        let columns = buffer(&mut f.app, 130, 30);
        assert_eq!(find(&columns, "notes.txt", 0..130), Some((3, 5)));
        assert_eq!(find(&columns, "a.jpg", 0..130), Some((30, 4)));
    }

    #[test]
    fn two_panes_share_the_body_each_under_its_own_path() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down]);
        f.app.tick();
        let single = screen(&mut f.app, 120, 30);
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        f.app.tick();
        let right = buffer(&mut f.app, 120, 30);

        // The whole body: 60, a line, 59.
        for y in 2..28 {
            assert_eq!(right[(60, y)].symbol(), "│", "row {y}");
            assert_eq!(right[(60, y)].fg, theme::BORDER);
        }
        // Each pane's path on its first row: the folder lit where the keyboard is.
        assert_eq!(cells(&right, 2, 0..3), " ~ ");
        assert_eq!(right[(1, 2)].fg, theme::MUTED);
        assert_eq!(cells(&right, 2, 61..73), " ~/Pictures ");
        assert_eq!(right[(62, 2)].fg, theme::MUTED);
        assert_eq!(right[(64, 2)].fg, theme::FOREGROUND);
        // Under it the files, each pane's own, with its own cursor: only the
        // pane with the keyboard draws the edge.
        assert_eq!(cells(&right, 3, 3..12), "Documents");
        assert_eq!(cells(&right, 4, 3..11), "Pictures");
        assert_eq!(cells(&right, 3, 64..68), "Trip");
        assert_eq!(cells(&right, 4, 64..69), "a.jpg");
        assert_eq!(
            (right[(0, 4)].symbol(), right[(0, 4)].bg),
            (" ", theme::SELECTED)
        );
        assert_eq!(
            (right[(61, 4)].symbol(), right[(61, 4)].bg),
            ("▎", theme::CURSOR)
        );
        // The header is the right pane's.
        assert_eq!(find(&right, "~/Pictures", 0..60), Some((1, 0)));

        // With the keyboard in the left pane they are the left pane's.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.tick();
        let left = buffer(&mut f.app, 120, 30);
        assert_eq!(left[(1, 2)].fg, theme::FOREGROUND);
        assert_eq!(left[(64, 2)].fg, theme::MUTED);
        assert_eq!(left[(0, 4)].symbol(), "▎");
        assert_eq!(left[(61, 4)].symbol(), " ");
        assert_eq!(cells(&left, 0, 0..60).trim(), "~");

        // Closed, the window is what it was before.
        f.app.handle_key(ctrl(KeyCode::Char('w')));
        f.app.tick();
        assert_eq!(screen(&mut f.app, 120, 30), single);
    }

    #[test]
    fn every_selected_item_is_drawn_selected_in_each_view() {
        let mut f = fixture();
        f.app.handle_key(shift(KeyCode::Down));
        let colors = |drawn: &Buffer, x, y| (drawn[(x, y)].fg, drawn[(x, y)].bg);

        // Documents and Pictures, not notes.txt: rows in the list and the columns.
        for (view, x, top) in [('2', 29, 2), ('3', 3, 3)] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let drawn = buffer(&mut f.app, 120, 30);
            assert_eq!(
                colors(&drawn, x, top),
                (theme::FOREGROUND, theme::MARKED),
                "view {view}"
            );
            assert_eq!(
                colors(&drawn, x, top + 1),
                (theme::FOREGROUND, theme::CURSOR),
                "view {view}"
            );
            assert_eq!(
                colors(&drawn, x, top + 2),
                (theme::FOREGROUND, Color::Reset),
                "view {view}"
            );
        }
        // Tiles in the grid: a frame around each, and the name bold.
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let grid = buffer(&mut f.app, 120, 30);
        let frames: Vec<(u16, u16)> = (0..120)
            .filter(|x| grid[(*x, 2)].symbol() == "┌")
            .map(|x| (x, 2))
            .collect();
        assert_eq!(frames.len(), 2, "{frames:?}");
        for at in &frames {
            assert_eq!(grid[*at].fg, theme::ACCENT);
        }
        let (x, y) = find(&grid, "Documents", 0..120).unwrap();
        assert!(grid[(x, y)].modifier.contains(Modifier::BOLD));
        let (x, y) = find(&grid, "notes.txt", 0..120).unwrap();
        assert!(!grid[(x, y)].modifier.contains(Modifier::BOLD));

        // With the keyboard in the other pane, the frames go muted.
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::BackTab]);
        press(&mut f.app, &[KeyCode::Tab]);
        let drawn = buffer(&mut f.app, 120, 30);
        let corner = find(&drawn, "┌", 0..60).unwrap();
        assert_eq!(drawn[corner].fg, theme::MUTED);
    }
}
