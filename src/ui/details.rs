//! The details pane: what the selected item is, and an Open button.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{fit, pad};
use crate::app::{App, Focus};
use crate::format;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let inner = Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(4),
        y: area.y + 1,
        height: area.height.saturating_sub(2),
    };
    let Some(entry) = app.selected() else {
        frame.render_widget(
            Paragraph::new("Nothing selected").style(theme::dim()),
            inner,
        );
        return;
    };
    let [info, button] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    let width = info.width as usize;

    let summary = match entry.item_count() {
        Some(count) => format!("Folder · {}", format::items(count)),
        None if entry.is_dir => "Folder".to_string(),
        None => format!("{} · {}", entry.kind(), format::size(entry.size)),
    };
    let mut lines = vec![
        Line::from(Span::styled(entry.name.clone(), theme::title())),
        Line::from(Span::styled(fit(&summary, width), theme::dim())),
        Line::default(),
    ];
    if let Some(modified) = entry.modified {
        lines.push(field(
            "Modified",
            &format::date_time(modified, app.date_style),
            width,
        ));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), info);

    let label = if app.focus == Focus::Details {
        "› Open"
    } else {
        "Open"
    };
    let text = format!(
        " {}⏎ ",
        pad(label, (button.width as usize).saturating_sub(3))
    );
    frame.render_widget(Paragraph::new(text).style(theme::button()), button);
}

/// A label on the left and its value against the right edge.
fn field(label: &str, value: &str, width: usize) -> Line<'static> {
    let gap = width.saturating_sub(label.width() + value.width()).max(1);
    Line::from(vec![
        Span::styled(label.to_string(), theme::dim()),
        Span::raw(" ".repeat(gap)),
        Span::styled(value.to_string(), theme::base()),
    ])
}
