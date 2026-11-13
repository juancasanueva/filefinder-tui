//! The sidebar: sections of places, the current one highlighted.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::pad;
use crate::app::{App, Focus};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let current = app.current_place();
    let cursor = (app.focus == Focus::Sidebar).then_some(app.sidebar_cursor);

    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    let mut index = 0;
    for section in &app.sidebar {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("  {}", section.title.to_uppercase()),
            theme::faint(),
        )));
        for place in &section.places {
            let style = if cursor == Some(index) {
                theme::selected()
            } else if current == Some(index) {
                Style::new()
                    .fg(theme::BRIGHT_FOREGROUND)
                    .bg(theme::SELECTION)
            } else {
                theme::base()
            };
            if cursor == Some(index) {
                cursor_line = lines.len();
            }
            let text = format!("  {}  {}", place.icon.glyph(), place.label);
            lines.push(Line::from(Span::styled(
                pad(&text, width.saturating_sub(1)),
                style,
            )));
            index += 1;
        }
    }

    // Keep the keyboard's place on screen in a long sidebar.
    let height = area.height as usize;
    let scroll = (cursor_line + 1).saturating_sub(height);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), area);
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{ctrl, fixture, press};
    use crate::theme;
    use crate::ui::testing::buffer;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use std::fs;

    /// The row, under the top bar, where the sidebar shows a piece of text.
    fn row(drawn: &Buffer, text: &str) -> Option<u16> {
        (2..drawn.area.height).find(|y| {
            let line: String = (0..23).map(|x| drawn[(x, *y)].symbol()).collect();
            line.contains(text)
        })
    }

    #[test]
    fn every_section_has_a_dim_label_and_a_blank_row_above_it() {
        let mut f = fixture();
        fs::create_dir_all(f.volumes.join("USB")).unwrap();
        f.app.navigate(&f.home.join("Pictures").join("Trip"));
        f.app.handle_key(ctrl(KeyCode::Char('p')));
        press(&mut f.app, &[KeyCode::F(5)]);
        let drawn = buffer(&mut f.app, 120, 30);

        let shown = [
            ("HOME", 3),
            ("Documents", 4),
            ("Pictures", 5),
            ("PINNED", 7),
            ("Trip", 8),
            ("VIEWS", 10),
            ("Recents", 11),
            ("Trash", 12),
            ("DRIVES", 14),
            ("USB", 15),
        ];
        for (text, y) in shown {
            assert_eq!(row(&drawn, text), Some(y), "{text}");
        }
        for y in [3, 7, 10, 14] {
            assert_eq!(drawn[(2, y)].fg, theme::FAINT, "the label on row {y}");
        }
        // The current place is the pin, not the home folder above it.
        assert_eq!(drawn[(0, 8)].bg, theme::SELECTION);
        assert_eq!(drawn[(0, 5)].bg, theme::BACKGROUND);
    }

    #[test]
    fn a_sidebar_taller_than_the_window_scrolls_to_the_keyboards_place() {
        let mut f = fixture();
        for n in 0..20 {
            fs::create_dir(f.home.join(format!("Folder {n:02}"))).unwrap();
        }
        press(&mut f.app, &[KeyCode::BackTab]);
        let top = buffer(&mut f.app, 120, 14);
        assert_eq!(row(&top, "HOME"), Some(3));
        assert_eq!(row(&top, "Trash"), None);

        press(&mut f.app, &[KeyCode::Down; 30]);
        let bottom = buffer(&mut f.app, 120, 14);
        assert_eq!(row(&bottom, "Trash"), Some(13), "on the last row");
        assert_eq!(bottom[(0, 13)].bg, theme::SELECTION);
        assert_eq!(bottom[(3, 13)].fg, theme::ACCENT);
        assert_eq!(row(&bottom, "HOME"), None);
    }
}
