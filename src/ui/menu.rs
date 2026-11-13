//! The menu: what can be done with the selection, or with a place in the
//! sidebar. It looks like the dialogs, a dark panel with a hairline border,
//! and opens next to what it is about.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::pad;
use crate::app::{HitMap, Menu, MenuRow, Spot};
use crate::theme;

/// Draws the menu next to `anchor`, the cells of the item it is about. The
/// menu and each of its lines are spots for the mouse.
pub fn draw(frame: &mut Frame, menu: &Menu, anchor: Rect, area: Rect, hits: &mut HitMap) {
    let label_width = |row: &MenuRow| match row {
        MenuRow::Item { label, .. } => label.width(),
        MenuRow::Separator => 0,
    };
    let widest = menu.rows.iter().map(label_width).max().unwrap_or(0);
    // The border, and a cell of air on each side of the labels.
    let width = (widest as u16 + 4).min(area.width);
    let height = (menu.rows.len() as u16 + 2).min(area.height);
    if width < 3 || height < 3 {
        return;
    }
    let outer = place(anchor, width, height, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme::rule())
        .style(theme::panel());
    let inner = block.inner(outer);
    frame.render_widget(Clear, outer);
    frame.render_widget(block, outer);

    // A window too short for every line shows the ones around the keyboard.
    let rows = inner.height as usize;
    let first = (menu.cursor + 1).saturating_sub(rows);
    let cells = inner.width as usize;
    let line = |(index, row): (usize, &MenuRow)| match row {
        MenuRow::Item { label, enabled, .. } => {
            let style = theme::menu_item(index == menu.cursor, *enabled);
            Line::from(Span::styled(pad(&format!(" {label}"), cells), style))
        }
        MenuRow::Separator => Line::from(Span::styled("─".repeat(cells), theme::rule())),
    };
    let showing = menu.rows.iter().enumerate().skip(first).take(rows);
    let lines: Vec<Line> = showing.map(line).collect();
    frame.render_widget(Paragraph::new(lines), inner);
    hits.add(outer, Spot::Menu);
    for (y, row) in (inner.y..inner.bottom()).zip(first..menu.rows.len()) {
        hits.add(Rect::new(inner.x, y, inner.width, 1), Spot::MenuRow(row));
    }
}

/// Where a menu of a size goes: right under the item it is about; above it
/// when there is no room below; and with room in neither place, as low as
/// it fits in the window. Never past the window's right edge.
fn place(anchor: Rect, width: u16, height: u16, area: Rect) -> Rect {
    let x = anchor.x.min(area.right().saturating_sub(width)).max(area.x);
    let below = anchor.bottom();
    let y = if below.saturating_add(height) <= area.bottom() {
        below
    } else if anchor.y >= area.y.saturating_add(height) {
        anchor.y - height
    } else {
        area.bottom().saturating_sub(height)
    };
    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{alt, ctrl, fixture, grid_fixture, press, several};
    use crate::theme;
    use crate::ui::testing::{buffer, find};
    use ratatui::crossterm::event::KeyCode;
    use std::fs;

    #[test]
    fn the_menu_opens_under_the_selected_item_in_the_look_of_the_dialogs() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(alt(KeyCode::Enter));
        let drawn = buffer(&mut f.app, 120, 30);
        let (name_x, name_y) = find(&drawn, "notes.txt", 24..86).expect("the item");
        // A hairline border starting under the name, on the darkest background.
        let corner = (name_x, name_y + 1);
        assert_eq!(drawn[corner].symbol(), "┌");
        assert_eq!(drawn[corner].fg, theme::RULE);
        assert_eq!(drawn[corner].bg, theme::DARK_BACKGROUND);
        // The first line has the keyboard: the selection's colors.
        let open = find(&drawn, "Open", 24..86).expect("the first line");
        assert_eq!(open, (name_x + 2, name_y + 2));
        assert_eq!(drawn[open].fg, theme::ACCENT);
        assert_eq!(drawn[open].bg, theme::SELECTION);
        assert_eq!(drawn[(open.0 - 1, open.1)].bg, theme::SELECTION);
        let preview = find(&drawn, "Preview", 24..86).expect("the second line");
        assert_eq!(preview, (open.0, open.1 + 1));
        assert_eq!(drawn[preview].fg, theme::FOREGROUND);
        assert_eq!(drawn[preview].bg, theme::DARK_BACKGROUND);
        // A hairline between two groups, from border to border.
        let hairline = (open.0 - 1, open.1 + 2);
        assert_eq!(drawn[hairline].symbol(), "─");
        assert_eq!(drawn[hairline].fg, theme::RULE);
        assert_eq!(drawn[hairline].bg, theme::DARK_BACKGROUND);
        assert_eq!(find(&drawn, "Rename", 24..86), Some((open.0, open.1 + 3)));
        let last = find(&drawn, "Delete…", 24..86).expect("the last line");
        assert_eq!(last, (open.0, open.1 + 15));
        assert_eq!(drawn[(name_x, last.1 + 1)].symbol(), "└");

        // Down moves the colors along.
        press(&mut f.app, &[KeyCode::Down]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[preview].bg, theme::SELECTION);
        assert_eq!(drawn[open].bg, theme::DARK_BACKGROUND);

        // Closed, it is gone.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "Preview", 24..86), None);
        assert_eq!(find(&drawn, "notes.txt", 24..86), Some((name_x, name_y)));
    }

    #[test]
    fn a_line_that_cannot_be_used_is_dim_wherever_the_keyboard_is() {
        let mut f = fixture();
        // A folder, with nothing copied to paste into it.
        press(&mut f.app, &[KeyCode::Down]);
        f.app.handle_key(alt(KeyCode::Enter));
        let drawn = buffer(&mut f.app, 120, 30);
        let paste = find(&drawn, "Paste Into Folder", 24..86).expect("the line");
        assert_eq!(drawn[paste].fg, theme::DIM);
        assert_eq!(drawn[paste].bg, theme::DARK_BACKGROUND);
        let copy = find(&drawn, "Copy", 24..86).expect("a line that can be used");
        assert_eq!(drawn[copy].fg, theme::FOREGROUND);

        // With the keyboard on it, it is still dim.
        press(&mut f.app, &[KeyCode::Down; 6]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[paste].fg, theme::DIM);
        assert_eq!(drawn[paste].bg, theme::SELECTION);
    }

    #[test]
    fn in_the_sidebar_the_menu_opens_under_the_place() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::BackTab, KeyCode::Down]);
        f.app.handle_key(alt(KeyCode::Enter));
        let drawn = buffer(&mut f.app, 120, 30);
        let (_, place_y) = find(&drawn, "Pictures", 0..24).expect("the place");
        assert_eq!(find(&drawn, "Open", 0..86), Some((4, place_y + 2)));
        assert_eq!(drawn[(2, place_y + 1)].symbol(), "┌");
        // The files underneath are not what it is about.
        assert_eq!(find(&drawn, "Preview", 0..120), None);
    }

    #[test]
    fn in_the_grid_and_in_columns_the_menu_opens_at_the_selected_item() {
        let mut f = grid_fixture();
        f.app.handle_key(alt(KeyCode::Enter));
        let first = buffer(&mut f.app, 120, 24);
        let at_first = find(&first, "Open Folder", 24..86).expect("the menu");
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Right]);
        f.app.handle_key(alt(KeyCode::Enter));
        let second = buffer(&mut f.app, 120, 24);
        let at_second = find(&second, "Open Folder", 24..86).expect("the menu");
        // One tile further right, and so is the menu.
        assert_eq!(at_second.0, at_first.0 + 20);
        assert_eq!(at_second.1, at_first.1);
        press(&mut f.app, &[KeyCode::Esc]);

        // In columns it is in the column of the folder that is showing.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        f.app.handle_key(alt(KeyCode::Enter));
        let columns = buffer(&mut f.app, 120, 30);
        let (name_x, name_y) = find(&columns, "Trip", 24..86).expect("the item");
        assert!(name_x > 24 + 27, "in the second column");
        let open = find(&columns, "Open Folder", 24..86).expect("the menu");
        assert_eq!(open, (name_x + 2, name_y + 2));
    }

    #[test]
    fn a_menu_with_no_room_below_opens_above_and_one_too_tall_scrolls() {
        let mut f = fixture();
        for n in 10..50 {
            fs::write(f.home.join(format!("{n}.txt")), "").unwrap();
        }
        press(&mut f.app, &[KeyCode::F(5), KeyCode::End]);
        f.app.handle_key(alt(KeyCode::Enter));
        let drawn = buffer(&mut f.app, 120, 40);
        let (name_x, name_y) = find(&drawn, "notes.txt", 24..86).expect("the item");
        assert!(name_y > 30, "near the bottom");
        assert_eq!(drawn[(name_x, name_y - 1)].symbol(), "└");
        assert_eq!(
            find(&drawn, "Delete…", 24..86),
            Some((name_x + 2, name_y - 2))
        );
        assert_eq!(
            find(&drawn, "Open", 24..86),
            Some((name_x + 2, name_y - 17))
        );
        press(&mut f.app, &[KeyCode::Esc]);

        // A window with room neither above nor below: the menu is as low
        // as it fits, and every line is there.
        let mut f = several();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(alt(KeyCode::Enter));
        let drawn = buffer(&mut f.app, 120, 24);
        assert!(find(&drawn, "Open", 24..86).is_some());
        assert_eq!(find(&drawn, "Delete…", 24..86).map(|at| at.1), Some(22));

        // A window shorter than the menu shows the lines around the keyboard.
        let short = buffer(&mut f.app, 120, 10);
        assert!(find(&short, "Cut", 24..86).is_some());
        assert_eq!(find(&short, "Delete…", 24..86), None);
        press(&mut f.app, &[KeyCode::Down; 30]);
        let short = buffer(&mut f.app, 120, 10);
        let last = find(&short, "Delete…", 24..86).expect("the last line");
        assert_eq!(short[last].bg, theme::SELECTION);
        assert_eq!(find(&short, "Rename", 24..86), None);

        // And one too small for anything is still drawn.
        for (width, height) in [(3, 2), (20, 5), (40, 3)] {
            buffer(&mut f.app, width, height);
        }
    }

    #[test]
    fn the_menu_goes_below_the_item_then_above_it_then_as_low_as_it_fits() {
        let area = Rect::new(0, 0, 80, 24);
        let item = |x, y| Rect::new(x, y, 0, 1);
        assert_eq!(place(item(30, 5), 20, 10, area), Rect::new(30, 6, 20, 10));
        assert_eq!(place(item(30, 13), 20, 10, area), Rect::new(30, 14, 20, 10));
        assert_eq!(place(item(30, 14), 20, 10, area), Rect::new(30, 4, 20, 10));
        assert_eq!(place(item(30, 9), 20, 20, area), Rect::new(30, 4, 20, 20));
        // Never past the right edge.
        assert_eq!(place(item(70, 5), 20, 10, area), Rect::new(60, 6, 20, 10));
    }
}
