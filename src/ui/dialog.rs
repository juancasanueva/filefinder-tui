//! Dialogs: the question before deleting for good or emptying the trash.
//! They look like the `?` overlay: a frame in the card's rule.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::fit;
use crate::app::{Dialog, HitMap, Spot};
use crate::ops;
use crate::theme;

const WIDTH: u16 = 72;

/// Draws the dialog; it and each of its buttons are spots for the mouse.
pub fn draw(frame: &mut Frame, dialog: &Dialog, area: Rect, hits: &mut HitMap) {
    match dialog {
        Dialog::Delete { paths, armed } => {
            let what = match paths.as_slice() {
                [one] => {
                    let name = one.file_name().unwrap_or(one.as_os_str());
                    fit(&name.to_string_lossy(), 40)
                }
                several => ops::count(several.len()),
            };
            let asked = "Delete for good?";
            confirm(frame, area, asked, &what, "Delete", *armed, hits);
        }
        Dialog::EmptyTrash { paths, armed } => {
            let what = ops::count(paths.len());
            confirm(
                frame,
                area,
                "Empty the trash?",
                &what,
                "Empty Trash",
                *armed,
                hits,
            );
        }
    }
}

/// Clears a panel in the middle of the window and returns where its `rows`
/// lines of content go.
fn panel(frame: &mut Frame, area: Rect, rows: u16, hits: &mut HitMap) -> Rect {
    let width = WIDTH.min(area.width.saturating_sub(4));
    // A short window gives up the air above and below the content first.
    let air = u16::from(area.height >= rows + 6);
    let height = (rows + 2 + 2 * air).min(area.height);
    let outer = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme::card_rule())
        .padding(Padding::new(3, 3, air, air))
        .style(theme::base());
    let inner = block.inner(outer);
    frame.render_widget(Clear, outer);
    frame.render_widget(block, outer);
    hits.add(outer, Spot::Dialog);
    inner
}

/// A button `width` cells wide at `x` on a row, as much of it as shows.
fn button(hits: &mut HitMap, row: Rect, x: u16, width: usize, yes: bool) {
    let cells = Rect::new(x, row.y, width as u16, 1).intersection(row);
    hits.add(cells, Spot::Choice(yes));
}

/// The question before deleting `what` for good, with its two choices:
/// `yes`, which is lit once it is `armed`, and Cancel.
fn confirm(
    frame: &mut Frame,
    area: Rect,
    asked: &str,
    what: &str,
    yes: &str,
    armed: bool,
    hits: &mut HitMap,
) {
    let inner = panel(frame, area, 6, hits);
    let rows = [1, 1, 2, 1, 1].map(Constraint::Length);
    let [title, _, text, _, choices] = Layout::vertical(rows).areas(inner);
    let asks = format!("{what} will be deleted permanently. This can't be undone.");
    frame.render_widget(Span::styled(asked, theme::title()), title);
    frame.render_widget(Paragraph::new(asks).wrap(Wrap { trim: true }), text);
    let (yes, cancel) = (format!("  {yes}  "), "  Cancel  ");
    button(hits, choices, choices.x, yes.width(), true);
    let after = choices.x.saturating_add(yes.width() as u16 + 2);
    button(hits, choices, after, cancel.width(), false);
    let line = Line::from(vec![
        Span::styled(yes, theme::choice(armed)),
        Span::raw("  "),
        Span::styled(cancel, theme::choice(!armed)),
    ]);
    frame.render_widget(line, choices);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{fixture, press, shift};
    use crate::ui::testing::{buffer, find};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn the_delete_dialog_asks_with_cancel_lit_until_delete_is_chosen() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(shift(KeyCode::Delete));
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "Delete for good?", 0..120).is_some());
        let asks = "notes.txt will be deleted permanently. This can't be undone.";
        assert!(find(&drawn, asks, 0..120).is_some());
        // The choice, not the title: it is followed by the gap before Cancel.
        let delete = find(&drawn, "Delete  ", 0..120).expect("the Delete choice");
        let cancel = find(&drawn, "Cancel", 0..120).expect("the Cancel choice");
        assert_eq!(delete.1, cancel.1, "side by side");
        assert_eq!(drawn[cancel].bg, theme::WASH, "Cancel is what Enter takes");
        assert_ne!(drawn[delete].bg, theme::WASH);

        press(&mut f.app, &[KeyCode::Right]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[delete].bg, theme::WASH);
        assert_ne!(drawn[cancel].bg, theme::WASH);
    }

    #[test]
    fn deleting_several_items_asks_about_their_number() {
        let mut f = fixture();
        let paths = vec![f.home.join("notes.txt"), f.home.join("Documents")];
        f.app.dialog = Some(Dialog::Delete {
            paths,
            armed: false,
        });
        let drawn = buffer(&mut f.app, 120, 30);
        let asks = "2 items will be deleted permanently. This can't be undone.";
        assert!(find(&drawn, asks, 0..120).is_some());
    }

    #[test]
    fn emptying_the_trash_asks_about_the_number_of_items_with_cancel_lit() {
        let mut f = fixture();
        let held = |name: &str| f.trash.join(name);
        f.app.dialog = Some(Dialog::EmptyTrash {
            paths: vec![held("a.txt"), held("b.txt"), held("Folder")],
            armed: false,
        });
        let drawn = buffer(&mut f.app, 120, 30);
        let title = find(&drawn, "Empty the trash?", 0..120).expect("the title");
        assert_eq!(drawn[title].fg, theme::FOREGROUND);
        assert!(drawn[title].modifier.contains(Modifier::BOLD));
        assert_eq!(drawn[title].bg, Color::Reset);
        let asks = "3 items will be deleted permanently. This can't be undone.";
        assert!(find(&drawn, asks, 0..120).is_some());
        // The choice, not the title: it is followed by the gap before Cancel.
        let empty = find(&drawn, "Empty Trash  ", 0..120).expect("the Empty Trash choice");
        let cancel = find(&drawn, "Cancel", 0..120).expect("the Cancel choice");
        assert_eq!(empty.1, cancel.1, "side by side");
        assert_eq!(drawn[cancel].bg, theme::WASH, "Cancel is what Enter takes");
        assert_ne!(drawn[empty].bg, theme::WASH);
        assert_eq!(find(&drawn, "Delete for good?", 0..120), None);

        press(&mut f.app, &[KeyCode::Right]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[empty].bg, theme::WASH);
        assert_ne!(drawn[cancel].bg, theme::WASH);

        // One item is one item.
        f.app.dialog = Some(Dialog::EmptyTrash {
            paths: vec![held("a.txt")],
            armed: false,
        });
        let drawn = buffer(&mut f.app, 120, 30);
        let asks = "1 item will be deleted permanently. This can't be undone.";
        assert!(find(&drawn, asks, 0..120).is_some());
    }

    #[test]
    fn a_small_window_still_shows_the_dialog() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(shift(KeyCode::Delete));
        let drawn = buffer(&mut f.app, 30, 8);
        assert!(find(&drawn, "Delete for good?", 0..30).is_some());
        buffer(&mut f.app, 3, 2);
    }
}
