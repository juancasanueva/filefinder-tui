//! The grid view: roomy tiles with a thumbnail, the name, and the size and date.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::list::{self, size_text};
use super::{fit, picture, scroll};
use crate::app::{App, HitMap, Spot, TileSize};
use crate::format::{self, DateStyle};
use crate::icons;
use crate::model::{self, Entry};
use crate::theme;

/// The least room between two tiles, and between the tiles and the edges.
const GAP: u16 = 2;
/// A tile's rows besides the thumbnail: the frame above and below it, the name, size and date.
const EXTRA_ROWS: u16 = 4;

/// Cells across a tile: the thumbnail and one cell of frame on each side.
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
    let date_style = app.date_style;
    let caption = app.caption;
    let pane = &mut app.panes[pane_index];
    pane.grid_columns = columns;
    pane.page_rows = rows;
    if pane.entries.is_empty() {
        list::empty(frame, area, pane, caption);
        return;
    }
    pane.offset = scroll(pane.offset, pane.cursor / columns, rows);

    let now = SystemTime::now();
    let first = pane.offset * columns;
    for (index, entry) in pane.entries.iter().enumerate().skip(first) {
        // Past the bottom edge: nothing further down shows either.
        let Some(tile) = tile_rect(area, size, pane.offset, index) else {
            break;
        };
        if let Some(showing) = clip(tile, area) {
            let item = Spot::Item {
                pane: pane_index,
                index,
            };
            hits.add(showing, item);
        }
        let selected = pane.is_selected(index);

        let thumb = thumb_of(tile, size);
        if let Some(visible) = thumb_rect(area, size, pane.offset, index) {
            if entry.is_dir {
                folder_card(frame, visible);
            } else if !picture(frame, app.thumbs.as_mut(), entry, thumb, visible) {
                stand_in(frame, thumb, visible, entry);
            }
        }
        if selected {
            let around = Rect::new(tile.x, tile.y, tile.width, thumb.height + 2);
            if let Some(around) = clip(around, area) {
                let style = selected_tile(focused);
                frame.render_widget(Block::bordered().border_style(style), around);
            }
        }

        let name = if selected {
            theme::title()
        } else {
            theme::base()
        };
        let width = tile.width as usize;
        let under = thumb.bottom() + 1;
        centered(frame, area, tile, under, fit(&entry.name, width), name);
        let details = details(entry, now, date_style, width);
        centered(frame, area, tile, under + 1, details, theme::muted());
    }
}

/// The frame around a selected tile: the accent where the keyboard is,
/// muted elsewhere.
fn selected_tile(focused: bool) -> Style {
    theme::fg(if focused { theme::ACCENT } else { theme::MUTED })
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
    let mut mark = |row: u16, text: String, alignment: Alignment| {
        let line = Rect {
            y: thumb.y + row,
            height: 1,
            width: thumb.width.saturating_sub(1),
            ..thumb
        };
        if let Some(line) = clip(line, visible) {
            let text = Paragraph::new(text).style(card()).alignment(alignment);
            frame.render_widget(text, line);
        }
    };
    let glyph = icons::of(entry);
    mark(
        thumb.height.saturating_sub(1) / 2,
        glyph.to_string(),
        Alignment::Center,
    );
    if let Some(extension) = model::extension(&entry.name) {
        let label = fit(&extension.to_uppercase(), thumb.width as usize - 2);
        mark(thumb.height.saturating_sub(1), label, Alignment::Right);
    }
}

/// One line of text across a tile, in the middle, when its row is on screen.
fn centered(frame: &mut Frame, area: Rect, tile: Rect, y: u16, text: String, style: Style) {
    let line = Rect {
        y,
        height: 1,
        ..tile
    };
    if let Some(line) = clip(line, area) {
        let text = Paragraph::new(text)
            .style(style)
            .alignment(Alignment::Center);
        frame.render_widget(text, line);
    }
}

/// `27.1 KB · 6:10 PM`, or `3 items · 6:10 PM` for a folder; just the first
/// of the two on a tile too narrow for both.
fn details(entry: &Entry, now: SystemTime, date_style: DateStyle, width: usize) -> String {
    let date = entry
        .modified
        .map(|time| format::short_date(time, now, date_style))
        .unwrap_or_default();
    let mut parts = [size_text(entry), date]
        .into_iter()
        .filter(|part| !part.is_empty());
    let first = parts.next().unwrap_or_default();
    let both = match parts.next() {
        Some(second) => format!("{first} · {second}"),
        None => first.clone(),
    };
    if both.width() <= width {
        both
    } else {
        fit(&first, width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{ctrl, fixture, grid_fixture, key, pictures, press, settle, split};
    use crate::icons;
    use crate::thumbs::testing::painted;
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use std::fs::{self, File};
    use std::time::Duration;

    #[test]
    fn tiles_fill_the_width_and_say_where_their_thumbnails_go() {
        assert_eq!(columns(62, TileSize::Small), 3);
        assert_eq!(columns(62, TileSize::Normal), 3);
        assert_eq!(columns(62, TileSize::Large), 2);
        assert_eq!(columns(10, TileSize::Normal), 1, "never fewer than one");
        assert_eq!(visible_rows(22, TileSize::Normal), 2);
        assert_eq!(visible_rows(3, TileSize::Normal), 1);

        // The files area of a 120 by 24 window: three tiles across, two whole rows.
        let area = Rect::new(0, 2, 62, 22);
        let thumb = |offset, index| thumb_rect(area, TileSize::Normal, offset, index);
        assert_eq!(thumb(0, 0), Some(Rect::new(3, 3, 16, 6)));
        assert_eq!(thumb(0, 4), Some(Rect::new(23, 13, 16, 6)));
        assert_eq!(
            thumb(0, 6),
            Some(Rect::new(3, 23, 16, 1)),
            "the row the window cuts is cut"
        );
        assert_eq!(thumb(0, 9), None, "below the window");
        assert_eq!(thumb(1, 0), None, "scrolled away above");
        assert_eq!(thumb(1, 6), Some(Rect::new(3, 13, 16, 6)));
        assert_eq!(
            thumb_rect(area, TileSize::Large, 0, 1),
            Some(Rect::new(33, 3, 24, 9))
        );
    }

    #[test]
    fn each_tile_shows_the_name_centered_over_its_size_and_date() {
        let mut f = fixture();
        // A 24-hour time is never wider than five cells, so the line fits at any hour.
        f.app.date_style = crate::format::DateStyle::DayFirst;
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let buffer = buffer(&mut f.app, 62, 26);
        let lines = screen(&mut f.app, 62, 26);

        // Three tiles of 18 cells starting at 26, 46 and 66; names on the row under the thumbnails.
        assert_eq!(find(&buffer, "Documents", 0..62), Some((7, 10)));
        assert_eq!(find(&buffer, "Pictures", 0..62), Some((27, 10)));
        assert_eq!(find(&buffer, "notes.txt", 0..62), Some((47, 10)));

        let notes = &f.app.pane().entries[2];
        let date = format::short_date(notes.modified.unwrap(), SystemTime::now(), f.app.date_style);
        let under = &lines[11];
        assert!(under.contains(&format!("No items · {date}")), "{under}");
        assert!(under.contains(&format!("2 items · {date}")), "{under}");
        assert!(under.contains(&format!("5 B · {date}")), "{under}");
        let (x, y) = find(&buffer, "5 B", 0..62).unwrap();
        assert_eq!(buffer[(x, y)].fg, theme::MUTED);
    }

    #[test]
    fn folders_are_cards_with_a_tab_and_files_a_box_with_their_kind() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let buffer = buffer(&mut f.app, 62, 26);

        // Documents: the tab on the top left, the card under it.
        assert_eq!(buffer[(3, 3)].symbol(), "▄");
        assert_eq!(buffer[(3, 3)].fg, theme::TAB);
        assert_eq!(buffer[(18, 3)].symbol(), " ");
        assert_eq!(buffer[(18, 3)].bg, Color::Reset);
        assert_eq!(buffer[(3, 4)].bg, theme::TAB);
        assert_eq!(buffer[(18, 8)].bg, theme::TAB);

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
    fn the_selected_tile_wears_the_color_itself() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        let lit = buffer(&mut f.app, 62, 26);
        // A frame in the accent around the thumbnail and the name bold.
        assert_eq!(lit[(2, 2)].symbol(), "┌");
        assert_eq!(lit[(19, 9)].symbol(), "┘");
        assert_eq!(lit[(2, 2)].fg, theme::ACCENT);
        assert!(lit[(7, 10)].modifier.contains(Modifier::BOLD));
        // The others have neither.
        assert_eq!(lit[(22, 2)].symbol(), " ");
        assert_eq!(lit[(27, 10)].fg, theme::FOREGROUND);
        assert!(!lit[(27, 10)].modifier.contains(Modifier::BOLD));

        // With the keyboard in the other pane the frame is muted.
        split(&mut f.app);
        let unlit = buffer(&mut f.app, 124, 26);
        let tile = tile_rect(LEFT, TileSize::Normal, 0, 0).unwrap();
        assert_eq!(unlit[(tile.x, tile.y)].symbol(), "┌");
        assert_eq!(unlit[(tile.x, tile.y)].fg, theme::MUTED);
    }

    #[test]
    fn a_name_too_long_for_its_tile_ends_in_an_ellipsis() {
        let mut f = fixture();
        fs::write(f.home.join("a-very-long-file-name-indeed.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::F(5)]);
        let all = screen(&mut f.app, 62, 26).join("\n");
        assert!(all.contains("a-very-long-file-…"), "{all}");
        assert!(!all.contains("a-very-long-file-n"), "{all}");
    }

    #[test]
    fn the_grid_scrolls_by_rows_to_keep_the_selected_tile_whole() {
        let mut f = grid_fixture();
        press(&mut f.app, &[KeyCode::End]);
        let below = buffer(&mut f.app, 62, 26);
        assert_eq!(f.app.pane().offset, 2, "rows scrolled away");
        // The last tile is on the second row now, all of it: frame, name and details.
        assert_eq!(below[(2, 12)].symbol(), "┌");
        assert_eq!(find(&below, "notes.txt", 0..62), Some((7, 20)));
        assert!(find(&below, "5 B", 0..62).is_some_and(|(_, y)| y == 21));
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
        screen(&mut f.app, 72, 32);
        assert_eq!(f.app.pane().grid_columns, 3);
        f.app.handle_key(key(KeyCode::Char('+')));
        let big = buffer(&mut f.app, 72, 32);
        assert_eq!(f.app.pane().grid_columns, 2);
        // A 26-cell tile: the frame's corners are that far apart, around a 9-row thumbnail.
        let (x, y) = find(&big, "┌", 0..72).unwrap();
        assert_eq!(big[(x + 25, y)].symbol(), "┐");
        assert_eq!(big[(x, y + 10)].symbol(), "└");

        f.app.handle_key(key(KeyCode::Char('-')));
        f.app.handle_key(key(KeyCode::Char('-')));
        let small = buffer(&mut f.app, 72, 32);
        assert_eq!(f.app.pane().grid_columns, 4);
        let (x, y) = find(&small, "┌", 0..72).unwrap();
        assert_eq!(small[(x + 13, y)].symbol(), "┐");
        assert_eq!(small[(x, y + 5)].symbol(), "└");
    }

    #[test]
    fn a_file_with_a_picture_shows_it_in_the_middle_of_its_thumbnail() {
        let (mut f, thumbnailer) = pictures();
        fs::write(f.home.join("tall.jpg"), "tall").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
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
        for (x, y) in [(6, 13), (14, 13), (6, 18), (14, 18)] {
            assert!(painted(&after[(x, y)]), "{x}, {y}");
        }
        for (x, y) in [(3, 13), (5, 13), (15, 13), (18, 18), (6, 12), (6, 19)] {
            assert!(!painted(&after[(x, y)]), "{x}, {y}");
            assert_eq!(after[(x, y)].bg, Color::Reset);
        }
        // Folders keep their card and are never asked for.
        assert_eq!(after[(3, 3)].symbol(), "▄");
        assert_eq!(after[(3, 4)].bg, theme::TAB);
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
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        // What the detail column says goes in the footer, under the rule.
        press(&mut f.app, &[KeyCode::F(5), KeyCode::Char('I')]);
        buffer(&mut f.app, 62, 27);
        settle(&mut f.app);
        let cut = buffer(&mut f.app, 62, 27);

        // The third row of tiles: two rows of its thumbnails fit above the rule.
        let area = Rect::new(0, 2, 62, 23);
        assert_eq!(
            thumb_rect(area, TileSize::Normal, 0, 6),
            Some(Rect::new(3, 23, 16, 2))
        );
        for (x, y) in [(3, 23), (18, 23), (3, 24), (18, 24)] {
            assert!(painted(&cut[(x, y)]), "{x}, {y}");
        }
        for x in 0..62 {
            assert!(!painted(&cut[(x, 25)]), "the rule at {x} is drawn over");
            assert_eq!(cut[(x, 25)].symbol(), "─");
            assert_eq!(cut[(x, 25)].fg, theme::BORDER);
        }
        assert!(find(&cut, "Detail column · Modified", 0..62).is_some_and(|(_, y)| y == 26));
    }

    #[test]
    fn a_file_that_cannot_be_drawn_keeps_its_stand_in_until_it_changes() {
        let (mut f, thumbnailer) = pictures();
        fs::write(f.home.join("notes.txt"), "broken").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
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
        f.app.handle_key(ctrl(KeyCode::Char('3')));
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
        assert!(all.contains("No items"), "{all}");
    }

    // Two panes in a window 182 cells wide: 124 of them for the files, 62 on
    // the left, a hairline, 61 on the right, each under a row for its path.
    const LEFT: Rect = Rect::new(0, 3, 62, 21);
    const RIGHT: Rect = Rect::new(63, 3, 61, 21);

    #[test]
    fn two_grids_each_have_their_own_tiles_across_and_their_own_selected_tile() {
        let mut f = grid_fixture();
        split(&mut f.app);
        buffer(&mut f.app, 124, 26);
        let across = |app: &App| (app.panes[0].grid_columns, app.panes[1].grid_columns);
        assert_eq!(across(&f.app), (3, 2));
        assert_eq!((f.app.panes[0].page_rows, f.app.panes[1].page_rows), (2, 2));

        // Down is a row of two on the right and a row of three on the left.
        press(&mut f.app, &[KeyCode::Down]);
        assert_eq!(f.app.pane().cursor, 2);
        press(&mut f.app, &[KeyCode::BackTab, KeyCode::Down]);
        assert_eq!(f.app.pane().cursor, 3);
        let drawn = buffer(&mut f.app, 124, 26);
        let here = tile_rect(LEFT, TileSize::Normal, 0, 3).unwrap();
        let there = tile_rect(RIGHT, TileSize::Normal, 0, 2).unwrap();
        assert_eq!((here.y, there.y), (13, 13), "the second row of each");
        assert_eq!(drawn[(here.x, here.y)].symbol(), "┌");
        assert_eq!(drawn[(here.x, here.y)].fg, theme::ACCENT);
        assert_eq!(drawn[(there.x, there.y)].symbol(), "┌");
        assert_eq!(drawn[(there.x, there.y)].fg, theme::MUTED);
        // One frame each, and a hairline between the two grids.
        let corners = (0..124u16)
            .flat_map(|x| (0..26u16).map(move |y| (x, y)))
            .filter(|at| drawn[*at].symbol() == "┌");
        assert_eq!(corners.count(), 2);
        assert_eq!(drawn[(62, 2)].symbol(), "│");
        assert_eq!(drawn[(62, 23)].symbol(), "│");
        // The path is on the row above the tiles.
        assert_eq!(drawn[(1, 2)].symbol(), "~");
        assert_eq!(drawn[(64, 2)].symbol(), "~");
    }

    #[test]
    fn each_pane_draws_the_thumbnails_of_its_own_folder() {
        let (mut f, thumbnailer) = pictures();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        split(&mut f.app);
        press(&mut f.app, &[KeyCode::Right, KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        buffer(&mut f.app, 124, 26);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 124, 26);

        // notes.txt, the third tile on the left; a.jpg, the second on the right.
        let notes = thumb_rect(LEFT, TileSize::Normal, 0, 2).unwrap();
        let picture = thumb_rect(RIGHT, TileSize::Normal, 0, 1).unwrap();
        assert!(notes.right() <= 62 && picture.x >= 63);
        for thumb in [notes, picture] {
            assert_eq!((thumb.width, thumb.height), (16, 6));
            for (x, y) in [
                (thumb.x, thumb.y),
                (thumb.right() - 1, thumb.y),
                (thumb.x, thumb.bottom() - 1),
                (thumb.right() - 1, thumb.bottom() - 1),
            ] {
                assert!(painted(&drawn[(x, y)]), "{x}, {y}");
            }
            assert!(!painted(&drawn[(thumb.x - 1, thumb.y)]));
        }
        let mut asked = thumbnailer.asked();
        asked.sort();
        assert_eq!(asked, ["a.jpg", "notes.txt"]);
    }
}
