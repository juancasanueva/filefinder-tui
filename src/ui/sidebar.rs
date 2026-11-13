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
