//! The list view: name, size and date, no headings.

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{empty_text, fit, icon_style, pad, pad_left, row_style, scroll};
use crate::app::{App, HitMap, Pane, Spot};
use crate::format;
use crate::icons::Icon;
use crate::model::Entry;
use crate::theme;

const SIZE_WIDTH: usize = 11;
const DATE_WIDTH: usize = 10;

/// Draws one pane's items as rows; `focused` is whether it has the keyboard.
/// Each row is a spot for the mouse.
pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    index: usize,
    area: Rect,
    focused: bool,
    hits: &mut HitMap,
) {
    let date_style = app.date_style;
    let pane = &mut app.panes[index];
    if pane.entries.is_empty() {
        empty(frame, area, pane);
        return;
    }

    // One blank row of air on top, like folio's panels.
    let rows = area.height.saturating_sub(1) as usize;
    pane.offset = scroll(pane.offset, pane.cursor, rows);

    let width = area.width as usize;
    // The date steps aside first when it gets narrow, then the size.
    let show_size = width >= 44;
    let show_date = width >= 60;
    let fixed =
        4 + if show_size { SIZE_WIDTH + 2 } else { 0 } + if show_date { DATE_WIDTH + 2 } else { 0 };
    let name_width = width.saturating_sub(fixed + 2);
    let now = SystemTime::now();

    let mut lines = vec![Line::default()];
    for (item, entry) in pane.entries.iter().enumerate().skip(pane.offset).take(rows) {
        let y = area.y + lines.len() as u16;
        let row = Rect::new(area.x, y, area.width, 1);
        hits.add(
            row,
            Spot::Item {
                pane: index,
                index: item,
            },
        );
        let selected = pane.is_selected(item);
        let row = row_style(selected, focused);
        let icon = icon_style(row, entry.is_dir, selected && focused);
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

/// What a pane with no items says, a third of the way down the view.
pub fn empty(frame: &mut Frame, area: Rect, pane: &Pane) {
    let middle = Rect {
        y: area.y + area.height / 3,
        height: 1.min(area.height),
        ..area
    };
    frame.render_widget(
        Paragraph::new(fit(&empty_text(pane), area.width as usize))
            .style(Style::new().fg(theme::DIM))
            .alignment(Alignment::Center),
        middle,
    );
}
