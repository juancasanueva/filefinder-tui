//! Cards: a frame in the frame colour with `┌┐└┘` corners, two
//! cells of air inside each side, a bold title with a muted "esc" at its
//! end, a rule, the body and `[ Label ]` buttons at the bottom right. While
//! one is open the rest of the screen is dimmed behind it.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{fit, pad, words};
use crate::app::{Dialog, HitMap, Spot};
use crate::format;
use crate::theme;

/// Cells of air inside each side of a card.
const AIR: u16 = 2;
/// What a card says at the end of its title row.
pub const ESC: &str = "esc";

/// Two cells of air inside each side, none on a screen under 30 columns so
/// that the text keeps the room.
pub fn air(columns: u16) -> u16 {
    if columns >= 30 { AIR } else { 0 }
}

/// The widest text a card can hold on a screen this wide.
pub fn room(columns: u16) -> u16 {
    columns.saturating_sub(2 + 2 * air(columns)).max(1)
}

/// Where a card's parts go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// The frame, corners included.
    pub outer: Rect,
    /// Where the body's rows go, inside the air.
    pub body: Rect,
    /// The first row of the body that shows.
    pub top: usize,
    /// Each button's cells, in the order given.
    pub buttons: Vec<Rect>,
}

impl Card {
    /// The cells inside the frame and the air: the title, the rule and the body.
    pub fn content(&self) -> Rect {
        Rect {
            y: self.outer.y + 1,
            height: self.outer.height.saturating_sub(2),
            ..self.body
        }
    }
}

/// A card in the middle of `area`, `need` cells of text wide (as far as the
/// screen allows), with `rows` rows of body scrolled `scroll` rows down and
/// buttons of these widths on its bottom row, right-aligned two cells
/// apart. A body taller than the screen shows as much as fits.
pub fn layout(area: Rect, need: u16, rows: usize, buttons: &[u16], scroll: usize) -> Card {
    let air = air(area.width);
    let content = need.min(room(area.width));
    let button_rows = u16::from(!buttons.is_empty());
    // The title and the rule, then a blank row and the buttons.
    let fixed = 2 + 2 * button_rows;
    let shown = (rows as u16).min(area.height.saturating_sub(2 + fixed));
    let (width, height) = (content + 2 * air + 2, fixed + shown + 2);
    let outer = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    };
    placed(
        outer,
        air,
        shown,
        buttons,
        rows.saturating_sub(shown as usize).min(scroll),
    )
}

/// A card whose frame is `outer`, its body `shown` rows tall from `top`.
pub fn placed(outer: Rect, air: u16, shown: u16, buttons: &[u16], top: usize) -> Card {
    let body = Rect {
        x: outer.x + 1 + air,
        y: outer.y + 3,
        width: outer.width.saturating_sub(2 + 2 * air),
        height: shown,
    };
    let row = body.y + shown + 1;
    let mut right = body.right();
    let mut cells = Vec::new();
    for width in buttons.iter().rev() {
        let width = (*width).min(body.width);
        right = right.saturating_sub(width);
        cells.insert(0, Rect::new(right, row, width, 1));
        right = right.saturating_sub(2);
    }
    Card {
        outer,
        body,
        top,
        buttons: cells,
    }
}

/// Draws the frame on a cleared ground, the bold title with "esc" at the
/// end of its row, and the rule under them.
pub fn draw_frame(frame: &mut Frame, card: &Card, title: &str) {
    let outer = card.outer;
    if outer.width < 2 || outer.height < 2 {
        return;
    }
    frame.render_widget(Clear, outer);
    let inside = outer.width - 2;
    let edge = |left: &str, right: &str| format!("{left}{}{right}", "─".repeat(inside as usize));
    let mut lines = vec![Line::from(Span::styled(edge("┌", "┐"), theme::frame()))];
    for _ in 2..outer.height {
        lines.push(Line::from(vec![
            Span::styled("│", theme::frame()),
            Span::styled(" ".repeat(inside as usize), theme::base()),
            Span::styled("│", theme::frame()),
        ]));
    }
    lines.push(Line::from(Span::styled(edge("└", "┘"), theme::frame())));
    frame.render_widget(Paragraph::new(lines), outer);

    let content = card.content();
    if content.height == 0 || content.width == 0 {
        return;
    }
    let width = content.width as usize;
    let room = width.saturating_sub(ESC.width() + 2);
    let title_line = if room > 0 {
        Line::from(vec![
            Span::styled(pad(title, room), theme::title()),
            Span::raw("  "),
            Span::styled(ESC, theme::muted()),
        ])
    } else {
        Line::from(Span::styled(fit(title, width), theme::title()))
    };
    frame.render_widget(
        title_line,
        Rect {
            height: 1,
            ..content
        },
    );
    if content.height > 1 {
        let rule = Span::styled("─".repeat(width), theme::card_rule());
        frame.render_widget(rule, Rect::new(content.x, content.y + 1, content.width, 1));
    }
}

/// A button: its label, whether it is the default (what the card does
/// unless asked otherwise) and whether the keyboard is on it.
pub struct Button<'a> {
    pub label: &'a str,
    pub default: bool,
    pub focused: bool,
}

/// How many cells a button takes: `[ Label ]`.
pub fn button_width(label: &str) -> u16 {
    label.width() as u16 + 4
}

/// Draws the buttons in their cells: the brackets in the foreground on the
/// one the keyboard is on, in the accent on the default and in the frame
/// colour on the others; the default's label on the wash.
pub fn buttons(frame: &mut Frame, card: &Card, buttons: &[Button]) {
    for (cells, button) in card.buttons.iter().zip(buttons) {
        let bracket = if button.focused {
            theme::base()
        } else if button.default {
            theme::fg(theme::ACCENT)
        } else {
            theme::frame()
        };
        let bracket = bracket.add_modifier(theme::cursor_modifier(button.focused));
        let label = if button.default {
            theme::on(theme::FOREGROUND, theme::WASH)
        } else {
            theme::base()
        };
        let inner = (cells.width as usize).saturating_sub(4);
        let line = Line::from(vec![
            Span::styled("[", bracket),
            Span::styled(format!(" {} ", fit(button.label, inner)), label),
            Span::styled("]", bracket),
        ]);
        frame.render_widget(line, *cells);
    }
}

/// Everything already drawn in `area`, mixed halfway to the ground: what
/// is behind a card.
pub fn dim(buffer: &mut Buffer, area: Rect) {
    let area = area.intersection(buffer.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buffer[(x, y)];
            let style = theme::dimmed(Style::new().fg(cell.fg).bg(cell.bg));
            cell.set_style(style);
            if theme::depth() == theme::Depth::Mono {
                cell.modifier.insert(Modifier::DIM);
            }
        }
    }
}

/// Draws the card that asks, and keeps where it and its buttons are.
pub fn draw_dialog(frame: &mut Frame, dialog: &Dialog, area: Rect, hits: &mut HitMap) {
    let (title, body, yes) = asked(dialog);
    let widths = [button_width("Cancel"), button_width(yes)];
    let room = room(area.width) as usize;
    let rows = words(&body, body.width().min(room));
    let text = rows.iter().map(|row| row.width()).max().unwrap_or(0);
    let heading = title.width() + 2 + ESC.width();
    let need = text.max(heading).max((widths[0] + widths[1] + 2) as usize);
    let card = layout(area, need as u16, rows.len(), &widths, 0);
    draw_frame(frame, &card, &title);
    let shown = rows.iter().skip(card.top).take(card.body.height as usize);
    let lines: Vec<Line> = shown
        .map(|row| Line::from(Span::styled(row.clone(), theme::base())))
        .collect();
    frame.render_widget(Paragraph::new(lines), card.body);
    let armed = dialog.armed();
    let labels = [
        Button {
            label: "Cancel",
            default: true,
            focused: !armed,
        },
        Button {
            label: yes,
            default: false,
            focused: armed,
        },
    ];
    buttons(frame, &card, &labels);
    hits.add(card.outer, Spot::Dialog);
    for (cells, yes) in card.buttons.iter().zip([false, true]) {
        hits.add(*cells, Spot::Choice(yes));
    }
}

/// The title, the body and the destructive button of a card that asks.
fn asked(dialog: &Dialog) -> (String, String, &'static str) {
    match dialog {
        Dialog::Delete { paths, .. } => {
            let (title, body) = match paths.len() {
                1 => (
                    "Delete 1 item permanently?".to_string(),
                    "This item is deleted from disk. This cannot be undone.",
                ),
                many => (
                    format!("Delete {many} items permanently?"),
                    "These items are deleted from disk. This cannot be undone.",
                ),
            };
            (title, body.to_string(), "Delete")
        }
        Dialog::EmptyTrash { paths, bytes, .. } => {
            let size = format::size(*bytes);
            let body = match paths.len() {
                1 => format!("1 item, {size}. This deletes it from disk. This cannot be undone."),
                many => format!(
                    "{many} items, {size}. This deletes them from disk. This cannot be undone."
                ),
            };
            ("Empty Trash?".to_string(), body, "Empty Trash")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{fixture, press, several, shift};
    use crate::ui::testing::{buffer, find, panes};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;

    #[test]
    fn the_delete_card_is_centred_framed_titled_with_cancel_its_default() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(shift(KeyCode::Delete));
        let drawn = buffer(&mut f.app, 120, 30);
        let title = "Delete 1 item permanently?";
        let (x, y) = find(&drawn, title, 0..120).expect("the title");
        assert_eq!(drawn[(x, y)].fg, theme::FOREGROUND);
        assert!(drawn[(x, y)].modifier.contains(Modifier::BOLD));
        // The body is the widest text: the card is it, two cells of air
        // each side and the frame, in the middle of the window.
        let body = "This item is deleted from disk. This cannot be undone.";
        assert_eq!(find(&drawn, body, 0..120), Some((x, y + 2)));
        let width = body.width() as u16 + 6;
        assert_eq!(x - 3, (120 - width) / 2);
        let (left, right) = (x - 3, x - 3 + width - 1);
        assert_eq!(drawn[(left, y - 1)].symbol(), "┌");
        assert_eq!(drawn[(right, y - 1)].symbol(), "┐");
        assert_eq!(drawn[(left, y + 5)].symbol(), "└");
        assert_eq!(drawn[(right, y + 5)].symbol(), "┘");
        assert_eq!(drawn[(left, y)].symbol(), "│");
        for corner in [(left, y - 1), (right, y + 5), (left, y)] {
            assert_eq!(drawn[corner].fg, theme::MUTED);
            assert_eq!(drawn[corner].bg, Color::Reset);
        }
        // "esc" muted at the end of the title row, then the rule.
        assert_eq!(find(&drawn, "esc", x..120), Some((right - 5, y)));
        assert_eq!(drawn[(right - 5, y)].fg, theme::MUTED);
        assert_eq!(drawn[(x, y + 1)].symbol(), "─");
        assert_eq!(drawn[(right - 3, y + 1)].symbol(), "─");
        assert_eq!(drawn[(x, y + 1)].fg, theme::CARD_RULE);
        // Bracket buttons at the bottom right, two cells apart: Cancel has
        // the keyboard and is the default, on the wash.
        let delete = find(&drawn, "[ Delete ]", 0..120).expect("Delete");
        assert_eq!(delete, (right - 12, y + 4));
        let cancel = find(&drawn, "[ Cancel ]", 0..120).expect("Cancel");
        assert_eq!(cancel, (delete.0 - 12, y + 4));
        assert_eq!(drawn[cancel].fg, theme::FOREGROUND);
        assert_eq!(drawn[(cancel.0 + 2, y + 4)].bg, theme::WASH);
        assert_eq!(drawn[(cancel.0 + 9, y + 4)].bg, Color::Reset);
        assert_eq!(drawn[delete].fg, theme::MUTED);
        assert_eq!(drawn[(delete.0 + 2, y + 4)].bg, Color::Reset);
        // Everything behind it is dimmed: the path, the item's row.
        assert_eq!(drawn[(1, 0)].fg, theme::DIMMED);
        let files = panes(&f.app, 120).files;
        let notes = find(&drawn, "notes.txt", files).expect("the item");
        assert_eq!(drawn[notes].fg, theme::DIMMED);
        assert_eq!(
            drawn[notes].bg,
            Color::Rgb(0x21, 0x24, 0x26),
            "the fill too"
        );

        // Right: the keyboard on Delete; Cancel is still the default.
        press(&mut f.app, &[KeyCode::Right]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[delete].fg, theme::FOREGROUND);
        assert_eq!(drawn[cancel].fg, theme::ACCENT);
        assert_eq!(drawn[(cancel.0 + 2, y + 4)].bg, theme::WASH);
    }

    #[test]
    fn deleting_several_items_says_how_many_and_enter_on_delete_deletes_just_them() {
        let mut f = several();
        press(
            &mut f.app,
            &[KeyCode::End, KeyCode::Char('K'), KeyCode::Char('K')],
        );
        f.app.handle_key(shift(KeyCode::Delete));
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "Delete 3 items permanently?", 0..120).is_some());
        let body = "These items are deleted from disk. This cannot be undone.";
        assert!(find(&drawn, body, 0..120).is_some());
        // Space on Cancel, the default, deletes nothing.
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert!(f.app.dialog.is_none());
        assert!(f.home.join("e.txt").exists());
        // Escape after moving to Delete deletes nothing either.
        f.app.handle_key(shift(KeyCode::Delete));
        press(&mut f.app, &[KeyCode::Tab, KeyCode::Esc]);
        assert!(f.home.join("e.txt").exists());
        // Enter on Delete deletes those three, and nothing else.
        f.app.handle_key(shift(KeyCode::Delete));
        press(&mut f.app, &[KeyCode::Char('l'), KeyCode::Enter]);
        for gone in ["d.txt", "e.txt", "notes.txt"] {
            assert!(!f.home.join(gone).exists(), "{gone}");
        }
        for kept in ["a.txt", "b.txt", "c.txt", "Documents", "Pictures"] {
            assert!(f.home.join(kept).exists(), "{kept}");
        }
        assert!(!f.trash.exists(), "deleted, not trashed");
    }

    #[test]
    fn the_empty_trash_card_says_how_many_items_and_what_they_take_up() {
        let mut f = fixture();
        let held = |name: &str| f.trash.join(name);
        f.app.dialog = Some(Dialog::EmptyTrash {
            paths: vec![held("a.txt"), held("b.txt"), held("Folder")],
            bytes: 2_500_000,
            armed: false,
        });
        let drawn = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&drawn, "Empty Trash?", 0..120).expect("the title");
        let size = format::size(2_500_000);
        let body = format!("3 items, {size}. This deletes them from disk. This cannot be undone.");
        assert_eq!(find(&drawn, &body, 0..120), Some((x, y + 2)));
        let empty = find(&drawn, "[ Empty Trash ]", 0..120).expect("the button");
        let cancel = find(&drawn, "[ Cancel ]", 0..120).expect("Cancel");
        assert_eq!((cancel.0 + 12, cancel.1), empty);
        assert_eq!(drawn[(cancel.0 + 2, cancel.1)].bg, theme::WASH);

        // One item is one item.
        f.app.dialog = Some(Dialog::EmptyTrash {
            paths: vec![held("a.txt")],
            bytes: 5,
            armed: false,
        });
        let drawn = buffer(&mut f.app, 120, 30);
        let body = format!(
            "1 item, {}. This deletes it from disk. This cannot be undone.",
            format::size(5)
        );
        assert!(find(&drawn, &body, 0..120).is_some());
    }

    #[test]
    fn under_30_columns_a_card_drops_its_air_and_a_long_body_wraps() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(shift(KeyCode::Delete));
        let drawn = buffer(&mut f.app, 29, 16);
        let (x, y) = find(&drawn, "Delete 1 item", 0..29).expect("the title, cut");
        assert_eq!(drawn[(x - 1, y)].symbol(), "│", "no air");
        assert_eq!(drawn[(x - 1, y)].fg, theme::MUTED);
        assert!(find(&drawn, "This item is deleted from", 0..29).is_some());
        assert!(find(&drawn, "[ Delete ]", 0..29).is_some());
        // One too small for anything is still drawn.
        for (width, height) in [(3, 2), (12, 5), (30, 6)] {
            buffer(&mut f.app, width, height);
        }
    }

    #[test]
    fn a_card_is_laid_out_by_its_rules() {
        let area = Rect::new(0, 0, 80, 24);
        let card = layout(area, 30, 2, &[10, 10], 0);
        // 30 cells of text, two of air each side and the frame: 36 by 8.
        assert_eq!(card.outer, Rect::new(22, 8, 36, 8));
        assert_eq!(card.body, Rect::new(25, 11, 30, 2));
        assert_eq!(
            card.buttons,
            [Rect::new(33, 14, 10, 1), Rect::new(45, 14, 10, 1)]
        );
        // A body taller than the window shows what fits.
        let tall = layout(area, 30, 100, &[10], 7);
        assert_eq!(tall.outer.height, 24);
        assert_eq!(tall.body.height, 18);
        assert_eq!(tall.top, 7);
        assert_eq!(air(29), 0);
        assert_eq!(room(29), 27);
        assert_eq!(room(80), 74);
    }
}
