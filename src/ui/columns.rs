//! The columns view: Finder-style, one column for each folder on the way down.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{empty_text, fit, icon_style, pad, row_style, scroll};
use crate::app::App;
use crate::icons::Icon;
use crate::model::Entry;
use crate::theme;

const COLUMN_WIDTH: u16 = 26;
/// A column and the hairline on its right.
const PITCH: u16 = COLUMN_WIDTH + 1;

/// Draws one pane's folder and the ones above it; `focused` is whether it
/// has the keyboard.
pub fn draw(frame: &mut Frame, app: &mut App, pane: usize, area: Rect, focused: bool) {
    // One blank row of air on top, like the list.
    let rows = area.height.saturating_sub(1) as usize;
    let pane = &mut app.panes[pane];
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
            .border_style(theme::rule());
        let inner = Rect {
            width: column.width.min(COLUMN_WIDTH),
            ..column
        };
        if column.width == PITCH {
            frame.render_widget(hairline, column);
        }
        match pane.ancestors.get(total - shown + slot) {
            Some(above) => {
                // The way down is highlighted in grey: the keyboard is further right.
                let offset = scroll(0, above.on_path.unwrap_or(0), rows);
                let lit = theme::selected_inactive();
                let on_path = |index| above.on_path == Some(index);
                draw_column(frame, inner, &above.entries, offset, on_path, lit);
            }
            None if pane.entries.is_empty() => {
                // The pane's own column is the last one: there is room to
                // its right for what it has to say.
                let room = Rect {
                    width: area.right().saturating_sub(inner.x),
                    ..inner
                };
                let said = fit(&format!("  {}", empty_text(pane)), room.width as usize);
                let text = vec![Line::default(), Line::from(said)];
                frame.render_widget(Paragraph::new(text).style(theme::dim()), room);
            }
            None => {
                let lit = row_style(true, focused);
                let selected = |index| pane.is_selected(index);
                draw_column(frame, inner, &pane.entries, pane.offset, selected, lit);
            }
        }
    }
}

/// One folder's rows from `offset` down, the marked ones in the `lit` style.
fn draw_column(
    frame: &mut Frame,
    area: Rect,
    entries: &[Entry],
    offset: usize,
    marked: impl Fn(usize) -> bool,
    lit: Style,
) {
    let rows = area.height.saturating_sub(1) as usize;
    let name_width = (area.width as usize).saturating_sub(7);
    let mut lines = vec![Line::default()];
    for (index, entry) in entries.iter().enumerate().skip(offset).take(rows) {
        let is_marked = marked(index);
        let row = if is_marked { lit } else { theme::base() };
        let icon = icon_style(row, entry.is_dir, is_marked && lit == theme::selected());
        // Folders end with a chevron: there is a column to their right.
        let chevron = if entry.is_dir { "›" } else { " " };
        let chevron_style = if is_marked { row } else { row.fg(theme::DIM) };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {}  ", Icon::for_category(entry.category()).glyph()),
                icon,
            ),
            Span::styled(pad(&entry.name, name_width), row),
            Span::styled(format!(" {chevron} "), chevron_style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{ctrl, fixture, press};
    use crate::theme;
    use crate::ui::testing::{buffer, find};
    use ratatui::crossterm::event::KeyCode;

    /// The cells of one screen row between two columns, as text.
    fn cells(buffer: &ratatui::buffer::Buffer, y: u16, from: u16, to: u16) -> String {
        (from..to).map(|x| buffer[(x, y)].symbol()).collect()
    }

    #[test]
    fn columns_run_from_home_down_to_the_folder_with_the_way_there_highlighted() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right]);
        let lit = buffer(&mut f.app, 130, 24);

        // Home, 26 cells wide, then a hairline.
        assert_eq!(find(&lit, "Documents", 24..96), Some((28, 3)));
        assert_eq!(find(&lit, "Pictures", 24..96), Some((28, 4)));
        assert_eq!(find(&lit, "notes.txt", 24..96), Some((28, 5)));
        assert_eq!(lit[(50, 3)].symbol(), "│");
        assert_eq!(lit[(50, 23)].symbol(), "│");
        assert_eq!(lit[(50, 3)].fg, theme::RULE);
        // The folder on the way down is highlighted, in grey: the keyboard isn't there.
        assert_eq!(lit[(28, 4)].fg, theme::BRIGHT_FOREGROUND);
        assert_eq!(lit[(24, 4)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(lit[(49, 4)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(lit[(28, 3)].bg, theme::BACKGROUND);
        // Folders end with a chevron, files don't.
        assert!(cells(&lit, 3, 24, 50).trim_end().ends_with('›'));
        assert!(cells(&lit, 4, 24, 50).trim_end().ends_with('›'));
        assert!(cells(&lit, 5, 24, 50).trim_end().ends_with("notes.txt"));

        // Pictures itself, with the cursor.
        assert_eq!(find(&lit, "Trip", 51..96), Some((55, 3)));
        assert_eq!(find(&lit, "a.jpg", 51..96), Some((55, 4)));
        assert!(cells(&lit, 3, 51, 77).trim_end().ends_with('›'));
        assert_eq!(lit[(55, 3)].fg, theme::ACCENT);
        assert_eq!(lit[(51, 3)].bg, theme::SELECTION);
        assert_eq!(lit[(76, 3)].bg, theme::SELECTION);
        assert_eq!(lit[(77, 3)].symbol(), "│");

        press(&mut f.app, &[KeyCode::Tab]);
        let unlit = buffer(&mut f.app, 130, 24);
        assert_eq!(unlit[(55, 3)].fg, theme::BRIGHT_FOREGROUND);
        assert_eq!(unlit[(51, 3)].bg, theme::LIGHTER_BACKGROUND);
    }

    #[test]
    fn the_rightmost_columns_stay_when_they_do_not_all_fit() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Right, KeyCode::Right]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures").join("Trip"));

        // Room for three: home, Pictures, Trip.
        let wide = buffer(&mut f.app, 160, 24);
        assert_eq!(find(&wide, "notes.txt", 24..126), Some((28, 5)));
        assert_eq!(find(&wide, "a.jpg", 24..126), Some((55, 4)));
        assert_eq!(find(&wide, "No items", 24..126), Some((80, 3)));

        // Room for two: home steps out on the left.
        let narrow = buffer(&mut f.app, 130, 24);
        assert_eq!(find(&narrow, "notes.txt", 24..96), None);
        assert_eq!(find(&narrow, "a.jpg", 24..96), Some((28, 4)));
        assert_eq!(narrow[(24, 3)].bg, theme::LIGHTER_BACKGROUND, "Trip");
        assert_eq!(find(&narrow, "No items", 24..96), Some((53, 3)));
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
        let buffer = buffer(&mut f.app, 130, 24);
        let (x, y) = find(&buffer, "Folder 30", 24..96).unwrap();
        assert_eq!((x, buffer[(x, y)].bg), (28, theme::LIGHTER_BACKGROUND));
        assert_eq!(find(&buffer, "inside.txt", 51..96), Some((55, 3)));
    }
}
