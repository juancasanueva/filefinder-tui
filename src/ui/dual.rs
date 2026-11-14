//! Two panes, Ctrl+4: two lists side by side with a line between them, each
//! under its own path. Only the list with the keyboard draws the accent edge.

use ratatui::Frame;
use ratatui::layout::Rect;

use super::{draw_pane, separator, topbar};
use crate::app::{App, HitMap};

/// Where the line between the two lists goes in a body `width` cells wide:
/// the left one takes the odd cell.
pub fn divider(width: u16) -> u16 {
    width.saturating_sub(1).div_ceil(2)
}

/// Draws both panes in the body. Returns the cells of the item under the
/// active pane's cursor.
pub fn draw(frame: &mut Frame, app: &mut App, body: Rect, hits: &mut HitMap) -> Rect {
    let left = divider(body.width);
    let sides = [
        Rect::new(body.x, body.y, left, body.height),
        Rect::new(
            body.x + left + 1,
            body.y,
            body.width.saturating_sub(left + 1),
            body.height,
        ),
    ];
    separator(frame, Rect::new(body.x + left, body.y, 1, body.height));
    let mut cells = [Rect::default(); 2];
    let count = app.panes().len().min(2);
    for (pane, area) in sides.into_iter().enumerate().take(count) {
        // The header has room only for the active pane's path: each pane
        // says its own on its first row.
        let path = Rect { height: 1, ..area };
        let files = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        topbar::draw_pane_path(frame, app, pane, path, hits);
        cells[pane] = draw_pane(frame, app, pane, files, hits);
    }
    cells[app.active().min(1)]
}
