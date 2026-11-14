//! The columns view: Finder-style, one column for each folder on the way down.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::list::{self, Look};
use super::{COLUMN_PITCH as PITCH, COLUMN_WIDTH};
use super::{empty_text, fit, scroll};
use crate::app::{App, HitMap, Spot};
use crate::model::Entry;
use crate::theme;

/// Draws one pane's folder and the ones above it; `focused` is whether it
/// has the keyboard. Each row is a spot for the mouse.
pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    index: usize,
    area: Rect,
    focused: bool,
    hits: &mut HitMap,
) {
    // One blank row of air on top.
    let rows = area.height.saturating_sub(1) as usize;
    let caption = app.caption;
    let pane = &mut app.panes[index];
    pane.offset = scroll(pane.offset, pane.cursor, rows);

    // The folders above, then the pane's own; the rightmost ones when they don't all fit.
    let total = pane.ancestors.len() + 1;
    let shown = ((area.width / PITCH) as usize).clamp(1, total);
    for slot in 0..shown {
        let x = area.x + slot as u16 * PITCH;
        let column = Rect {
            x,
            width: PITCH.min(area.right().saturating_sub(x)),
            ..area
        };
        let hairline = Block::new()
            .borders(Borders::RIGHT)
            .border_style(theme::border());
        let inner = Rect {
            width: column.width.min(COLUMN_WIDTH),
            ..column
        };
        if column.width == PITCH {
            frame.render_widget(hairline, column);
        }
        let depth = total - shown + slot;
        match pane.ancestors.get(depth) {
            Some(above) => {
                // The way down is on the selected fill: the keyboard is further right.
                let offset = scroll(0, above.on_path.unwrap_or(0), rows);
                let look = |index| Look {
                    cursor: above.on_path == Some(index),
                    parent: true,
                    ..Look::default()
                };
                draw_column(frame, inner, &above.entries, offset, look);
                let spot = |row| Spot::Above {
                    pane: index,
                    depth,
                    index: row,
                };
                add_rows(hits, inner, above.entries.len(), offset, spot);
            }
            None if pane.entries.is_empty() => {
                // The pane's own column is the last one: there is room to
                // its right for what it has to say.
                let room = Rect {
                    width: area.right().saturating_sub(inner.x),
                    ..inner
                };
                let said = fit(
                    &format!("  {}", empty_text(pane, caption)),
                    room.width as usize,
                );
                let text = vec![Line::default(), Line::from(said)];
                frame.render_widget(Paragraph::new(text).style(theme::muted()), room);
            }
            None => {
                let look = |row| Look {
                    cursor: row == pane.cursor,
                    focused,
                    marked: pane.is_marked(row),
                    ..Look::default()
                };
                draw_column(frame, inner, &pane.entries, pane.offset, look);
                let spot = |row| Spot::Item {
                    pane: index,
                    index: row,
                };
                add_rows(hits, inner, pane.entries.len(), pane.offset, spot);
            }
        }
    }
}

/// Where `draw_column` drew each of a folder's rows, for the mouse.
fn add_rows(
    hits: &mut HitMap,
    area: Rect,
    count: usize,
    offset: usize,
    spot: impl Fn(usize) -> Spot,
) {
    let rows = area.height.saturating_sub(1) as usize;
    for (line, row) in (offset..count).take(rows).enumerate() {
        let y = area.y + 1 + line as u16;
        hits.add(Rect::new(area.x, y, area.width, 1), spot(row));
    }
}

/// One folder's rows from `offset` down, each as `look` says.
fn draw_column(
    frame: &mut Frame,
    area: Rect,
    entries: &[Entry],
    offset: usize,
    look: impl Fn(usize) -> Look,
) {
    let rows = area.height.saturating_sub(1) as usize;
    let mut lines = vec![Line::default()];
    for (index, entry) in entries.iter().enumerate().skip(offset).take(rows) {
        lines.push(list::row(
            entry,
            area.width as usize,
            None,
            look(index),
            "",
            None,
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{ctrl, fixture, press};
    use crate::icons;
    use crate::theme;
    use crate::ui::testing::{buffer, find};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;

    /// The cells of one screen row between two columns, as text.
    fn cells(buffer: &ratatui::buffer::Buffer, y: u16, columns: std::ops::Range<u16>) -> String {
        columns.map(|x| buffer[(x, y)].symbol()).collect()
    }

    #[test]
    fn columns_run_from_home_down_to_the_folder_with_the_way_there_on_the_selected_fill() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        let lit = buffer(&mut f.app, 72, 26);

        // Home, 26 cells wide, then a line.
        assert_eq!(find(&lit, "Documents", 0..72), Some((3, 3)));
        assert_eq!(
            find(&lit, "Pictures", 0..26).map(|(x, y)| (x, y > 2)),
            Some((3, false))
        );
        assert_eq!(cells(&lit, 4, 3..11), "Pictures");
        assert_eq!(find(&lit, "notes.txt", 0..72), Some((3, 5)));
        assert_eq!(lit[(26, 3)].symbol(), "│");
        assert_eq!(lit[(26, 23)].symbol(), "│");
        assert_eq!(lit[(26, 3)].fg, theme::BORDER);
        // The folder on the way down is on the selected fill, without the
        // edge: the keyboard isn't there. The others are muted.
        assert_eq!(lit[(3, 4)].fg, theme::FOREGROUND);
        assert_eq!(lit[(0, 4)].symbol(), " ");
        assert_eq!(lit[(0, 4)].bg, theme::SELECTED);
        assert_eq!(lit[(25, 4)].bg, theme::SELECTED);
        assert_eq!(lit[(3, 3)].fg, theme::MUTED);
        assert_eq!(lit[(3, 3)].bg, Color::Reset);
        // Folders and files have their glyphs.
        assert_eq!(lit[(1, 3)].symbol(), icons::FOLDER);
        assert_eq!(lit[(1, 5)].symbol(), icons::TEXT);

        // Pictures itself, with the cursor and the edge of the list with the keyboard.
        assert_eq!(find(&lit, "Trip", 27..72), Some((30, 3)));
        assert_eq!(find(&lit, "a.jpg", 27..72), Some((30, 4)));
        assert_eq!(lit[(27, 3)].symbol(), "▎");
        assert_eq!(lit[(27, 3)].fg, theme::ACCENT);
        assert_eq!(lit[(30, 3)].fg, theme::FOREGROUND);
        assert_eq!(lit[(27, 3)].bg, theme::CURSOR);
        assert_eq!(lit[(52, 3)].bg, theme::CURSOR);
        assert_eq!(lit[(53, 3)].symbol(), "│");
    }

    #[test]
    fn the_rightmost_columns_stay_when_they_do_not_all_fit() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));

        // Room for three: home, Pictures, Trip.
        let wide = buffer(&mut f.app, 102, 26);
        assert_eq!(find(&wide, "notes.txt", 0..102), Some((3, 5)));
        assert_eq!(find(&wide, "a.jpg", 0..102), Some((30, 4)));
        assert_eq!(find(&wide, "NOTHING HERE YET", 0..102), Some((56, 3)));

        // Room for two: home steps out on the left.
        let narrow = buffer(&mut f.app, 72, 26);
        assert_eq!(find(&narrow, "notes.txt", 0..72), None);
        assert_eq!(find(&narrow, "a.jpg", 0..72), Some((3, 4)));
        assert_eq!(narrow[(0, 3)].bg, theme::SELECTED, "Trip");
        assert_eq!(find(&narrow, "NOTHING HERE YET", 0..72), Some((29, 3)));
    }

    #[test]
    fn a_long_column_scrolls_to_keep_the_way_down_in_view() {
        let mut f = fixture();
        for n in 1..=30 {
            std::fs::create_dir(f.home.join(format!("Folder {n}"))).unwrap();
        }
        std::fs::write(f.home.join("Folder 30").join("inside.txt"), "").unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::F(5), KeyCode::Down]);
        // Documents, Folder 1 ... Folder 30, Pictures: go into Folder 30.
        for _ in 0..29 {
            press(&mut f.app, &[KeyCode::Down]);
        }
        press(&mut f.app, &[KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Folder 30"));
        let buffer = buffer(&mut f.app, 72, 26);
        let y = (2..24).find(|y| cells(&buffer, *y, 3..12) == "Folder 30");
        let y = y.expect("the way down, in view");
        assert_eq!(buffer[(3, y)].bg, theme::SELECTED);
        assert_eq!(find(&buffer, "inside.txt", 27..72), Some((30, 3)));
    }
}
