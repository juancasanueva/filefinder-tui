//! Dialogs: a name being typed, and the question before deleting for good.
//! They look like the `?` overlay: a dark panel with a hairline border.

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

use super::{fit, pad};
use crate::app::Dialog;
use crate::input::Input;
use crate::ops;
use crate::theme;

const WIDTH: u16 = 72;

pub fn draw(frame: &mut Frame, dialog: &Dialog, area: Rect) {
    match dialog {
        Dialog::Rename { input, .. } => name(frame, area, "Rename", input),
        Dialog::NewFolder { input, .. } => name(frame, area, "New Folder", input),
        Dialog::Delete { paths, armed } => confirm(frame, area, paths, *armed),
    }
}

/// Clears a panel in the middle of the window and returns where its `rows`
/// lines of content go.
fn panel(frame: &mut Frame, area: Rect, rows: u16) -> Rect {
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
        .border_style(theme::rule())
        .padding(Padding::new(3, 3, air, air))
        .style(theme::panel());
    let inner = block.inner(outer);
    frame.render_widget(Clear, outer);
    frame.render_widget(block, outer);
    inner
}

/// A title over a field holding the name being typed.
fn name(frame: &mut Frame, area: Rect, title: &str, input: &Input) {
    let inner = panel(frame, area, 5);
    let [title_row, _, field, _, hint] = Layout::vertical([Constraint::Length(1); 5]).areas(inner);
    frame.render_widget(Span::styled(title, theme::title()), title_row);
    frame.render_widget(Span::styled("⏎ Apply    Esc Cancel", theme::dim()), hint);
    if field.is_empty() {
        return;
    }
    let (text, column) = window(input.chars(), input.cursor(), field.width as usize);
    let typed = Paragraph::new(pad(&text, field.width as usize)).style(theme::field());
    frame.render_widget(typed, field);
    frame.set_cursor_position(Position::new(field.x + column, field.y));
}

/// The question before deleting for good, with its two choices.
fn confirm(frame: &mut Frame, area: Rect, paths: &[PathBuf], armed: bool) {
    let inner = panel(frame, area, 6);
    let rows = [1, 1, 2, 1, 1].map(Constraint::Length);
    let [title, _, text, _, choices] = Layout::vertical(rows).areas(inner);
    let what = match paths {
        [one] => {
            let name = one.file_name().unwrap_or(one.as_os_str());
            fit(&name.to_string_lossy(), 40)
        }
        several => ops::count(several.len()),
    };
    let asks = format!("{what} will be deleted permanently. This can't be undone.");
    frame.render_widget(Span::styled("Delete for good?", theme::title()), title);
    frame.render_widget(Paragraph::new(asks).wrap(Wrap { trim: true }), text);
    let line = Line::from(vec![
        Span::styled("  Delete  ", theme::choice(armed)),
        Span::raw("  "),
        Span::styled("  Cancel  ", theme::choice(!armed)),
    ]);
    frame.render_widget(line, choices);
}

/// The part of a field's text that fits in `width` cells with the cursor in
/// sight, and the cell the cursor is on.
fn window(chars: &[char], cursor: usize, width: usize) -> (String, u16) {
    let cells = |c: &char| c.width().unwrap_or(0);
    let mut before: usize = chars[..cursor].iter().map(cells).sum();
    // Start late enough for the cursor's own cell to be inside.
    let mut start = 0;
    while before >= width && start < cursor {
        before -= cells(&chars[start]);
        start += 1;
    }
    let mut text = String::new();
    let mut used = 0;
    for c in &chars[start..] {
        used += cells(c);
        if used > width {
            break;
        }
        text.push(*c);
    }
    (text, before as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{ctrl_shift, fixture, press, shift, type_text};
    use crate::ui::testing::{buffer, cursor, find};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    /// Where a piece of text starts in the rows under `top`, left of the details pane.
    fn under(drawn: &Buffer, text: &str, top: u16) -> Option<(u16, u16)> {
        let cells: Vec<String> = text.chars().map(String::from).collect();
        (top + 1..drawn.area.height).find_map(|y| {
            (0..86u16.saturating_sub(cells.len() as u16))
                .find(|x| {
                    cells
                        .iter()
                        .zip(*x..)
                        .all(|(cell, x)| drawn[(x, y)].symbol() == cell)
                })
                .map(|x| (x, y))
        })
    }

    #[test]
    fn the_rename_dialog_shows_the_name_with_the_cursor_before_the_extension() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::F(2)]);
        let drawn = buffer(&mut f.app, 120, 30);
        let (title_x, title_y) = find(&drawn, "Rename", 0..120).expect("the title");
        assert_eq!(drawn[(title_x, title_y)].fg, theme::BRIGHT_FOREGROUND);
        assert_eq!(drawn[(title_x, title_y)].bg, theme::DARK_BACKGROUND);
        let (x, y) = under(&drawn, "notes.txt", title_y).expect("the name");
        assert_eq!(drawn[(x, y)].bg, theme::LIGHTER_BACKGROUND, "in a field");
        assert_eq!(cursor(&mut f.app, 120, 30), (x + 5, y));

        // Typing shows up where the cursor is, and the cursor moves along.
        type_text(&mut f.app, "-2");
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(under(&drawn, "notes-2.txt", title_y), Some((x, y)));
        assert_eq!(cursor(&mut f.app, 120, 30), (x + 7, y));
        // Closed, the panel is gone.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "Rename", 0..120), None);
    }

    #[test]
    fn the_new_folder_dialog_starts_with_a_name_to_type_over() {
        let mut f = fixture();
        f.app.handle_key(ctrl_shift('n'));
        let drawn = buffer(&mut f.app, 120, 30);
        // Once as the title, once as the name in the field under it.
        let (title_x, title_y) = find(&drawn, "New Folder", 0..120).expect("the title");
        let (x, y) = under(&drawn, "New Folder", title_y).expect("the name");
        assert_eq!(x, title_x);
        assert_eq!(drawn[(x, y)].bg, theme::LIGHTER_BACKGROUND, "in a field");
        assert_eq!(cursor(&mut f.app, 120, 30), (x + 10, y), "after the name");
    }

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
        assert_eq!(
            drawn[cancel].bg,
            theme::ACCENT,
            "Cancel is what Enter takes"
        );
        assert_ne!(drawn[delete].bg, theme::ACCENT);

        press(&mut f.app, &[KeyCode::Right]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[delete].bg, theme::ACCENT);
        assert_ne!(drawn[cancel].bg, theme::ACCENT);
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
    fn a_name_longer_than_its_field_scrolls_to_keep_the_cursor_in_sight() {
        let name: Vec<char> = "abcdefghij".chars().collect();
        assert_eq!(window(&name, 10, 20), ("abcdefghij".to_string(), 10));
        assert_eq!(window(&name, 3, 6), ("abcdef".to_string(), 3));
        // The cursor after the last letter needs a cell of its own.
        assert_eq!(window(&name, 10, 6), ("fghij".to_string(), 5));
        assert_eq!(window(&name, 7, 6), ("cdefgh".to_string(), 5));
        // Wide letters take two cells each.
        let wide: Vec<char> = "日本語のファイル".chars().collect();
        assert_eq!(window(&wide, 8, 7), ("ァイル".to_string(), 6));
        assert_eq!(window(&wide, 0, 7), ("日本語".to_string(), 0));
    }

    #[test]
    fn a_small_window_still_shows_the_dialog() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::F(2)]);
        let drawn = buffer(&mut f.app, 30, 8);
        assert!(find(&drawn, "Rename", 0..30).is_some());
        buffer(&mut f.app, 3, 2);
    }
}
