//! The grid view: roomy tiles with a thumbnail, the name, and the size and date.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::list::{self, size_text};
use super::{fit, picture, scroll};
use crate::app::{App, TileSize};
use crate::format::{self, DateStyle};
use crate::icons::Icon;
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

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let size = app.tile;
    let columns = columns(area.width, size);
    let rows = visible_rows(area.height, size);
    app.grid_columns = columns;
    app.page_rows = rows;
    let date_style = app.date_style;
    let pane = &mut app.panes[app.active];
    if pane.entries.is_empty() {
        list::empty(frame, area);
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
        let selected = index == pane.cursor;

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
                let style = theme::selected_tile(focused);
                frame.render_widget(Block::bordered().border_style(style), around);
            }
        }

        let name = if selected {
            theme::selected_tile(focused)
        } else {
            theme::base()
        };
        let width = tile.width as usize;
        let under = thumb.bottom() + 1;
        centered(frame, area, tile, under, fit(&entry.name, width), name);
        let details = details(entry, now, date_style, width);
        centered(frame, area, tile, under + 1, details, theme::dim());
    }
}

/// A plain card with a folder's tab on its top left.
pub(super) fn folder_card(frame: &mut Frame, thumb: Rect) {
    let tab = "▄".repeat((thumb.width / 3) as usize);
    frame.render_widget(Paragraph::new(tab).style(theme::card_tab()), thumb);
    let card = Rect {
        y: thumb.y + 1,
        height: thumb.height - 1,
        ..thumb
    };
    frame.render_widget(Block::new().style(theme::card()), card);
}

/// What a file shows until it has a picture of its own: a box with the glyph
/// of its kind in the middle and its extension in the bottom right corner.
pub(super) fn stand_in(frame: &mut Frame, thumb: Rect, visible: Rect, entry: &Entry) {
    frame.render_widget(Block::new().style(theme::card()), visible);
    let mut mark = |row: u16, text: String, alignment: Alignment| {
        let line = Rect {
            y: thumb.y + row,
            height: 1,
            width: thumb.width.saturating_sub(1),
            ..thumb
        };
        if let Some(line) = clip(line, visible) {
            let text = Paragraph::new(text)
                .style(theme::card())
                .alignment(alignment);
            frame.render_widget(text, line);
        }
    };
    let glyph = Icon::for_category(entry.category()).glyph();
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
    use crate::app::testing::{ctrl, fixture, grid_fixture, pictures, press, settle};
    use crate::icons::Icon;
    use crate::thumbs::testing::painted;
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::crossterm::event::KeyCode;
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
        let area = Rect::new(24, 2, 62, 22);
        let thumb = |offset, index| thumb_rect(area, TileSize::Normal, offset, index);
        assert_eq!(thumb(0, 0), Some(Rect::new(27, 3, 16, 6)));
        assert_eq!(thumb(0, 4), Some(Rect::new(47, 13, 16, 6)));
        assert_eq!(
            thumb(0, 6),
            Some(Rect::new(27, 23, 16, 1)),
            "the row the window cuts is cut"
        );
        assert_eq!(thumb(0, 9), None, "below the window");
        assert_eq!(thumb(1, 0), None, "scrolled away above");
        assert_eq!(thumb(1, 6), Some(Rect::new(27, 13, 16, 6)));
        assert_eq!(
            thumb_rect(area, TileSize::Large, 0, 1),
            Some(Rect::new(57, 3, 24, 9))
        );
    }

    #[test]
    fn each_tile_shows_the_name_centered_over_its_size_and_date() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let buffer = buffer(&mut f.app, 120, 24);
        let lines = screen(&mut f.app, 120, 24);

        // Three tiles of 18 cells starting at 26, 46 and 66; names on the row under the thumbnails.
        assert_eq!(find(&buffer, "Documents", 24..86), Some((31, 10)));
        assert_eq!(find(&buffer, "Pictures", 24..86), Some((51, 10)));
        assert_eq!(find(&buffer, "notes.txt", 24..86), Some((71, 10)));

        let notes = &f.app.pane().entries[2];
        let date = format::short_date(notes.modified.unwrap(), SystemTime::now(), f.app.date_style);
        let under = &lines[11];
        assert!(under.contains(&format!("No items · {date}")), "{under}");
        assert!(under.contains(&format!("2 items · {date}")), "{under}");
        assert!(under.contains(&format!("5 B · {date}")), "{under}");
        let (x, y) = find(&buffer, "5 B", 24..86).unwrap();
        assert_eq!(buffer[(x, y)].fg, theme::DIM);
    }

    #[test]
    fn folders_are_cards_with_a_tab_and_files_a_box_with_their_kind() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let buffer = buffer(&mut f.app, 120, 24);

        // Documents: the tab on the top left, the card under it.
        assert_eq!(buffer[(27, 3)].symbol(), "▄");
        assert_eq!(buffer[(27, 3)].fg, theme::LIGHTER_BACKGROUND);
        assert_eq!(buffer[(42, 3)].symbol(), " ");
        assert_eq!(buffer[(42, 3)].bg, theme::BACKGROUND);
        assert_eq!(buffer[(27, 4)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(buffer[(42, 8)].bg, theme::LIGHTER_BACKGROUND);

        // notes.txt: a box, the glyph of its kind in the middle, the extension in a corner.
        assert_eq!(buffer[(67, 3)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(buffer[(82, 8)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(buffer[(74, 5)].symbol(), Icon::Text.glyph());
        assert_eq!(buffer[(74, 5)].fg, theme::DIM);
        assert_eq!(find(&buffer, "TXT", 24..86), Some((79, 8)));
        assert_eq!(buffer[(79, 8)].fg, theme::DIM);
        assert_eq!(buffer[(79, 8)].bg, theme::LIGHTER_BACKGROUND);
    }

    #[test]
    fn the_selected_tile_wears_the_color_itself() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        let lit = buffer(&mut f.app, 120, 24);
        // An accent frame around the thumbnail and the name in accent.
        assert_eq!(lit[(26, 2)].symbol(), "┌");
        assert_eq!(lit[(43, 9)].symbol(), "┘");
        assert_eq!(lit[(26, 2)].fg, theme::ACCENT);
        assert_eq!(lit[(31, 10)].fg, theme::ACCENT);
        // The others have neither.
        assert_eq!(lit[(46, 2)].symbol(), " ");
        assert_eq!(lit[(51, 10)].fg, theme::FOREGROUND);

        // With the keyboard in the details pane the selection goes grey.
        press(&mut f.app, &[KeyCode::Tab]);
        let unlit = buffer(&mut f.app, 120, 24);
        assert_eq!(unlit[(26, 2)].symbol(), "┌");
        assert_eq!(unlit[(26, 2)].fg, theme::BRIGHT_FOREGROUND);
        assert_eq!(unlit[(31, 10)].fg, theme::BRIGHT_FOREGROUND);
    }

    #[test]
    fn a_name_too_long_for_its_tile_ends_in_an_ellipsis() {
        let mut f = fixture();
        fs::write(f.home.join("a-very-long-file-name-indeed.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        press(&mut f.app, &[KeyCode::F(5)]);
        let all = screen(&mut f.app, 120, 24).join("\n");
        assert!(all.contains("a-very-long-file-…"), "{all}");
        assert!(!all.contains("a-very-long-file-n"), "{all}");
    }

    #[test]
    fn the_grid_scrolls_by_rows_to_keep_the_selected_tile_whole() {
        let mut f = grid_fixture();
        press(&mut f.app, &[KeyCode::End]);
        let below = buffer(&mut f.app, 120, 24);
        assert_eq!(f.app.pane().offset, 2, "rows scrolled away");
        // The last tile is on the second row now, all of it: frame, name and details.
        assert_eq!(below[(26, 12)].symbol(), "┌");
        assert_eq!(find(&below, "notes.txt", 24..86), Some((31, 20)));
        assert!(find(&below, "5 B", 24..86).is_some_and(|(_, y)| y == 21));
        assert_eq!(find(&below, "a.txt", 24..86), None);
        assert!(find(&below, "e.txt", 24..86).is_some());

        press(&mut f.app, &[KeyCode::Up]);
        buffer(&mut f.app, 120, 24);
        assert_eq!(f.app.pane().offset, 2, "still in view: nothing moves");
        press(&mut f.app, &[KeyCode::Home]);
        let top = buffer(&mut f.app, 120, 24);
        assert_eq!(f.app.pane().offset, 0);
        assert!(find(&top, "a.txt", 24..86).is_some());
        assert_eq!(find(&top, "notes.txt", 24..86), None);
    }

    #[test]
    fn bigger_and_smaller_tiles_change_how_many_fit_across() {
        let mut f = grid_fixture();
        screen(&mut f.app, 130, 30);
        assert_eq!(f.app.grid_columns, 3);
        f.app.handle_key(ctrl(KeyCode::Char('=')));
        let big = buffer(&mut f.app, 130, 30);
        assert_eq!(f.app.grid_columns, 2);
        // A 26-cell tile: the frame's corners are that far apart, around a 9-row thumbnail.
        let (x, y) = find(&big, "┌", 24..96).unwrap();
        assert_eq!(big[(x + 25, y)].symbol(), "┐");
        assert_eq!(big[(x, y + 10)].symbol(), "└");

        f.app.handle_key(ctrl(KeyCode::Char('-')));
        f.app.handle_key(ctrl(KeyCode::Char('-')));
        let small = buffer(&mut f.app, 130, 30);
        assert_eq!(f.app.grid_columns, 4);
        let (x, y) = find(&small, "┌", 24..96).unwrap();
        assert_eq!(small[(x + 13, y)].symbol(), "┐");
        assert_eq!(small[(x, y + 5)].symbol(), "└");
    }

    #[test]
    fn a_file_with_a_picture_shows_it_in_the_middle_of_its_thumbnail() {
        let (mut f, thumbnailer) = pictures();
        fs::write(f.home.join("tall.jpg"), "tall").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        press(&mut f.app, &[KeyCode::F(5)]);

        // Drawing only asks: until the picture has been made the stand-in shows.
        let before = buffer(&mut f.app, 120, 24);
        assert_eq!(before[(74, 5)].symbol(), Icon::Text.glyph());
        assert!(!painted(&before[(74, 5)]));
        settle(&mut f.app);
        let after = buffer(&mut f.app, 120, 24);

        // notes.txt, four by three: all sixteen by six cells of its thumbnail and no more.
        for (x, y) in [(67, 3), (82, 3), (74, 5), (67, 8), (82, 8)] {
            assert!(painted(&after[(x, y)]), "{x}, {y}");
        }
        assert_eq!(after[(74, 5)].symbol(), " ", "the stand-in is gone");
        for (x, y) in [(66, 3), (83, 3), (67, 2), (67, 9)] {
            assert!(!painted(&after[(x, y)]), "{x}, {y}");
        }
        // tall.jpg on the second row, three by four: nine cells wide in the middle of sixteen.
        for (x, y) in [(30, 13), (38, 13), (30, 18), (38, 18)] {
            assert!(painted(&after[(x, y)]), "{x}, {y}");
        }
        for (x, y) in [(27, 13), (29, 13), (39, 13), (42, 18), (30, 12), (30, 19)] {
            assert!(!painted(&after[(x, y)]), "{x}, {y}");
            assert_eq!(after[(x, y)].bg, theme::BACKGROUND);
        }
        // Folders keep their card and are never asked for.
        assert_eq!(after[(27, 3)].symbol(), "▄");
        assert_eq!(after[(27, 4)].bg, theme::LIGHTER_BACKGROUND);
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
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        // Sorting says so in a band under the view: the view is one row shorter.
        press(&mut f.app, &[KeyCode::Char('s'), KeyCode::Char('S')]);
        buffer(&mut f.app, 120, 26);
        settle(&mut f.app);
        let cut = buffer(&mut f.app, 120, 26);

        // The third row of tiles: two rows of its thumbnails fit above the band.
        let area = Rect::new(24, 2, 62, 23);
        assert_eq!(
            thumb_rect(area, TileSize::Normal, 0, 6),
            Some(Rect::new(27, 23, 16, 2))
        );
        for (x, y) in [(27, 23), (42, 23), (27, 24), (42, 24)] {
            assert!(painted(&cut[(x, y)]), "{x}, {y}");
        }
        for x in 24..86 {
            assert!(!painted(&cut[(x, 25)]), "the band at {x} is drawn over");
            assert_eq!(cut[(x, 25)].bg, theme::LIGHTER_BACKGROUND);
        }
        assert_eq!(find(&cut, "Sorted by name", 24..86), Some((26, 25)));
    }

    #[test]
    fn a_file_that_cannot_be_drawn_keeps_its_stand_in_until_it_changes() {
        let (mut f, thumbnailer) = pictures();
        fs::write(f.home.join("notes.txt"), "broken").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        press(&mut f.app, &[KeyCode::F(5)]);
        for _ in 0..3 {
            buffer(&mut f.app, 120, 24);
            settle(&mut f.app);
        }
        let failed = buffer(&mut f.app, 120, 24);
        assert_eq!(failed[(74, 5)].symbol(), Icon::Text.glyph());
        assert_eq!(failed[(67, 3)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(thumbnailer.asked(), ["notes.txt"], "asked once");

        // Written again later: it is worth another try.
        fs::write(f.home.join("notes.txt"), "mended").unwrap();
        let later = SystemTime::now() + Duration::from_secs(60);
        let file = File::options().write(true).open(f.home.join("notes.txt"));
        file.unwrap().set_modified(later).unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        buffer(&mut f.app, 120, 24);
        settle(&mut f.app);
        let mended = buffer(&mut f.app, 120, 24);
        assert!(painted(&mended[(74, 5)]));
        assert_eq!(thumbnailer.asked(), ["notes.txt", "notes.txt"]);
    }

    #[test]
    fn bigger_tiles_show_the_same_thumbnail_bigger() {
        let (mut f, thumbnailer) = pictures();
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        buffer(&mut f.app, 130, 30);
        settle(&mut f.app);
        let normal = buffer(&mut f.app, 130, 30);
        let area = Rect::new(24, 2, 72, 28);
        let thumb = thumb_rect(area, TileSize::Normal, 0, 2).unwrap();
        assert!(painted(&normal[(thumb.x, thumb.y)]));
        assert!(painted(&normal[(thumb.right() - 1, thumb.bottom() - 1)]));

        f.app.handle_key(ctrl(KeyCode::Char('=')));
        buffer(&mut f.app, 130, 30);
        settle(&mut f.app);
        let large = buffer(&mut f.app, 130, 30);
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
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        press(&mut f.app, &[KeyCode::Enter]);
        let all = screen(&mut f.app, 100, 20).join("\n");
        assert!(all.contains("No items"), "{all}");
    }
}
