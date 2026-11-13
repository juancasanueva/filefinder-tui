//! The list view: name, permissions, size and date, no headings.

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

/// `drwx------`
const MODE_WIDTH: usize = 10;
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
    // When it gets narrow the permissions step aside first, then the date,
    // then the size.
    let show_size = width >= 44;
    let show_date = width >= 60;
    let show_mode = width >= 74;
    let column = |shown: bool, width: usize| if shown { width + 2 } else { 0 };
    let fixed = 4
        + column(show_mode, MODE_WIDTH)
        + column(show_size, SIZE_WIDTH)
        + column(show_date, DATE_WIDTH);
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
        if show_mode {
            spans.push(Span::styled(
                pad_left(&format::permissions(entry.mode), MODE_WIDTH + 2),
                meta,
            ));
        }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use ratatui::crossterm::event::KeyCode;

    use crate::app::testing::{fixture, press};
    use crate::ui::testing::screen;

    fn row<'a>(lines: &'a [String], name: &str) -> &'a str {
        lines
            .iter()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("no row for {name}"))
    }

    #[test]
    fn rows_show_permissions_between_the_name_and_the_size() {
        let mut f = fixture();
        let set = |name: &str, mode| {
            fs::set_permissions(f.home.join(name), fs::Permissions::from_mode(mode)).unwrap()
        };
        set("Documents", 0o700);
        set("notes.txt", 0o640);
        press(&mut f.app, &[KeyCode::F(5)]);

        // 150 columns leave the list 92 cells: room for every column.
        let lines = screen(&mut f.app, 150, 20);
        assert!(row(&lines, "Documents").contains("drwx------"));
        let notes = row(&lines, "notes.txt");
        let name = notes.find("notes.txt").unwrap();
        let mode = notes.find("-rw-r-----").expect("permissions shown");
        let size = notes.find("5 B").expect("size shown");
        assert!(name < mode && mode < size, "{notes}");
    }

    #[test]
    fn permissions_step_aside_first_when_the_list_is_narrow() {
        let mut f = fixture();
        fs::set_permissions(f.home.join("notes.txt"), fs::Permissions::from_mode(0o640)).unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);

        // 120 columns leave the list 62 cells: size and date stay, as before.
        let lines = screen(&mut f.app, 120, 20);
        let notes = row(&lines, "notes.txt");
        assert!(!notes.contains("rw-"), "{notes}");
        assert!(notes.contains("5 B"), "{notes}");
    }
}
