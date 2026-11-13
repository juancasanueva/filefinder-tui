//! The list view: name, size and date, no headings.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{pad, pad_left};
use crate::app::App;
use crate::format;
use crate::icons::Icon;
use crate::model::Entry;
use crate::theme;

const SIZE_WIDTH: usize = 11;
const DATE_WIDTH: usize = 10;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let date_style = app.date_style;
    let pane = &mut app.panes[app.active];
    if pane.entries.is_empty() {
        empty(frame, area);
        return;
    }

    // One blank row of air on top, like folio's panels.
    let rows = area.height.saturating_sub(1) as usize;
    if pane.cursor < pane.offset {
        pane.offset = pane.cursor;
    } else if rows > 0 && pane.cursor >= pane.offset + rows {
        pane.offset = pane.cursor + 1 - rows;
    }

    let width = area.width as usize;
    // The date steps aside first when it gets narrow, then the size.
    let show_size = width >= 44;
    let show_date = width >= 60;
    let fixed =
        4 + if show_size { SIZE_WIDTH + 2 } else { 0 } + if show_date { DATE_WIDTH + 2 } else { 0 };
    let name_width = width.saturating_sub(fixed + 2);
    let now = SystemTime::now();

    let mut lines = vec![Line::default()];
    for (index, entry) in pane.entries.iter().enumerate().skip(pane.offset).take(rows) {
        let selected = index == pane.cursor;
        let row = match (selected, focused) {
            (true, true) => theme::selected(),
            (true, false) => theme::selected_inactive(),
            _ => theme::base(),
        };
        let icon = if selected && focused {
            row
        } else if entry.is_dir {
            row.fg(theme::ACCENT)
        } else {
            row.fg(theme::DIM)
        };
        let meta = if selected { row } else { row.fg(theme::DIM) };

        let mut spans = vec![
            Span::styled(
                format!(" {}  ", Icon::for_category(entry.category()).glyph()),
                icon,
            ),
            Span::styled(pad(&entry.name, name_width), row),
        ];
        if show_size {
            spans.push(Span::styled(
                pad_left(&size_text(entry), SIZE_WIDTH + 2),
                meta,
            ));
        }
        if show_date {
            let date = entry
                .modified
                .map(|time| format::short_date(time, now, date_style))
                .unwrap_or_default();
            spans.push(Span::styled(pad_left(&date, DATE_WIDTH + 2), meta));
        }
        spans.push(Span::styled("  ", row));
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// A file's size, or how many things a folder holds.
pub fn size_text(entry: &Entry) -> String {
    match entry.item_count() {
        Some(count) => format::items(count),
        None if entry.is_dir => String::new(),
        None => format::size(entry.size),
    }
}

pub fn empty(frame: &mut Frame, area: Rect) {
    let middle = Rect {
        y: area.y + area.height / 3,
        height: 1.min(area.height),
        ..area
    };
    frame.render_widget(
        Paragraph::new("No items")
            .style(Style::new().fg(theme::DIM))
            .alignment(Alignment::Center),
        middle,
    );
}
