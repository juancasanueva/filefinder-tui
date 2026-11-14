//! The grid view, Ctrl+3: tiles with a thumbnail over a caption of two
//! lines, wrapped and cut at word breaks, in the palette's colours: the cursor's tile
//! on the cursor fill with the accent edge, marked tiles on the marked fill.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::list::{self, Look, bold_match, inks};
use super::{fit, picture, scroll, tag_marks, tag_width};
use crate::app::{App, HitMap, Spot, TileSize};
use crate::icons;
use crate::model::{self, Entry};
use crate::theme;

/// The least room between two tiles, and between the tiles and the edges.
const GAP: u16 = 2;
/// A tile's rows besides the thumbnail: one of air above it and the two of
/// its caption under it.
const EXTRA_ROWS: u16 = 3;
/// The lines of a caption.
const CAPTION_LINES: usize = 2;

/// Cells across a tile: the thumbnail and one cell on each side.
pub fn tile_width(size: TileSize) -> u16 {
    match size {
        TileSize::Small => 14,
        TileSize::Normal => 18,
        TileSize::Large => 26,
    }
}

/// Rows of a tile's thumbnail area.
pub fn thumb_rows(size: TileSize) -> u16 {
    match size {
        TileSize::Small => 4,
        TileSize::Normal => 6,
        TileSize::Large => 9,
    }
}

/// How many tiles fit across a width; never fewer than one.
pub fn columns(width: u16, size: TileSize) -> usize {
    (width.saturating_sub(GAP) / (tile_width(size) + GAP)).max(1) as usize
}

/// How many whole rows of tiles fit in a height; never fewer than one.
pub fn visible_rows(height: u16, size: TileSize) -> usize {
    (height / (thumb_rows(size) + EXTRA_ROWS)).max(1) as usize
}

/// Where tile `index` goes in `area` once `offset` rows have scrolled away, or
/// `None` when it is off screen. It may hang over the bottom edge.
fn tile_rect(area: Rect, size: TileSize, offset: usize, index: usize) -> Option<Rect> {
    let width = tile_width(size);
    let height = thumb_rows(size) + EXTRA_ROWS;
    let columns = columns(area.width, size);
    let row = (index / columns).checked_sub(offset)?;
    let y = area.y as usize + row * height as usize;
    if y >= area.bottom() as usize {
        return None;
    }
    // The room left over is shared out evenly around the tiles.
    let gap = area.width.saturating_sub(columns as u16 * width) / (columns as u16 + 1);
    let x = area.x + gap + (index % columns) as u16 * (width + gap);
    Some(Rect::new(x, y as u16, width, height))
}

/// The cells tile `index` keeps for its thumbnail, or `None` when none of them
/// are on screen; shorter than [`thumb_rows`] when the bottom edge cuts the row.
pub fn thumb_rect(area: Rect, size: TileSize, offset: usize, index: usize) -> Option<Rect> {
    let tile = tile_rect(area, size, offset, index)?;
    clip(thumb_of(tile, size), area)
}

fn thumb_of(tile: Rect, size: TileSize) -> Rect {
    Rect::new(
        tile.x + 1,
        tile.y + 1,
        tile.width.saturating_sub(2),
        thumb_rows(size),
    )
}

/// The part of a rectangle inside the area, when there is any.
fn clip(rect: Rect, area: Rect) -> Option<Rect> {
    let inside = rect.intersection(area);
    (!inside.is_empty()).then_some(inside)
}

/// Draws one pane's items as tiles; `focused` is whether it has the keyboard.
/// Each tile, as much of it as shows, is a spot for the mouse.
pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    pane_index: usize,
    area: Rect,
    focused: bool,
    hits: &mut HitMap,
) {
    let size = app.tile;
    let columns = columns(area.width, size);
    let rows = visible_rows(area.height, size);
    let caption = app.caption;
    let tab = &mut app.tabs[app.current];
    let pane = &mut tab.panes[pane_index];
    pane.grid_columns = columns;
    pane.page_rows = rows;
    if pane.entries.is_empty() {
        list::empty(frame, area, pane, caption);
        return;
    }
    pane.offset = scroll(pane.offset, pane.cursor / columns, rows);
    let query = pane.matching();

    let first = pane.offset * columns;
    for (index, entry) in pane.entries.iter().enumerate().skip(first) {
        // Past the bottom edge: nothing further down shows either.
        let Some(tile) = tile_rect(area, size, pane.offset, index) else {
            break;
        };
        let look = Look {
            cursor: index == pane.cursor,
            focused,
            marked: pane.is_marked(index),
            ..Look::default()
        };
        let fill = theme::row_fill(look.cursor, look.focused, look.marked);
        if let Some(showing) = clip(tile, area) {
            hits.add(
                showing,
                Spot::Item {
                    pane: pane_index,
                    index,
                },
            );
            frame.render_widget(Block::new().style(fill), showing);
            if look.cursor && focused && !theme::depth().reverses() {
                let edge = Rect {
                    width: 1,
                    ..showing
                };
                let bars = vec![Line::from("▎"); edge.height as usize];
                let style = fill.fg(theme::ink(theme::ACCENT));
                frame.render_widget(Paragraph::new(bars).style(style), edge);
            }
        }

        let thumb = thumb_of(tile, size);
        if let Some(visible) = thumb_rect(area, size, pane.offset, index) {
            if entry.is_dir {
                folder_card(frame, visible);
            } else if !picture(frame, app.thumbs.as_mut(), entry, thumb, visible) {
                stand_in(frame, thumb, visible, entry);
            }
        }

        let (ink, _) = inks(entry, look);
        let text = fill.fg(theme::ink(ink));
        let width = thumb.width as usize;
        let mut caption = caption_lines(&entry.name, width, CAPTION_LINES);
        // The tags follow the last line of the name, which makes room for them.
        let marks = Some(tag_width(entry.tags)).filter(|marks| marks + 2 <= width);
        if let (Some(marks), Some(last)) = (marks, caption.last_mut())
            && last.width() + marks > width
        {
            *last = fit(last, width - marks);
        }
        let lines = caption.len();
        for (line, words) in caption.into_iter().enumerate() {
            let row = Rect::new(thumb.x, thumb.bottom() + line as u16, thumb.width, 1);
            let Some(row) = clip(row, area) else { continue };
            let mut spans: Vec<Span> = bold_match(&words, &query, text);
            if marks.is_some() && line + 1 == lines {
                spans.extend(tag_marks(entry.tags, fill));
            }
            let line = Line::from(spans).alignment(Alignment::Center);
            frame.render_widget(Paragraph::new(line).style(fill), row);
        }
    }
}

/// A folder card or a file's stand-in box, and the muted marks on it.
fn card() -> Style {
    theme::on(theme::MUTED, theme::TAB)
}

/// A plain card with a folder's tab on its top left.
pub(super) fn folder_card(frame: &mut Frame, thumb: Rect) {
    let tab = "▄".repeat((thumb.width / 3) as usize);
    frame.render_widget(Paragraph::new(tab).style(theme::fg(theme::TAB)), thumb);
    let body = Rect {
        y: thumb.y + 1,
        height: thumb.height - 1,
        ..thumb
    };
    frame.render_widget(Block::new().style(card()), body);
}

/// What a file shows until it has a picture of its own: a box with the glyph
/// of its kind in the middle and its extension in the bottom right corner.
pub(super) fn stand_in(frame: &mut Frame, thumb: Rect, visible: Rect, entry: &Entry) {
    frame.render_widget(Block::new().style(card()), visible);
    let mut mark = |row: u16, text: String, alignment: Alignment, style: Style| {
        let line = Rect {
            y: thumb.y + row,
            height: 1,
            width: thumb.width.saturating_sub(1),
            ..thumb
        };
        if let Some(line) = clip(line, visible) {
            let text = Paragraph::new(text).style(style).alignment(alignment);
            frame.render_widget(text, line);
        }
    };
    let glyph = icons::of(entry);
    let tint = card().fg(theme::ink(icons::tint(entry)));
    mark(
        thumb.height.saturating_sub(1) / 2,
        glyph.to_string(),
        Alignment::Center,
        tint,
    );
    if let Some(extension) = model::extension(&entry.name) {
        let label = fit(&extension.to_uppercase(), thumb.width as usize - 2);
        mark(
            thumb.height.saturating_sub(1),
            label,
            Alignment::Right,
            card(),
        );
    }
}

/// Where a caption may break: after one of these, so a word is never split
/// when a break fits.
const BREAKS: [char; 4] = [' ', '-', '_', '.'];

/// A name as a caption of at most `count` lines of `per_line` cells:
/// each line breaks after the last space, `-`, `_` or `.` that leaves
/// the rest room to fit, the extension's own dot only when no other will
/// do; a name too long for all of them is cut in the middle with `…`, its
/// extension kept whole when it fits on a line.
pub fn caption_lines(name: &str, per_line: usize, count: usize) -> Vec<String> {
    let source: Vec<char> = name.chars().collect();
    if per_line == 0 || count == 0 || source.is_empty() {
        return vec![name.to_string()];
    }
    let mut capacity = per_line * count;
    if capacity > 1 && cells(&source) <= per_line {
        return vec![name.to_string()];
    }
    loop {
        let shown = elide(&source, capacity, per_line);
        let lines = wrap(&shown, per_line, count);
        let held: usize = lines.iter().map(|line| line.len()).sum();
        if capacity <= 1 || held == shown.len() {
            return lines.iter().map(|line| line.iter().collect()).collect();
        }
        // A wide character never straddles two lines: cut more until it fits.
        capacity -= 1;
    }
}

/// The cells some characters take.
fn cells(chars: &[char]) -> usize {
    chars.iter().map(|c| c.width().unwrap_or(0)).sum()
}

/// How many characters from the start fit in `budget` cells.
fn span_from(chars: &[char], budget: usize) -> usize {
    let mut used = 0;
    chars
        .iter()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= budget
        })
        .count()
}

/// How many characters at the end fit in `budget` cells.
fn tail_span(chars: &[char], budget: usize) -> usize {
    let mut used = 0;
    chars
        .iter()
        .rev()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= budget
        })
        .count()
}

/// The characters of the extension with its dot; none for a dotfile or a
/// name that ends in a dot.
fn extension_length(chars: &[char]) -> usize {
    match (1..chars.len()).rev().find(|&at| chars[at] == '.') {
        Some(dot) if dot != chars.len() - 1 => chars.len() - dot,
        _ => 0,
    }
}

/// The name in `capacity` cells: whole when it fits, else its start, `…`
/// and its end, the end keeping the extension when it fits on a line.
fn elide(chars: &[char], capacity: usize, per_line: usize) -> Vec<char> {
    if capacity <= 1 {
        return vec!['…'];
    }
    if cells(chars) <= capacity {
        return chars.to_vec();
    }
    let extension = extension_length(chars);
    let extension_cells = cells(&chars[chars.len() - extension..]);
    let (mut head, mut tail) = ((capacity - 1).div_ceil(2), (capacity - 1) / 2);
    if extension > 0 && extension_cells <= per_line && extension_cells < capacity {
        tail = tail.max(extension_cells);
        head = capacity - 1 - tail;
    }
    let head = span_from(chars, head);
    let tail = tail_span(chars, tail);
    let mut shown = chars[..head].to_vec();
    shown.push('…');
    shown.extend_from_slice(&chars[chars.len() - tail..]);
    shown
}

/// Up to `count` lines of `per_line` cells, each breaking after the last
/// separator that leaves the rest room to fit.
fn wrap(chars: &[char], per_line: usize, count: usize) -> Vec<Vec<char>> {
    let dot = chars.len() - extension_length(chars);
    let mut lines = Vec::new();
    let mut pos = 0;
    for line in 0..count {
        if pos >= chars.len() {
            break;
        }
        if cells(&chars[pos..]) <= per_line {
            lines.push(chars[pos..].to_vec());
            break;
        }
        let far = pos + span_from(&chars[pos..], per_line);
        if line == count - 1 {
            lines.push(chars[pos..far].to_vec());
            break;
        }
        let left = count - line;
        let mut cut = None;
        let mut kept_dot = None;
        for at in (pos + 1..far.max(pos + 1)).rev() {
            let rest_fits = cells(&chars[at + 1..]) <= (left - 1) * per_line;
            if !BREAKS.contains(&chars[at]) || !rest_fits {
                continue;
            }
            if at == dot {
                kept_dot = Some(at + 1);
                continue;
            }
            cut = Some(at + 1);
            break;
        }
        let cut = cut.or(kept_dot).unwrap_or(far.max(pos + 1));
        lines.push(chars[pos..cut].to_vec());
        pos = cut;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{alt, ctrl, fixture, grid_fixture, key, pictures, press, settle};
    use crate::icons;
    use crate::thumbs::testing::painted;
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    /// Ctrl+3 with the preview and the places panel hidden: the grid takes
    /// the whole body.
    fn whole_grid(app: &mut App) {
        app.handle_key(ctrl(KeyCode::Char('3')));
        app.handle_key(alt(KeyCode::Char('p')));
        app.handle_key(ctrl(KeyCode::Char('b')));
    }

    #[test]
    fn tiles_fill_the_width_and_say_where_their_thumbnails_go() {
        assert_eq!(columns(62, TileSize::Small), 3);
        assert_eq!(columns(62, TileSize::Normal), 3);
        assert_eq!(columns(62, TileSize::Large), 2);
        assert_eq!(columns(10, TileSize::Normal), 1, "never fewer than one");
        assert_eq!(visible_rows(22, TileSize::Normal), 2);
        assert_eq!(visible_rows(3, TileSize::Normal), 1);

        // The body of a 62 by 26 window: three tiles across, two whole rows
        // of a row of air, six of thumbnail and two of caption.
        let area = Rect::new(0, 2, 62, 22);
        let thumb = |offset, index| thumb_rect(area, TileSize::Normal, offset, index);
        assert_eq!(thumb(0, 0), Some(Rect::new(3, 3, 16, 6)));
        assert_eq!(thumb(0, 4), Some(Rect::new(23, 12, 16, 6)));
        assert_eq!(
            thumb(0, 6),
            Some(Rect::new(3, 21, 16, 3)),
            "the row the window cuts is cut"
        );
        assert_eq!(thumb(0, 9), None, "below the window");
        assert_eq!(thumb(1, 0), None, "scrolled away above");
        assert_eq!(thumb(1, 6), Some(Rect::new(3, 12, 16, 6)));
        assert_eq!(
            thumb_rect(area, TileSize::Large, 0, 1),
            Some(Rect::new(33, 3, 24, 9))
        );
    }

    #[test]
    fn a_caption_wraps_after_a_space_dash_underscore_or_dot_and_keeps_the_extension() {
        let lines = |name, width| caption_lines(name, width, 2);
        assert_eq!(lines("harbour-dusk.jpg", 16), ["harbour-dusk.jpg"]);
        assert_eq!(
            lines("summer holiday photos.jpg", 16),
            ["summer holiday ", "photos.jpg"]
        );
        // The extension's own dot loses to an earlier one.
        assert_eq!(lines("holiday.photo.jpg", 12), ["holiday.", "photo.jpg"]);
        assert_eq!(lines("plan_for-2027.md", 10), ["plan_for-", "2027.md"]);
        // No break that leaves the rest room: cut where the line is full.
        assert_eq!(lines("abcdefghijklmnop", 10), ["abcdefghij", "klmnop"]);
        // Too long for two lines: cut in the middle, the extension kept.
        assert_eq!(
            lines("report_2026_final_version_draft.pdf", 16),
            ["report_2026_fina", "…rsion_draft.pdf"]
        );
        let cut = lines("a-very-long-file-name-for-a-small-tile.txt", 6);
        assert_eq!(cut, ["a-very", "…e.txt"]);
        // Wide characters never straddle the lines.
        assert_eq!(lines("写真写真写真.png", 8), ["写真写真", "写真.png"]);
        assert_eq!(lines("写真写真写真.png", 7), ["写真写…", "真.png"]);
    }

    #[test]
    fn each_tile_shows_its_name_under_the_thumbnail_and_no_size_or_date() {
        let mut f = fixture();
        whole_grid(&mut f.app);
        let drawn = buffer(&mut f.app, 62, 26);
        // Three tiles of 18 cells at 2, 22 and 42; the caption under each
        // thumbnail, in the middle of its 16 cells.
        assert_eq!(find(&drawn, "Documents", 0..62), Some((7, 9)));
        assert_eq!(find(&drawn, "Pictures", 0..62), Some((27, 9)));
        assert_eq!(find(&drawn, "notes.txt", 0..62), Some((47, 9)));
        assert_eq!(drawn[(47, 9)].fg, theme::FOREGROUND);
        let grid = screen(&mut f.app, 62, 26)[2..24].join("\n");
        for gone in ["5 B", "items", "AM", "PM"] {
            assert!(!grid.contains(gone), "{gone} in\n{grid}");
        }
    }

    #[test]
    fn a_long_name_takes_two_lines_cut_in_the_middle_with_its_extension() {
        let mut f = fixture();
        fs::write(f.home.join("report_2026_final_version_draft.pdf"), "").unwrap();
        whole_grid(&mut f.app);
        press(&mut f.app, &[KeyCode::F(5)]);
        let drawn = buffer(&mut f.app, 62, 26);
        // Documents, Pictures, notes.txt, then the report on the second row.
        assert_eq!(find(&drawn, "report_2026_fina", 0..62), Some((3, 18)));
        assert_eq!(find(&drawn, "…rsion_draft.pdf", 0..62), Some((3, 19)));
    }

    #[test]
    fn folders_are_cards_with_a_tab_and_files_a_box_with_their_kind() {
        let mut f = fixture();
        whole_grid(&mut f.app);
        let buffer = buffer(&mut f.app, 62, 26);

        // Pictures: the tab on the top left, the card under it.
        assert_eq!(buffer[(23, 3)].symbol(), "▄");
        assert_eq!(buffer[(23, 3)].fg, theme::TAB);
        assert_eq!(buffer[(38, 3)].symbol(), " ");
        assert_eq!(buffer[(38, 3)].bg, Color::Reset);
        assert_eq!(buffer[(23, 4)].bg, theme::TAB);
        assert_eq!(buffer[(38, 8)].bg, theme::TAB);

        // notes.txt: a box, the glyph of its kind in the middle, the extension in a corner.
        assert_eq!(buffer[(43, 3)].bg, theme::TAB);
        assert_eq!(buffer[(58, 8)].bg, theme::TAB);
        assert_eq!(buffer[(50, 5)].symbol(), icons::TEXT);
        assert_eq!(buffer[(50, 5)].fg, theme::MUTED);
        assert_eq!(find(&buffer, "TXT", 0..62), Some((55, 8)));
        assert_eq!(buffer[(55, 8)].fg, theme::MUTED);
        assert_eq!(buffer[(55, 8)].bg, theme::TAB);
    }

    #[test]
    fn the_cursor_tile_has_the_cursor_fill_and_the_accent_edge_and_marked_tiles_the_marked_fill() {
        let mut f = fixture();
        whole_grid(&mut f.app);
        let lit = buffer(&mut f.app, 62, 26);
        // Documents: its 18 by 9 cells on the cursor fill, the edge down its left.
        for y in 2..11 {
            assert_eq!(lit[(2, y)].symbol(), "▎", "{y}");
            assert_eq!(lit[(2, y)].fg, theme::ACCENT);
            assert_eq!(lit[(2, y)].bg, theme::CURSOR);
            assert_eq!(lit[(19, y)].bg, theme::CURSOR);
        }
        assert_eq!(lit[(20, 2)].bg, Color::Reset);
        assert_eq!(lit[(7, 9)].fg, theme::FOREGROUND);
        assert_eq!(lit[(7, 9)].bg, theme::CURSOR);
        // The others have neither.
        assert_eq!(lit[(22, 2)].symbol(), " ");
        assert_eq!(lit[(22, 2)].bg, Color::Reset);
        assert!(!lit[(27, 9)].modifier.contains(Modifier::BOLD));

        // Shift+Right marks Documents and Pictures, the cursor on Pictures.
        f.app.handle_key(crate::app::testing::shift(KeyCode::Right));
        let marked = buffer(&mut f.app, 62, 26);
        assert_eq!(marked[(2, 2)].symbol(), " ");
        assert_eq!(marked[(2, 2)].bg, theme::MARKED);
        assert_eq!(marked[(6, 9)].bg, theme::MARKED);
        assert_eq!(marked[(22, 2)].symbol(), "▎");
        assert_eq!(marked[(22, 2)].bg, theme::CURSOR);
        assert_eq!(marked[(42, 2)].bg, Color::Reset, "notes.txt");

        // With the keyboard in the preview, the cursor's tile is on the
        // selected fill without the edge.
        f.app.handle_key(alt(KeyCode::Char('p')));
        press(&mut f.app, &[KeyCode::Esc]);
        crate::app::testing::click_preview(&mut f.app);
        let away = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&away, "Pictures", 0..73).unwrap();
        assert_eq!(away[(x, y)].bg, theme::SELECTED);
        assert!((0..73).all(|x| away[(x, 2)].symbol() != "▎"));
    }

    #[test]
    fn the_grid_scrolls_by_rows_to_keep_the_cursor_tile_whole() {
        let mut f = grid_fixture();
        press(&mut f.app, &[KeyCode::End]);
        let below = buffer(&mut f.app, 62, 26);
        assert_eq!(f.app.pane().offset, 2, "rows scrolled away");
        // The last tile is on the second row now, all of it.
        assert_eq!(below[(2, 11)].symbol(), "▎");
        assert_eq!(find(&below, "notes.txt", 0..62), Some((7, 18)));
        assert_eq!(find(&below, "a.txt", 0..62), None);
        assert!(find(&below, "e.txt", 0..62).is_some());

        press(&mut f.app, &[KeyCode::Up]);
        buffer(&mut f.app, 62, 26);
        assert_eq!(f.app.pane().offset, 2, "still in view: nothing moves");
        press(&mut f.app, &[KeyCode::Home]);
        let top = buffer(&mut f.app, 62, 26);
        assert_eq!(f.app.pane().offset, 0);
        assert!(find(&top, "a.txt", 0..62).is_some());
        assert_eq!(find(&top, "notes.txt", 0..62), None);
    }

    #[test]
    fn bigger_and_smaller_tiles_change_how_many_fit_across() {
        let mut f = grid_fixture();
        // The cursor's tile, as wide and tall as its fill goes.
        let tile = |app: &mut App| {
            let drawn = buffer(app, 72, 32);
            let (x, y) = find(&drawn, "▎", 0..72).unwrap();
            let wide = (x..72).take_while(|x| drawn[(*x, y)].bg == theme::CURSOR);
            let tall = (y..30).take_while(|y| drawn[(x, *y)].bg == theme::CURSOR);
            (wide.count(), tall.count())
        };
        assert_eq!(tile(&mut f.app), (18, 9));
        assert_eq!(f.app.pane().grid_columns, 3);
        f.app.handle_key(key(KeyCode::Char('+')));
        assert_eq!(tile(&mut f.app), (26, 12));
        assert_eq!(f.app.pane().grid_columns, 2);

        f.app.handle_key(key(KeyCode::Char('-')));
        f.app.handle_key(key(KeyCode::Char('-')));
        assert_eq!(tile(&mut f.app), (14, 7));
        assert_eq!(f.app.pane().grid_columns, 4);
    }

    #[test]
    fn a_file_with_a_picture_shows_it_in_the_middle_of_its_thumbnail() {
        let (mut f, thumbnailer) = pictures();
        fs::write(f.home.join("tall.jpg"), "tall").unwrap();
        whole_grid(&mut f.app);
        press(&mut f.app, &[KeyCode::F(5)]);

        // Drawing only asks: until the picture has been made the stand-in shows.
        let before = buffer(&mut f.app, 62, 26);
        assert_eq!(before[(50, 5)].symbol(), icons::TEXT);
        assert!(!painted(&before[(50, 5)]));
        settle(&mut f.app);
        let after = buffer(&mut f.app, 62, 26);

        // notes.txt, four by three: all sixteen by six cells of its thumbnail and no more.
        for (x, y) in [(43, 3), (58, 3), (50, 5), (43, 8), (58, 8)] {
            assert!(painted(&after[(x, y)]), "{x}, {y}");
        }
        assert_eq!(after[(50, 5)].symbol(), " ", "the stand-in is gone");
        for (x, y) in [(42, 3), (59, 3), (43, 2), (43, 9)] {
            assert!(!painted(&after[(x, y)]), "{x}, {y}");
        }
        // tall.jpg on the second row, three by four: nine cells wide in the middle of sixteen.
        for (x, y) in [(6, 12), (14, 12), (6, 17), (14, 17)] {
            assert!(painted(&after[(x, y)]), "{x}, {y}");
        }
        for (x, y) in [(3, 12), (5, 12), (15, 12), (18, 17), (6, 11), (6, 18)] {
            assert!(!painted(&after[(x, y)]), "{x}, {y}");
            assert_eq!(after[(x, y)].bg, Color::Reset);
        }
        // Folders keep their card and are never asked for.
        assert_eq!(after[(23, 3)].symbol(), "▄");
        assert_eq!(after[(23, 4)].bg, theme::TAB);
        let mut asked = thumbnailer.asked();
        asked.sort();
        assert_eq!(asked, ["notes.txt", "tall.jpg"]);
        let here = std::thread::current().id();
        let threads = thumbnailer.threads.lock().unwrap();
        assert!(
            threads.iter().all(|thread| *thread != here),
            "off this thread"
        );
    }

    #[test]
    fn a_thumbnail_the_bottom_edge_cuts_stays_inside_the_view() {
        let (mut f, _thumbnailer) = pictures();
        for name in ["a", "b", "c", "d", "e", "f", "g"] {
            fs::write(f.home.join(format!("{name}.txt")), name).unwrap();
        }
        whole_grid(&mut f.app);
        // What the detail column says goes in the footer, under the rule.
        press(&mut f.app, &[KeyCode::F(5), KeyCode::Char('I')]);
        buffer(&mut f.app, 62, 27);
        settle(&mut f.app);
        let cut = buffer(&mut f.app, 62, 27);

        // The third row of tiles: four rows of its thumbnails fit above the rule.
        let area = Rect::new(0, 2, 62, 23);
        assert_eq!(
            thumb_rect(area, TileSize::Normal, 0, 6),
            Some(Rect::new(3, 21, 16, 4))
        );
        for (x, y) in [(3, 21), (18, 21), (3, 24), (18, 24)] {
            assert!(painted(&cut[(x, y)]), "{x}, {y}");
        }
        for x in 0..62 {
            assert!(!painted(&cut[(x, 25)]), "the rule at {x} is drawn over");
            assert_eq!(cut[(x, 25)].symbol(), "─");
            // The grid, the whole window wide, has the keyboard: lit.
            assert_eq!(cut[(x, 25)].fg, theme::ACCENT);
        }
        assert!(find(&cut, "Detail column · Modified", 0..62).is_some_and(|(_, y)| y == 26));
    }

    #[test]
    fn a_file_that_cannot_be_drawn_keeps_its_stand_in_until_it_changes() {
        let (mut f, thumbnailer) = pictures();
        fs::write(f.home.join("notes.txt"), "broken").unwrap();
        whole_grid(&mut f.app);
        press(&mut f.app, &[KeyCode::F(5)]);
        for _ in 0..3 {
            buffer(&mut f.app, 62, 26);
            settle(&mut f.app);
        }
        let failed = buffer(&mut f.app, 62, 26);
        assert_eq!(failed[(50, 5)].symbol(), icons::TEXT);
        assert_eq!(failed[(43, 3)].bg, theme::TAB);
        assert_eq!(thumbnailer.asked(), ["notes.txt"], "asked once");

        // Written again later: it is worth another try.
        fs::write(f.home.join("notes.txt"), "mended").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        let file = File::options().write(true).open(f.home.join("notes.txt"));
        file.unwrap().set_modified(later).unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        buffer(&mut f.app, 62, 26);
        settle(&mut f.app);
        let mended = buffer(&mut f.app, 62, 26);
        assert!(painted(&mended[(50, 5)]));
        assert_eq!(thumbnailer.asked(), ["notes.txt", "notes.txt"]);
    }

    #[test]
    fn bigger_tiles_show_the_same_thumbnail_bigger() {
        let (mut f, thumbnailer) = pictures();
        whole_grid(&mut f.app);
        buffer(&mut f.app, 72, 32);
        settle(&mut f.app);
        let normal = buffer(&mut f.app, 72, 32);
        let area = Rect::new(0, 2, 72, 28);
        let thumb = thumb_rect(area, TileSize::Normal, 0, 2).unwrap();
        assert!(painted(&normal[(thumb.x, thumb.y)]));
        assert!(painted(&normal[(thumb.right() - 1, thumb.bottom() - 1)]));

        f.app.handle_key(key(KeyCode::Char('+')));
        buffer(&mut f.app, 72, 32);
        settle(&mut f.app);
        let large = buffer(&mut f.app, 72, 32);
        // Twenty-four cells by nine now, filled from edge to edge.
        let thumb = thumb_rect(area, TileSize::Large, 0, 2).unwrap();
        assert_eq!((thumb.width, thumb.height), (24, 9));
        for (x, y) in [
            (thumb.x, thumb.y),
            (thumb.right() - 1, thumb.y),
            (thumb.x, thumb.bottom() - 1),
            (thumb.right() - 1, thumb.bottom() - 1),
        ] {
            assert!(painted(&large[(x, y)]), "{x}, {y}");
        }
        assert!(!painted(&large[(thumb.x - 1, thumb.y)]));
        assert!(!painted(&large[(thumb.right(), thumb.y)]));
        assert_eq!(
            thumbnailer.asked(),
            ["notes.txt"],
            "the file is drawn once; only the fitting is done again"
        );
    }

    #[test]
    fn an_empty_folder_says_so_in_the_grid() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Enter]);
        let all = screen(&mut f.app, 100, 20).join("\n");
        assert!(all.contains("NOTHING HERE YET"), "{all}");
    }
}
