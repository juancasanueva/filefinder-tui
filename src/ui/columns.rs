//! The columns view, Ctrl+2: Miller columns for the way from home (or `/`)
//! down to the folder, as many as the window has room for, then the
//! preview of the item under the cursor.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

use super::list::{self, Look, detail_text};
use super::{NAME_INDENT, scroll, separator};
use crate::app::{App, HitMap, Spot};

/// How many columns a window `width` cells wide has room for, flea's way:
/// three at the usual widths, two when narrow, up to five when wide.
pub fn track_count(width: u16) -> usize {
    match width {
        0..90 => 2,
        90..170 => 3,
        170..230 => 4,
        _ => 5,
    }
}

/// Where the columns go in `area`: equal shares of it, one line between
/// each two, the rightmost (the active pane's own folder) taking what is
/// left over. As many as `track_count` says and as there are folders on
/// the way down; what a search found is a column on its own.
pub fn tracks(app: &App, area: Rect, window: u16) -> Vec<Rect> {
    let total = app.pane().ancestors.len() + 1;
    let count = track_count(window);
    let shown = count.min(total) as u16;
    let width = area.width.saturating_sub(count as u16 - 1) / count as u16;
    (0..shown)
        .map(|slot| {
            let x = area.x + slot * (width + 1);
            let width = if slot + 1 == shown {
                area.right().saturating_sub(x)
            } else {
                width
            };
            Rect::new(x, area.y, width, area.height)
        })
        .collect()
}

/// Draws the active pane's way down in the `tracks` laid out for it, with
/// a line between each two. Returns the cells of the item under the cursor.
pub fn draw(frame: &mut Frame, app: &mut App, tracks: &[Rect], hits: &mut HitMap) -> Rect {
    let index = app.active();
    let focused = super::focused(app, index);
    let (detail, style, caption) = (app.detail, app.date_style, app.caption);
    let now = std::time::SystemTime::now();
    let pane = &mut app.tab_mut().panes[index];
    let Some(own) = tracks.last().copied() else {
        return Rect::default();
    };
    let rows = own.height as usize;
    pane.page_rows = rows.max(1);
    let total = pane.ancestors.len() + 1;
    for (slot, track) in tracks.iter().enumerate() {
        if slot > 0 {
            separator(frame, Rect::new(track.x - 1, track.y, 1, track.height));
        }
        let depth = total - tracks.len() + slot;
        let Some(above) = pane.ancestors.get(depth) else {
            continue;
        };
        // The way down is a cursor row the keyboard isn't in; the rest muted.
        let offset = scroll(0, above.on_path.unwrap_or(0), rows);
        let mut lines = Vec::new();
        for (row, entry) in above.entries.iter().enumerate().skip(offset).take(rows) {
            let y = track.y + lines.len() as u16;
            let spot = Spot::Above {
                pane: index,
                depth,
                index: row,
            };
            hits.add(Rect::new(track.x, y, track.width, 1), spot);
            let look = Look {
                cursor: above.on_path == Some(row),
                parent: true,
                ..Look::default()
            };
            lines.push(list::row(entry, track.width as usize, None, look, "", None));
        }
        frame.render_widget(Paragraph::new(lines), *track);
    }

    // The pane's own folder, the rightmost column.
    hits.add(own, Spot::Pane(index));
    let mut item = Rect::new(own.x + NAME_INDENT, own.y, 0, 1);
    if pane.entries.is_empty() {
        list::empty(frame, own, pane, caption);
        return item;
    }
    pane.offset = scroll(pane.offset, pane.cursor, rows);
    let query = pane.matching();
    let mut lines = Vec::new();
    for (row, entry) in pane.entries.iter().enumerate().skip(pane.offset).take(rows) {
        let y = own.y + lines.len() as u16;
        let spot = Spot::Item {
            pane: index,
            index: row,
        };
        hits.add(Rect::new(own.x, y, own.width, 1), spot);
        if row == pane.cursor {
            item.y = y;
        }
        let look = Look {
            cursor: row == pane.cursor,
            focused,
            marked: pane.is_marked(row),
            ..Look::default()
        };
        let value = detail_text(entry, detail, now, style);
        let width = own.width as usize;
        lines.push(list::row(entry, width, Some(&value), look, &query, None));
    }
    frame.render_widget(Paragraph::new(lines), own);
    item
}
