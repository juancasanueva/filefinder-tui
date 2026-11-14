//! Drawing: flea's frame of a header, a rule, the body, a rule and the
//! footer, with one function per region.

mod card;
mod columns;
mod dual;
mod footer;
mod grid;
mod help;
mod list;
mod look;
mod menu;
mod places;
mod preview;
mod prompt;
mod topbar;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::Image;
use ratatui_image::protocol::Protocol;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, HitMap, Pane, Spot, ViewMode};
use crate::model::{Category, Entry};
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

/// The widths of the places panel, the files and the preview, in that
/// order, for a window `columns` wide and a panel that `needed` cells fit;
/// the preview's is 0 when it is hidden.
/// A `│` follows each of the first two.
pub fn pane_widths(columns: u16, preview: bool, needed: u16) -> (u16, u16, u16) {
    let columns = columns as usize;
    // flea's shares: the panel 22%, the files 40%, the preview the rest.
    let most = (columns * 22 / 100).saturating_sub(1);
    let left = most.min(needed as usize);
    let right = if preview {
        columns.saturating_sub(most + (columns * 40 / 100).saturating_sub(1) + 2)
    } else {
        0
    };
    // What the panel doesn't need goes to the files.
    let middle = if preview {
        columns.saturating_sub(left + right + 2)
    } else {
        columns.saturating_sub(left + 1)
    };
    (left as u16, middle as u16, right as u16)
}

fn draw_window(frame: &mut Frame, app: &mut App, hits: &mut HitMap) {
    let area = frame.area();
    if let Some(thumbs) = &mut app.thumbs {
        let covered = app.help_visible || app.dialog.is_some() || app.menu.is_some();
        thumbs.cover(covered || app.look.is_some());
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

    let preview = app.shows_preview();
    let needed = places::needed(&app.places);
    let (left, middle, right) = pane_widths(area.width, preview, needed);
    let column = |x: u16, width: u16| Rect::new(body.x + x, body.y, width, body.height);
    // Where the lines between the panes meet the rules.
    let mut junctions = Vec::new();
    // The places panel on the left in every view, when it shows; the files
    // between it and the preview, or in their room when either is hidden.
    let panel = app.places_visible;
    let start = if panel { left + 1 } else { 0 };
    let end = if preview {
        left + middle + 1
    } else {
        body.width
    };
    let files = column(start, end.saturating_sub(start));
    if panel {
        junctions.push(left);
    }
    if preview {
        junctions.push(left + middle + 1);
    }
    let tracks = match app.view() {
        // As many columns as the room left for them takes.
        ViewMode::Columns => columns::tracks(app, files, files.width),
        _ => Vec::new(),
    };
    junctions.extend(tracks.iter().skip(1).map(|track| track.x - body.x - 1));
    if app.view() == ViewMode::Dual {
        junctions.push(start + dual::divider(files.width));
    }
    rule(frame, top_rule, &junctions, '┬');
    rule(frame, bottom_rule, &junctions, '┴');

    if panel {
        places::draw(frame, app, column(0, left), hits);
        separator(frame, column(left, 1));
    }
    let item = match app.view() {
        ViewMode::Dual => dual::draw(frame, app, files, hits),
        ViewMode::Columns => columns::draw(frame, app, &tracks, hits),
        _ => draw_pane(frame, app, app.active(), files, hits),
    };
    if preview {
        separator(frame, column(left + middle + 1, 1));
        preview::draw(frame, app, column(left + middle + 2, right), hits);
    }
    // The area with the keyboard: its stretch of both rules and the lines
    // on its sides lit, from the line (or edge) left of it to the one right.
    let list = (
        if panel { left } else { 0 },
        if preview { end } else { body.width - 1 },
    );
    let span = match app.focus {
        Focus::Places if panel => Some((0, left)),
        Focus::Preview if preview => Some((left + middle + 1, body.width - 1)),
        Focus::List if app.view() == ViewMode::Dual => {
            let line = start + dual::divider(files.width);
            Some(if app.active() == 0 {
                (list.0, line)
            } else {
                (line, list.1)
            })
        }
        Focus::List => Some(list),
        _ => None,
    };
    if let Some((from, to)) = span {
        light(
            frame,
            [top_rule.y, bottom_rule.y],
            body,
            (from, to),
            &junctions,
        );
    }
    // The bottom row asks while a paste waits or a prompt is open.
    if let Some(question) = &app.question {
        prompt::question(frame, question, footer);
    } else if let Some(open) = &app.prompt {
        prompt::draw(frame, open, footer);
    } else if let Some(field) = &app.field {
        prompt::draw_field(frame, app, field, footer);
    } else {
        footer::draw(frame, app, footer);
    }

    if let Some(menu) = &app.menu {
        // Under the item it is about, at its name.
        menu::draw(frame, menu, item, body, hits);
    }
    // A card is the one lit thing: everything behind it is dimmed.
    if app.help_visible || app.look.is_some() || app.dialog.is_some() {
        card::dim(frame.buffer_mut(), area);
    }
    if app.help_visible {
        help::draw(frame, app, area, hits);
    }
    if app.look.is_some() {
        look::draw(frame, app, area, hits);
    }
    if let Some(dialog) = &app.dialog {
        card::draw_dialog(frame, dialog, area, hits);
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

/// Lights the cells of the rules on rows `rules` from column `from` to
/// `to`, and the lines between panes at either end, all the body down.
fn light(frame: &mut Frame, rules: [u16; 2], body: Rect, (from, to): (u16, u16), lines: &[u16]) {
    let ink = theme::ink(theme::ACCENT);
    let buffer = frame.buffer_mut();
    for y in rules {
        for x in from..=to {
            buffer[(body.x + x, y)].set_fg(ink);
        }
    }
    for x in [from, to].into_iter().filter(|x| lines.contains(x)) {
        for y in body.y..body.y + body.height {
            buffer[(body.x + x, y)].set_fg(ink);
        }
    }
}

/// The line between two panes, as tall as the body.
fn separator(frame: &mut Frame, area: Rect) {
    let lines: Vec<Line> = (0..area.height).map(|_| Line::from("│")).collect();
    frame.render_widget(Paragraph::new(lines).style(theme::border()), area);
}

/// The cells in front of a name in a row: the edge, the glyph and a space.
const NAME_INDENT: u16 = 3;

/// The cells of the item under the cursor of a pane whose view was just
/// drawn in `area`: its row from the name on, or its tile's picture. With
/// nothing under the cursor, the top of the pane.
fn item_cell(app: &App, hits: &HitMap, pane: usize, area: Rect) -> Rect {
    let index = pane;
    let pane = &app.panes()[pane];
    let top = Rect::new(area.x + NAME_INDENT, area.y + 1, 0, 0);
    if pane.entries.is_empty() {
        return top;
    }
    if app.view() == ViewMode::Grid {
        return grid::thumb_rect(area, app.tile, pane.offset, pane.cursor).unwrap_or(top);
    }
    // The row the view drew it on.
    let item = Spot::Item {
        pane: index,
        index: pane.cursor,
    };
    let row = hits.rect_of(&item);
    row.map_or(top, |row| Rect::new(row.x + NAME_INDENT, row.y, 0, 1))
}

/// Whether a pane's list has the keyboard.
fn focused(app: &App, pane: usize) -> bool {
    app.focus == Focus::List && app.active() == pane
}

/// One pane's items as rows, or as tiles in the grid. Returns the cells of
/// the item under its cursor.
fn draw_pane(frame: &mut Frame, app: &mut App, pane: usize, area: Rect, hits: &mut HitMap) -> Rect {
    let focused = focused(app, pane);
    hits.add(area, Spot::Pane(pane));
    match app.view() {
        ViewMode::Grid => grid::draw(frame, app, pane, area, focused, hits),
        _ => list::draw(frame, app, pane, area, focused, hits),
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

/// Draws an app's icon over its glyph and the space after it in a list
/// row, where the terminal draws pictures sharp enough; until the icon is
/// ready, and anywhere else, the glyph stays.
pub fn app_icon(frame: &mut Frame, thumbs: Option<&mut Thumbs>, entry: &Entry, row: Rect) {
    let Some(thumbs) = thumbs.filter(|thumbs| thumbs.sharp()) else {
        return;
    };
    // A row narrower than four cells has no glyph.
    if entry.category() != Category::App || row.width < 4 {
        return;
    }
    let slot = Rect::new(row.x + 1, row.y, 2, 1);
    picture(frame, Some(thumbs), entry, slot, slot);
}

/// The rows of a list that are apps, for `app_icon` to draw over once the
/// list is drawn.
pub type AppRows = Vec<(Entry, Rect)>;

/// Keeps a row for `app_icon` when its item is an app.
pub fn note_app(apps: &mut AppRows, entry: &Entry, row: Rect) {
    if entry.category() == Category::App {
        apps.push((entry.clone(), row));
    }
}

/// flea's captions for an empty folder, which take turns.
pub const CAPTIONS: [&str; crate::app::CAPTIONS] =
    ["NOTHING HERE YET", "A VERY TIDY FOLDER", "QUIET IN HERE"];

/// What a view says in place of the items of a pane that has none.
pub fn empty_text(pane: &Pane, caption: usize) -> String {
    let running = match (pane.search(), pane.filter()) {
        (Some(search), _) => search.is_running(),
        (_, Some(filter)) => filter.is_running(),
        _ => false,
    };
    // Find of nothing but `tag:` terms says what it asked for.
    let asked = pane
        .search()
        .map(|search| search.input.text().trim().to_string());
    let query = match pane.matching() {
        query if query.is_empty() => asked.unwrap_or_default(),
        query => query,
    };
    match query {
        query if query.is_empty() => CAPTIONS[caption % CAPTIONS.len()].to_string(),
        // Still looking: it may find some yet.
        _ if running => String::new(),
        query => format!("NOTHING MATCHES {query}"),
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
/// The marks of the tags an item carries, after its name: a space, then a
/// mark for each in the order of the seven, on the row's own fill.
pub fn tag_marks(tags: u8, style: Style) -> Vec<Span<'static>> {
    if tags == 0 {
        return Vec::new();
    }
    let mut spans = vec![Span::styled(" ", style)];
    for index in (0..crate::tags::NAMES.len()).filter(|index| tags & 1 << index != 0) {
        let (glyph, color) = theme::tag_mark(index);
        spans.push(Span::styled(glyph, style.fg(color)));
    }
    spans
}

/// How many cells `tag_marks` takes.
pub fn tag_width(tags: u8) -> usize {
    match tags.count_ones() {
        0 => 0,
        count => count as usize + 1,
    }
}

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

    /// Where the window lays its panes across `width` columns for an app,
    /// from the real layout: the places panel (its line right after it),
    /// the files (up to the preview's line, or the edge) and the preview
    /// (empty when it is hidden).
    pub struct Panes {
        pub panel: Range<u16>,
        pub files: Range<u16>,
        pub preview: Range<u16>,
    }

    pub fn panes(app: &App, width: u16) -> Panes {
        let preview = app.shows_preview();
        let (left, middle, _) = pane_widths(width, preview, places::needed(&app.places));
        let start = if app.places_visible { left + 1 } else { 0 };
        let end = if preview { left + middle + 1 } else { width };
        Panes {
            panel: 0..left,
            files: start..end,
            preview: if preview {
                end + 1..width
            } else {
                width..width
            },
        }
    }

    /// The columns of each track the columns view lays out in the files,
    /// for an app drawn `width` wide, leftmost first.
    pub fn tracks(app: &App, width: u16) -> Vec<Range<u16>> {
        let files = panes(app, width).files;
        let area = Rect::new(files.start, 0, files.end - files.start, 1);
        let tracks = columns::tracks(app, area, area.width);
        tracks.iter().map(|track| track.x..track.right()).collect()
    }

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
    use super::testing::{buffer, find, panes, screen, tracks};
    use super::*;
    use crate::app::testing::{
        ctrl, finish_search, finish_work, fixture, key, manual_clock, press, shift, split,
        type_text,
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
        // A panel that needs all of flea's 22%, or more: cut to it.
        assert_eq!(pane_widths(120, true, 40), (25, 47, 46));
        assert_eq!(pane_widths(80, true, 16), (16, 31, 31));
        assert_eq!(pane_widths(120, false, 25), (25, 94, 0));
        assert_eq!(pane_widths(80, false, 30), (16, 63, 0));
        // One that needs less takes only that; the list takes the rest and
        // the preview keeps its width.
        assert_eq!(pane_widths(120, true, 17), (17, 55, 46));
        assert_eq!(pane_widths(120, false, 17), (17, 102, 0));
        assert_eq!(pane_widths(80, true, 10), (10, 37, 31));
    }

    /// Which cells of the two rules and the lines between the panes are
    /// lit, as (first, last) column of the lit stretch; checks each cell.
    fn assert_lit(drawn: &Buffer, from: u16, to: u16, label: &str) {
        let width = drawn.area.width;
        let lines: Vec<u16> = (0..width)
            .filter(|x| drawn[(*x, 1)].symbol() == "┬")
            .collect();
        for y in [1, drawn.area.height - 2] {
            for x in 0..width {
                let wanted = if (from..=to).contains(&x) {
                    theme::ACCENT
                } else {
                    theme::BORDER
                };
                assert_eq!(drawn[(x, y)].fg, wanted, "{label}: rule {x}, {y}");
            }
        }
        for x in lines {
            let wanted = if x == from || x == to {
                theme::ACCENT
            } else {
                theme::BORDER
            };
            for y in 2..drawn.area.height - 2 {
                assert_eq!(drawn[(x, y)].fg, wanted, "{label}: line {x}, {y}");
            }
        }
    }

    #[test]
    fn the_area_with_the_keyboard_has_its_frame_lit() {
        let mut f = in_pictures();
        // The panel's line, then the preview's, as the layout puts them.
        let files = panes(&f.app, 120).files;
        let (line, end) = (files.start - 1, files.end);
        for (focus, from, to) in [
            (Focus::List, line, end),
            (Focus::Preview, end, 119),
            (Focus::Places, 0, line),
        ] {
            f.app.focus = focus;
            let drawn = buffer(&mut f.app, 120, 30);
            assert_lit(&drawn, from, to, &format!("{focus:?}"));
        }
        // Tab moves the keyboard, and the light with it.
        f.app.focus = Focus::List;
        press(&mut f.app, &[KeyCode::Tab]);
        assert_eq!(f.app.focus, Focus::Preview);
        assert_lit(&buffer(&mut f.app, 120, 30), end, 119, "after Tab");

        // Two panes: the active one's stretch, up to the line between them.
        f.app.focus = Focus::List;
        f.app.handle_key(ctrl(KeyCode::Char('4')));
        let drawn = buffer(&mut f.app, 120, 30);
        let lines: Vec<u16> = (0..120)
            .filter(|x| drawn[(*x, 1)].symbol() == "┬")
            .collect();
        let divider = *lines.last().unwrap();
        let active = f.app.tab().active;
        let (from, to) = if active == 0 {
            (line, divider)
        } else {
            (divider, 119)
        };
        assert_lit(&drawn, from, to, "the active pane");
        f.app.tab_mut().active = 1 - active;
        let drawn = buffer(&mut f.app, 120, 30);
        let (from, to) = if active == 0 {
            (divider, 119)
        } else {
            (line, divider)
        };
        assert_lit(&drawn, from, to, "the other pane");
    }

    #[test]
    fn the_frame_is_a_header_a_rule_the_body_a_rule_and_the_footer() {
        let mut f = in_pictures();
        let drawn = buffer(&mut f.app, 120, 30);
        // The panel's line, then the preview's, as the layout puts them.
        let files = panes(&f.app, 120).files;
        let (line, end) = (files.start - 1, files.end);
        assert_eq!(find(&drawn, "~/Pictures", 0..120), Some((1, 0)));
        for (y, junction) in [(1, "┬"), (28, "┴")] {
            for x in 0..120 {
                let wanted = if x == line || x == end {
                    junction
                } else {
                    "─"
                };
                assert_eq!(drawn[(x, y)].symbol(), wanted, "{x}, {y}");
                // The list has the keyboard: its stretch is lit.
                let ink = if (line..=end).contains(&x) {
                    theme::ACCENT
                } else {
                    theme::BORDER
                };
                assert_eq!(drawn[(x, y)].fg, ink, "{x}, {y}");
            }
        }
        // A line between the panes all the way down the body, both by the
        // list here.
        for y in 2..28 {
            for x in [line, end] {
                assert_eq!(drawn[(x, y)].symbol(), "│", "{x}, {y}");
                assert_eq!(drawn[(x, y)].fg, theme::ACCENT);
            }
        }
        assert_eq!(find(&drawn, "2 items", 0..120), Some((1, 29)));
        // The terminal's own background everywhere outside the fills.
        assert_eq!(drawn[(50, 20)].bg, Color::Reset);

        // Without the preview the list takes its room, and one junction stays.
        f.app
            .handle_key(crate::app::testing::alt(KeyCode::Char('p')));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(line, 1)].symbol(), "┬");
        assert_eq!(drawn[(end, 1)].symbol(), "─");
        assert_eq!(drawn[(end, 10)].symbol(), " ");
        assert_eq!(
            drawn[(119, 3)].bg,
            theme::CURSOR,
            "the cursor row to the edge"
        );
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
    fn list_rows_are_an_edge_a_glyph_the_name_and_the_size() {
        let mut f = in_pictures();
        let drawn = buffer(&mut f.app, 120, 30);
        // The list's columns: the edge, the glyph, a space, the name; then the
        // columns, each under its title in the header on the list's first row.
        let files = panes(&f.app, 120).files;
        let (edge, glyph, name) = (files.start, files.start + 1, files.start + 3);
        assert_eq!(find(&drawn, "Name ▴", files.clone()), Some((name, 2)));
        assert_eq!(drawn[(name, 2)].fg, theme::FOREGROUND, "the sorted title");
        let (size, _) = find(&drawn, "Size", files.clone()).expect("the size's title");
        assert_eq!(drawn[(size, 2)].fg, theme::MUTED);
        // Sizes end where their title does.
        let size_end = size + 4;
        // Trip, under the cursor in the list that has the keyboard.
        assert_eq!(drawn[(edge, 3)].symbol(), "▎");
        assert_eq!(drawn[(edge, 3)].fg, theme::ACCENT);
        assert_eq!(drawn[(glyph, 3)].symbol(), icons::FOLDER);
        assert_eq!(
            drawn[(glyph, 3)].fg,
            theme::ICON_FOLDER,
            "a folder's colour"
        );
        assert_eq!(find(&drawn, "Trip", files.clone()), Some((name, 3)));
        assert_eq!(
            find(&drawn, "No items", files.clone()),
            Some((size_end - 8, 3))
        );
        for x in files.clone() {
            assert_eq!(drawn[(x, 3)].bg, theme::CURSOR, "{x}");
        }
        // a.jpg: no fill, the glyph in a picture's colour, the size in the
        // metadata ink.
        assert_eq!(drawn[(edge, 4)].symbol(), " ");
        assert_eq!(drawn[(glyph, 4)].fg, theme::ICON_IMAGE);
        assert_eq!(find(&drawn, "a.jpg", files.clone()), Some((name, 4)));
        assert_eq!(drawn[(name, 4)].fg, theme::FOREGROUND);
        assert_eq!(find(&drawn, "3 B", files.clone()), Some((size_end - 3, 4)));
        assert_eq!(drawn[(size_end - 3, 4)].fg, theme::MUTED);
        assert_eq!(drawn[(name + 11, 4)].bg, Color::Reset);

        // With the keyboard in the preview the cursor row loses its edge.
        press(&mut f.app, &[KeyCode::Tab]);
        let away = buffer(&mut f.app, 120, 30);
        assert_eq!(away[(edge, 3)].symbol(), " ");
        assert_eq!(away[(edge, 3)].bg, theme::SELECTED);

        // A long name is cut with an ellipsis, a cell before the columns.
        fs::write(
            f.home
                .join("Pictures")
                .join(format!("{}.txt", "x".repeat(60))),
            "",
        )
        .unwrap();
        press(&mut f.app, &[KeyCode::Esc, KeyCode::F(5)]);
        let cut = buffer(&mut f.app, 120, 30);
        let (ellipsis, y) = find(&cut, "… ", files.clone()).expect("the cut name");
        assert_eq!(y, 5);
        assert_eq!(
            cells(&cut, 5, name..ellipsis),
            "x".repeat((ellipsis - name) as usize)
        );
        assert_eq!(find(&cut, "0 B", files.clone()), Some((size_end - 3, 5)));
        assert!(ellipsis + 1 < size_end - 3);
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
        let files = panes(&f.app, 120).files;
        let ink = |name: &str| {
            let (x, y) = find(&drawn, name, files.clone()).unwrap();
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
        let (x, y) = find(&marked, "run.sh", files.clone()).unwrap();
        assert_eq!(
            (marked[(x, y)].fg, marked[(x, y)].bg),
            (theme::FOREGROUND, theme::MARKED)
        );
        let (x, y) = find(&marked, "link", files).unwrap();
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
        let files = panes(&f.app, 120).files;
        // a.jpg is the cursor's, Trip marked with it; b.jpg neither.
        let at = |name: &str| find(&drawn, name, files.clone()).unwrap();
        let (x, y) = at("a.jpg");
        assert!(drawn[(x, y)].modifier.contains(Modifier::REVERSED));
        assert_eq!(drawn[(files.start, y)].symbol(), " ", "no edge");
        assert_eq!(
            (drawn[(x, y)].fg, drawn[(x, y)].bg),
            (Color::Reset, Color::Reset)
        );
        let (x, y) = at("Trip");
        assert!(drawn[(x, y)].modifier.contains(Modifier::UNDERLINED));
        assert!(!drawn[(x, y)].modifier.contains(Modifier::REVERSED));
        let (x, y) = at("b.jpg");
        assert!(drawn[(x, y)].modifier.is_empty());
        let line = (files.start - 1, 10);
        assert_eq!(drawn[line].fg, Color::Reset, "no colour anywhere");

        // Sixteen colours: reverse video, and the marked fill.
        f.app.depth = Depth::Sixteen;
        let drawn = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&drawn, "a.jpg", files.clone()).unwrap();
        assert!(drawn[(x, y)].modifier.contains(Modifier::REVERSED));
        let (x, y) = find(&drawn, "Trip", files).unwrap();
        assert_eq!(drawn[(x, y)].bg, Color::DarkGray);
    }

    #[test]
    fn in_256_colours_the_palette_is_the_nearest_index() {
        let mut f = in_pictures();
        f.app.depth = Depth::Indexed;
        let drawn = buffer(&mut f.app, 120, 30);
        // The list's edge, its names three cells in, a.jpg's size.
        let files = panes(&f.app, 120).files;
        let (edge, name) = (files.start, files.start + 3);
        let (size, _) = find(&drawn, "3 B", files.clone()).expect("a.jpg's size");
        assert_eq!(drawn[(name, 4)].fg, Color::Indexed(252), "a.jpg");
        assert_eq!(drawn[(size, 4)].fg, Color::Indexed(246), "its size");
        assert_eq!(
            drawn[(name, 3)].bg,
            Color::Indexed(236),
            "the cursor's fill"
        );
        assert_eq!(
            drawn[(100, 1)].fg,
            Color::Indexed(235),
            "a rule away from the keyboard"
        );
        assert_eq!(
            drawn[(edge - 1, 10)].fg,
            Color::Indexed(252),
            "the line by the list, which has the keyboard"
        );
        assert_eq!(drawn[(edge, 3)].symbol(), "▎");
    }

    #[test]
    fn what_a_search_matches_is_bold_and_where_it_was_found_follows_the_name() {
        let mut f = in_pictures();
        fs::write(f.home.join("Pictures").join("Trip").join("day.jpg"), "").unwrap();
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "JP");
        finish_search(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let files = panes(&f.app, 120).files;
        let (x, y) = find(&drawn, "a.jpg", files.clone()).unwrap();
        let bold = |x: u16| drawn[(x, y)].modifier.contains(Modifier::BOLD);
        assert_eq!(
            (bold(x), bold(x + 1), bold(x + 2), bold(x + 3), bold(x + 4)),
            (false, false, true, true, false)
        );
        let (x, y) = find(&drawn, "day.jpg", files.clone()).unwrap();
        assert_eq!(find(&drawn, "day.jpg Trip", files), Some((x, y)));
        assert_eq!(drawn[(x + 8, y)].fg, theme::MUTED);
    }

    #[test]
    fn what_the_filter_finds_in_subfolders_says_which_folder_it_is_in() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "a.jp");
        press(&mut f.app, &[KeyCode::Tab]);
        finish_search(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let files = panes(&f.app, 120).files;
        assert!(
            find(&drawn, "a.jpg Pictures", files).is_some(),
            "where it is"
        );
    }

    #[test]
    fn the_filter_and_find_fields_sit_on_the_bottom_row_like_flea_s_prompts() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('/')]);
        type_text(&mut f.app, "no");
        let drawn = buffer(&mut f.app, 100, 20);
        assert_eq!(find(&drawn, "Filter no", 0..100), Some((1, 19)));
        assert_eq!(drawn[(1, 19)].fg, theme::MUTED, "the label");
        assert_eq!(
            (drawn[(8, 19)].fg, drawn[(8, 19)].bg),
            (theme::FOREGROUND, theme::FIELD)
        );
        assert_eq!(drawn[(10, 19)].bg, theme::FIELD, "the caret's cell");
        assert_eq!(crate::ui::testing::cursor(&mut f.app, 100, 20), (10, 19));
        let (x, _) = find(&drawn, "1 of 3 shown", 0..100).unwrap();
        assert_eq!((x, drawn[(x, 19)].fg), (12, theme::MUTED));

        press(&mut f.app, &[KeyCode::Esc, KeyCode::Char('f')]);
        let drawn = buffer(&mut f.app, 100, 20);
        assert_eq!(find(&drawn, "Find", 0..100), Some((1, 19)));
        let (x, _) = find(&drawn, "in ~ · tab changes scope", 0..100).unwrap();
        assert_eq!(drawn[(x, 19)].fg, theme::MUTED);
        // The header keeps the path alone.
        assert_eq!(drawn[(1, 0)].symbol(), "~");
        assert!(!cells(&drawn, 0, 0..100).contains('/'));
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

        // An error in the error colour, gone after eight seconds. The name is
        // taken now: the paste asks, and `b` keeps both.
        f.app.handle_key(ctrl(KeyCode::Char('v')));
        press(&mut f.app, &[KeyCode::Char('b')]);
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
            // The list's 47 cells and the 25 rows under its header: the
            // glyph on row 11 of them, the caption under it.
            let glyph = cells(&drawn, 14, 26..73);
            assert_eq!(glyph.trim(), icons::FOLDER);
            assert_eq!(drawn[(49, 14)].fg, theme::MUTED);
            cells(&drawn, 15, 26..73).trim().to_string()
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
        press(&mut f.app, &[KeyCode::Char('f')]);
        type_text(&mut f.app, "zebra");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);
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
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let drawn = buffer(&mut f.app, 120, 30);
        let (_, y) = find(&drawn, "NOTHING MATCHES zebra", 26..73).unwrap();
        assert_eq!(cells(&drawn, y - 1, 26..73).trim(), icons::SEARCH);
        // An empty folder still says what it says.
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Enter]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "NOTHING HERE YET", 26..73).is_some());
    }

    #[test]
    fn a_picture_that_arrives_under_a_dialog_waits_for_it_to_go() {
        use crate::app::testing::{pictures, settle};
        use crate::thumbs::testing::painted;
        let (mut f, _thumbnailer) = pictures();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        f.app.handle_key(shift(KeyCode::Delete));
        settle(&mut f.app);
        // The card ends left of column 96; the preview's picture is under
        // its head, from row 5, and goes on past the card.
        let preview = |drawn: &Buffer| {
            let cells = (96..120).flat_map(|x| (5..28).map(move |y| (x, y)));
            cells.filter(|at| painted(&drawn[*at])).count()
        };
        let covered = buffer(&mut f.app, 120, 30);
        assert!(find(&covered, "Delete 1 item permanently?", 0..120).is_some());
        assert_eq!(preview(&covered), 0);
        press(&mut f.app, &[KeyCode::Esc]);
        let clear = buffer(&mut f.app, 120, 30);
        assert!(preview(&clear) > 0);
        let head = (96..120).filter(|x| painted(&clear[(*x, 2)])).count();
        assert_eq!(head, 0, "never over the name");
    }

    #[test]
    fn ctrl_1_2_3_draw_the_grid_the_list_and_the_columns_each_beside_the_preview() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let grid = buffer(&mut f.app, 120, 30);
        let panes = panes(&f.app, 120);
        let files = || panes.files.clone();
        let (line, end) = (panes.files.start - 1, panes.files.end);
        // The tiles between the places panel and the preview, the names
        // under the pictures.
        let (x, _) = find(&grid, "notes.txt", files()).expect("a tile");
        assert!(x > panes.files.start);
        assert_eq!(grid[(line, 10)].symbol(), "│", "the places panel's line");
        assert_eq!(grid[(line, 1)].symbol(), "┬");
        // The preview on the right, as in the list, of the cursor's item.
        assert_eq!(grid[(end, 1)].symbol(), "┬");
        assert_eq!(grid[(end, 28)].symbol(), "┴");
        assert_eq!(grid[(end, 10)].symbol(), "│");
        assert_eq!(
            find(&grid, "Documents", panes.preview.clone()).map(|(_, y)| y),
            Some(2)
        );

        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let list = buffer(&mut f.app, 120, 30);
        assert_eq!(
            find(&list, "notes.txt", files()),
            Some((panes.files.start + 3, 5))
        );
        assert_eq!(find(&list, "5 B", files()).map(|(_, y)| y), Some(5));
        assert_eq!(list[(line, 10)].symbol(), "│");

        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let columns = buffer(&mut f.app, 120, 30);
        // At home, home is the only column: it takes the room between the
        // places panel and the preview.
        assert_eq!(
            find(&columns, "notes.txt", files()),
            Some((panes.files.start + 3, 4))
        );
        assert_eq!(find(&columns, "5 B", files()).map(|(_, y)| y), Some(4));
        assert_eq!(columns[(line, 10)].symbol(), "│");
        assert_eq!(columns[(end, 10)].symbol(), "│");
        assert_eq!(
            find(&columns, "Documents", panes.preview.clone()).map(|(_, y)| y),
            Some(2)
        );
    }

    #[test]
    fn the_columns_run_from_home_down_with_the_way_there_on_the_selected_fill() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('l')]);
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        // Room for two columns in the cells between the places panel and
        // the preview: home, then Pictures, a line between them.
        let [home, pictures] = <[_; 2]>::try_from(tracks(&f.app, 120)).unwrap();
        let between = home.end;
        let preview = panes(&f.app, 120).preview;
        for y in 2..28 {
            assert_eq!(drawn[(between, y)].symbol(), "│", "row {y}");
            assert_eq!(drawn[(between, y)].fg, theme::BORDER);
        }
        assert_eq!(
            (drawn[(between, 1)].symbol(), drawn[(between, 28)].symbol()),
            ("┬", "┴")
        );
        assert_eq!(
            (
                drawn[(pictures.end, 1)].symbol(),
                drawn[(pictures.end, 28)].symbol()
            ),
            ("┬", "┴")
        );
        // Home: muted, the way down on the selected fill without the edge.
        assert_eq!(
            find(&drawn, "Documents", home.clone()),
            Some((home.start + 3, 2))
        );
        assert_eq!(drawn[(home.start + 3, 2)].fg, theme::MUTED);
        assert_eq!(
            cells(&drawn, 3, home.start + 3..home.start + 11),
            "Pictures"
        );
        assert_eq!(drawn[(home.start + 3, 3)].fg, theme::FOREGROUND);
        assert_eq!(
            (drawn[(home.start, 3)].symbol(), drawn[(home.start, 3)].bg),
            (" ", theme::SELECTED)
        );
        assert_eq!(drawn[(home.end - 1, 3)].bg, theme::SELECTED);
        assert!(
            !cells(&drawn, 4, home.clone()).contains("5 B"),
            "no sizes above"
        );
        // Pictures: the cursor with the accent edge, the detail column.
        assert_eq!(
            find(&drawn, "Trip", pictures.clone()),
            Some((pictures.start + 3, 2))
        );
        assert_eq!(
            (
                drawn[(pictures.start, 2)].symbol(),
                drawn[(pictures.start, 2)].fg
            ),
            ("▎", theme::ACCENT)
        );
        assert_eq!(
            (
                drawn[(pictures.start, 2)].bg,
                drawn[(pictures.end - 1, 2)].bg
            ),
            (theme::CURSOR, theme::CURSOR)
        );
        assert_eq!(
            find(&drawn, "a.jpg", pictures.clone()),
            Some((pictures.start + 3, 3))
        );
        assert_eq!(
            find(&drawn, "3 B", pictures.clone()).map(|(_, y)| y),
            Some(3)
        );
        // The preview of Trip on the right.
        assert_eq!(find(&drawn, "Trip", preview).map(|(_, y)| y), Some(2));

        // `l` goes on into Trip: Pictures, then Trip, empty; `h` back out.
        press(&mut f.app, &[KeyCode::Char('l')]);
        let deeper = buffer(&mut f.app, 120, 30);
        assert_eq!(deeper[(between, 10)].symbol(), "│");
        assert_eq!(
            find(&deeper, "a.jpg", home.clone()),
            Some((home.start + 3, 3))
        );
        assert!(find(&deeper, "NOTHING HERE YET", pictures).is_some());
        press(&mut f.app, &[KeyCode::Char('h')]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        assert_eq!(
            f.app.selected().map(|entry| entry.name.as_str()),
            Some("Trip")
        );
    }

    #[test]
    fn the_columns_keep_the_rightmost_folders_when_the_way_down_is_long() {
        let mut f = fixture();
        let deep = f.home.join("Documents/alpha/bravo/charlie");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("inside.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        f.app.navigate(&deep);
        let drawn = buffer(&mut f.app, 120, 30);
        // Two of home, Documents, alpha, bravo and charlie in the cells
        // beside the preview: bravo's and charlie's own items, their names
        // three cells into each column.
        let files = panes(&f.app, 120).files;
        let name = |track: &std::ops::Range<u16>, text: &str| {
            let start = track.start + 3;
            start..start + text.len() as u16
        };
        let narrow = tracks(&f.app, 120);
        assert_eq!(narrow.len(), 2);
        assert_eq!(cells(&drawn, 2, name(&narrow[0], "charlie")), "charlie");
        assert_eq!(
            cells(&drawn, 2, name(&narrow[1], "inside.txt")),
            "inside.txt"
        );
        assert!((2..28).all(|y| !cells(&drawn, y, files.clone()).contains("Documents")));
        assert!(!cells(&drawn, 2, files).contains("bravo"));
        // Without the preview the wider room has room for three.
        f.app
            .handle_key(crate::app::testing::alt(KeyCode::Char('p')));
        let wide = buffer(&mut f.app, 120, 30);
        let three = tracks(&f.app, 120);
        assert_eq!(three.len(), 3);
        assert_eq!(cells(&wide, 2, name(&three[0], "bravo")), "bravo");
        assert_eq!(cells(&wide, 2, name(&three[1], "charlie")), "charlie");
        assert_eq!(cells(&wide, 2, name(&three[2], "inside.txt")), "inside.txt");
    }

    #[test]
    fn a_long_column_scrolls_to_keep_the_way_down_in_view() {
        let mut f = fixture();
        for n in 1..=30 {
            fs::create_dir(f.home.join(format!("Folder {n}"))).unwrap();
        }
        fs::write(f.home.join("Folder 30").join("inside.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        f.app.navigate(&f.home.join("Folder 30"));
        let drawn = buffer(&mut f.app, 120, 30);
        // Names three cells into each column: home's, then Folder 30's.
        let [home, folder] = <[_; 2]>::try_from(tracks(&f.app, 120)).unwrap();
        let name = home.start + 3;
        let y = (2..28).find(|y| cells(&drawn, *y, name..name + 9) == "Folder 30");
        let y = y.expect("the way down, in view");
        assert_eq!(drawn[(name, y)].bg, theme::SELECTED);
        let inside = find(&drawn, "inside.txt", folder.clone());
        assert_eq!(inside, Some((folder.start + 3, 2)));
    }

    #[test]
    fn a_click_in_a_column_above_goes_there() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('l')]);
        buffer(&mut f.app, 120, 30);
        f.app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 31,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(f.app.pane().cwd, f.home);
        assert_eq!(
            f.app.selected().map(|entry| entry.name.as_str()),
            Some("Documents")
        );
    }

    #[test]
    fn what_a_search_found_shows_in_each_view_and_in_a_single_column() {
        let mut f = fixture();
        let trip = f.home.join("Pictures").join("Trip");
        fs::write(trip.join("day.jpg"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Right, KeyCode::Char('f')],
        );
        type_text(&mut f.app, "jpg");
        finish_search(&mut f.app);
        press(&mut f.app, &[KeyCode::Enter]);

        // The home column steps out: what was found is the only one,
        // between the places panel and its line and the preview, its names
        // three cells in.
        let columns = buffer(&mut f.app, 130, 30);
        let files = panes(&f.app, 130).files;
        let name = files.start + 3;
        assert_eq!(find(&columns, "a.jpg", files.clone()), Some((name, 2)));
        assert_eq!(find(&columns, "day.jpg", files.clone()), Some((name, 3)));
        assert_eq!(find(&columns, "notes.txt", 0..130), None);
        assert_eq!(columns[(files.start, 2)].bg, theme::CURSOR, "the cursor");

        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let list = buffer(&mut f.app, 130, 30);
        let (x, y) = find(&list, "a.jpg", files.clone()).unwrap();
        assert_eq!(find(&list, "day.jpg", files.clone()), Some((x, y + 1)));
        assert!(find(&list, "3 B", 0..130).is_some(), "with its size");

        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let grid = buffer(&mut f.app, 130, 30);
        let (a, day) = (
            find(&grid, "a.jpg", files.clone()),
            find(&grid, "day.jpg", files.clone()),
        );
        let ((ax, ay), (dx, dy)) = (a.expect("the first"), day.expect("the second"));
        assert!(ay == dy && ax < dx, "side by side, best first");

        // Left, the folder is back with the ones above it.
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Esc]);
        let columns = buffer(&mut f.app, 130, 30);
        let [home, pictures] = <[_; 2]>::try_from(tracks(&f.app, 130)).unwrap();
        assert_eq!(
            find(&columns, "notes.txt", 0..130),
            Some((home.start + 3, 4))
        );
        assert_eq!(
            find(&columns, "a.jpg", files),
            Some((pictures.start + 3, 3))
        );
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

        // The body right of the places panel: the two halves and a line
        // between them meeting the rules; no preview.
        let files = panes(&f.app, 120).files;
        let (line, start) = (files.start - 1, files.start);
        let div = start + dual::divider(files.end - start);
        // Each pane's columns: its edge, its glyph and its names three in.
        let (left_edge, right_edge) = (start, div + 1);
        for y in 2..28 {
            assert_eq!(right[(div, y)].symbol(), "│", "row {y}");
            // The right pane has the keyboard: the line left of it is lit.
            assert_eq!(right[(div, y)].fg, theme::ACCENT);
        }
        assert_eq!(
            (right[(div, 1)].symbol(), right[(div, 28)].symbol()),
            ("┬", "┴")
        );
        for x in (0..120).filter(|x| *x != line && *x != div) {
            assert_eq!(
                (right[(x, 1)].symbol(), right[(x, 28)].symbol()),
                ("─", "─")
            );
        }
        // Each pane's path on its first row: the folder lit where the keyboard is.
        assert_eq!(cells(&right, 2, start..start + 3), " ~ ");
        assert_eq!(right[(start + 1, 2)].fg, theme::MUTED);
        assert_eq!(
            cells(&right, 2, right_edge..right_edge + 12),
            " ~/Pictures "
        );
        assert_eq!(right[(right_edge + 1, 2)].fg, theme::MUTED);
        assert_eq!(right[(right_edge + 3, 2)].fg, theme::FOREGROUND);
        // Under them a header, then the files, each pane's own, with its own
        // cursor: only the pane with the keyboard draws the edge.
        assert_eq!(cells(&right, 4, start + 3..start + 12), "Documents");
        assert_eq!(cells(&right, 5, start + 3..start + 11), "Pictures");
        assert_eq!(cells(&right, 4, right_edge + 3..right_edge + 7), "Trip");
        assert_eq!(cells(&right, 5, right_edge + 3..right_edge + 8), "a.jpg");
        assert_eq!(
            (right[(left_edge, 5)].symbol(), right[(left_edge, 5)].bg),
            (" ", theme::SELECTED)
        );
        assert_eq!(
            (right[(right_edge, 5)].symbol(), right[(right_edge, 5)].bg),
            ("▎", theme::CURSOR)
        );
        // The header is the right pane's.
        assert_eq!(find(&right, "~/Pictures", 0..60), Some((1, 0)));

        // With the keyboard in the left pane they are the left pane's.
        press(&mut f.app, &[KeyCode::BackTab]);
        f.app.tick();
        let left = buffer(&mut f.app, 120, 30);
        assert_eq!(left[(start + 1, 2)].fg, theme::FOREGROUND);
        assert_eq!(left[(right_edge + 3, 2)].fg, theme::MUTED);
        assert_eq!(left[(left_edge, 5)].symbol(), "▎");
        assert_eq!(left[(right_edge, 5)].symbol(), " ");
        assert_eq!(cells(&left, 0, 0..60).trim(), "~");

        // Closed, the window is what it was before.
        f.app.handle_key(key(KeyCode::Char('|')));
        f.app.tick();
        assert_eq!(screen(&mut f.app, 120, 30), single);
    }

    #[test]
    fn every_selected_item_is_drawn_selected_in_each_view() {
        let mut f = fixture();
        f.app.handle_key(shift(KeyCode::Down));
        let colors = |drawn: &Buffer, x, y| (drawn[(x, y)].fg, drawn[(x, y)].bg);

        // Documents and Pictures, not notes.txt: rows in the list and the columns.
        // The list's rows start under its header.
        for (view, x, top) in [('1', 29, 3), ('2', 29, 2)] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let drawn = buffer(&mut f.app, 120, 30);
            let at = |y| colors(&drawn, x, y);
            assert_eq!(at(top), (theme::FOREGROUND, theme::MARKED), "view {view}");
            assert_eq!(
                at(top + 1),
                (theme::FOREGROUND, theme::CURSOR),
                "view {view}"
            );
            assert_eq!(
                at(top + 2),
                (theme::FOREGROUND, Color::Reset),
                "view {view}"
            );
        }
        // Tiles in the grid: the marked fill and the cursor's.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let grid = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&grid, "Documents", 26..73).unwrap();
        assert_eq!(colors(&grid, x, y), (theme::FOREGROUND, theme::MARKED));
        let (x, y) = find(&grid, "Pictures", 26..73).unwrap();
        assert_eq!(colors(&grid, x, y), (theme::FOREGROUND, theme::CURSOR));
        let (x, y) = find(&grid, "notes.txt", 26..73).unwrap();
        assert_eq!(colors(&grid, x, y), (theme::FOREGROUND, Color::Reset));
    }
}
