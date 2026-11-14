//! flea's menu: a framed box under the item it is about, at its name
//! column (above it when the body has no room below), with a ` glyph
//! label` row for each thing to do, rules between the groups, and a `›`
//! on the rows that open a submenu, which is titled in muted capitals.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{fit, pad};
use crate::app::{Action, HitMap, Menu, MenuRow, Spot};
use crate::icons;
use crate::theme;

// The marks for the menu's verbs: filled Material Design glyphs, like the
// rest of the app's icons.
const OPEN: &str = "\u{f03cc}";
const WINDOW: &str = "\u{f0614}";
const EYE: &str = "\u{f0208}";
const EYE_CLOSED: &str = "\u{f0209}";
const ARCHIVE: &str = "\u{f003c}";
const FILE: &str = "\u{f0214}";
const TAG: &str = "\u{f04f9}";
const COPY: &str = "\u{f018f}";
const EDIT: &str = "\u{f03eb}";
const FILES: &str = "\u{f0222}";
const TRASH: &str = "\u{f0a79}";
const RESTORE: &str = "\u{f099b}";
const CHEVRON: &str = "\u{f0142}";

/// The `›` at the end of a row that opens a submenu.
pub fn chevron() -> &'static str {
    icons::mark(CHEVRON, "›")
}

/// The mark before a row's label: its verb's glyph, or a blank without
/// the Nerd Font glyphs.
fn glyph(action: Action, label: &str) -> &'static str {
    let glyph = match action {
        Action::Open => OPEN,
        Action::OpenWith | Action::DefaultApp => WINDOW,
        Action::QuickLook => EYE,
        Action::ToggleHidden if label.starts_with("Hide") => EYE_CLOSED,
        Action::ToggleHidden => EYE,
        Action::Compress | Action::Extract => ARCHIVE,
        Action::CompressAs(_) => FILE,
        Action::Tags | Action::Tag(_) | Action::RemoveTags => TAG,
        Action::CopyPath => COPY,
        Action::Rename => EDIT,
        Action::Duplicate => FILES,
        Action::Trash | Action::Delete | Action::OpenTrash | Action::EmptyTrash => TRASH,
        Action::Restore => RESTORE,
    };
    icons::mark(glyph, " ")
}

/// The cells a menu's rows need inside its frame: two of air, the mark and
/// its space, the longest label and, on a row that opens a submenu, room
/// for the `›`. A submenu is at least as wide as the menu it opened from.
pub fn width(menu: &Menu) -> u16 {
    let row = |row: &MenuRow| match row {
        MenuRow::Item { label, action, .. } => {
            let opens = matches!(action, Action::OpenWith | Action::Compress | Action::Tags);
            2 + label.width() + if opens { 3 } else { 0 }
        }
        MenuRow::Separator => 0,
    };
    let longest = menu.rows.iter().map(row).max().unwrap_or(0);
    let title = menu.sub.title().map_or(0, UnicodeWidthStr::width);
    let own = (2 + longest.max(title)) as u16;
    let parent = menu.parent.as_deref().map_or(0, width);
    own.max(parent)
}

/// Draws the menu under `anchor`, the cells of the name of the item it is
/// about, inside `body`, the rows between the rules of the window. The
/// menu and each of its lines are spots for the mouse.
pub fn draw(frame: &mut Frame, menu: &Menu, anchor: Rect, body: Rect, hits: &mut HitMap) {
    let screen = frame.area();
    let inside = width(menu).min(screen.width.saturating_sub(2)).max(1);
    let title = menu.sub.title();
    let wanted = menu.rows.len() + usize::from(title.is_some());
    let shown = (wanted as u16).min(body.height.saturating_sub(2)).max(1);
    let outer = place(anchor, inside + 2, shown + 2, body, screen);
    if outer.width < 3 || outer.height < 3 {
        return;
    }
    frame.render_widget(Clear, outer);
    // The lines go on top of the menu's own spot.
    hits.add(outer, Spot::Menu);
    let inside = outer.width - 2;
    let cells = inside as usize;
    let mut lines = Vec::new();
    let edge = |left: &str, right: &str| {
        let line = format!("{left}{}{right}", "─".repeat(cells));
        Line::from(Span::styled(line, theme::frame()))
    };
    lines.push(edge("┌", "┐"));
    let mut rows = (outer.height - 2) as usize;
    if let Some(title) = title {
        let text = pad(title, cells.saturating_sub(2));
        lines.push(framed(vec![Span::styled(
            format!(" {text} "),
            theme::muted(),
        )]));
        rows = rows.saturating_sub(1);
    }
    // A menu taller than the body shows the rows around the keyboard.
    let top = (menu.cursor + 1)
        .saturating_sub(rows)
        .min(menu.rows.len().saturating_sub(rows));
    let first_row = outer.y + 1 + u16::from(title.is_some());
    for (index, row) in menu.rows.iter().enumerate().skip(top).take(rows) {
        lines.push(framed(spans(row, index == menu.cursor, cells)));
        if matches!(row, MenuRow::Item { .. }) {
            let y = first_row + (index - top) as u16;
            hits.add(Rect::new(outer.x + 1, y, inside, 1), Spot::MenuRow(index));
        }
    }
    lines.push(edge("└", "┘"));
    frame.render_widget(Paragraph::new(lines), outer);
}

/// A row between the frame's two sides.
fn framed(inner: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled("│", theme::frame())];
    spans.extend(inner);
    spans.push(Span::styled("│", theme::frame()));
    Line::from(spans)
}

/// A row's cells inside the frame: ` mark label ›` on the tab fill where
/// the keyboard is; a rule between groups.
fn spans(row: &MenuRow, lit: bool, cells: usize) -> Vec<Span<'static>> {
    let MenuRow::Item {
        label,
        action,
        enabled,
        tag,
    } = row
    else {
        let rule = "─".repeat(cells.saturating_sub(2));
        return vec![Span::styled(format!(" {rule} "), theme::card_rule())];
    };
    let (mark_ink, ink) = if !enabled {
        (theme::MUTED, theme::MUTED)
    } else if *action == Action::Delete {
        (theme::ERROR, theme::ERROR)
    } else {
        (theme::MUTED, theme::FOREGROUND)
    };
    let fill = |color| {
        let style = theme::fg(color).add_modifier(theme::cursor_modifier(lit));
        if lit {
            style.bg(theme::ink(theme::TAB))
        } else {
            style
        }
    };
    let opens = matches!(action, Action::OpenWith | Action::Compress | Action::Tags);
    let trail = if opens { 2 } else { 0 };
    let room = cells.saturating_sub(4 + trail);
    let (mark, mark_style): (String, Style) = match tag {
        Some(index) => {
            let (glyph, color) = theme::tag_mark(*index);
            (glyph, fill(mark_ink).fg(color))
        }
        None => (glyph(*action, label).to_string(), fill(mark_ink)),
    };
    let mut spans = vec![
        Span::styled(" ", fill(ink)),
        Span::styled(fit(&mark, 1), mark_style),
        Span::styled(" ", fill(ink)),
        Span::styled(pad(label, room), fill(ink)),
    ];
    if opens {
        spans.push(Span::styled(" ", fill(ink)));
        spans.push(Span::styled(chevron(), fill(theme::MUTED)));
    }
    spans.push(Span::styled(" ", fill(ink)));
    spans
}

/// Where a menu of a size goes: right under the item it is about; above it
/// when the body has no room below; and with room in neither place, as low
/// as it fits in the body. Never past the screen's right edge.
fn place(anchor: Rect, width: u16, height: u16, body: Rect, screen: Rect) -> Rect {
    let x = anchor.x.min(screen.right().saturating_sub(width));
    let row = anchor.y;
    let y = if row + 1 + height <= body.bottom() {
        row + 1
    } else if row >= body.y + height {
        row - height
    } else {
        body.bottom().saturating_sub(height).max(body.y)
    };
    Rect::new(x, y, width, height).intersection(screen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Sub;
    use crate::app::testing::{ctrl, fixture, grid_fixture, key, press};
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;
    use std::fs;

    /// The cells of one row from one column to another, as text.
    fn cells(drawn: &Buffer, y: u16, columns: std::ops::Range<u16>) -> String {
        columns.map(|x| drawn[(x, y)].symbol()).collect()
    }

    #[test]
    fn the_menu_is_a_frame_under_the_row_at_the_name_with_glyphs_rules_and_chevrons() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(key(KeyCode::Char('m')));
        let drawn = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&drawn, "notes.txt", 26..73).expect("the item");
        // The frame in the frame colour, on the terminal's own background,
        // from the name's column, right under the row.
        let corner = (x, y + 1);
        assert_eq!(drawn[corner].symbol(), "┌");
        assert_eq!(drawn[corner].fg, theme::MUTED);
        assert_eq!(drawn[corner].bg, Color::Reset);
        // 23 cells inside: the longest label, Delete Permanently…, with
        // two cells of air, the mark and the space after it.
        assert_eq!(drawn[(x + 24, y + 1)].symbol(), "┐");
        assert_eq!(drawn[(x, y + 2)].symbol(), "│");
        assert_eq!(drawn[(x, y + 2)].fg, theme::MUTED);

        // ` glyph label`: the first row has the keyboard, on the tab fill.
        let open = (x + 4, y + 2);
        assert_eq!(find(&drawn, "Open ", x..x + 24), Some(open));
        assert_eq!(
            (drawn[open].fg, drawn[open].bg),
            (theme::FOREGROUND, theme::TAB)
        );
        let glyph = (x + 2, y + 2);
        assert_eq!(drawn[glyph].symbol(), OPEN);
        assert_eq!(
            (drawn[glyph].fg, drawn[glyph].bg),
            (theme::MUTED, theme::TAB)
        );
        assert_eq!(drawn[(x + 1, y + 2)].bg, theme::TAB);
        assert_eq!(drawn[(x + 23, y + 2)].bg, theme::TAB, "the whole row");

        // Open With opens a submenu: a `›` near its end.
        let with = (x + 4, y + 3);
        assert_eq!(cells(&drawn, y + 3, with.0..with.0 + 9), "Open With");
        assert_eq!(
            (drawn[with].fg, drawn[with].bg),
            (theme::FOREGROUND, Color::Reset)
        );
        assert_eq!(drawn[(x + 22, y + 3)].symbol(), CHEVRON);
        assert_eq!(drawn[(x + 22, y + 3)].fg, theme::MUTED);
        // A rule between the groups, in the card's rule colour.
        assert_eq!(
            cells(&drawn, y + 4, x + 1..x + 24),
            format!(" {} ", "─".repeat(21))
        );
        assert_eq!(drawn[(x + 2, y + 4)].fg, theme::CARD_RULE);
        assert_eq!(cells(&drawn, y + 5, x + 4..x + 12), "Compress");
        assert_eq!(drawn[(x + 22, y + 5)].symbol(), CHEVRON);
        // Delete Permanently… in the error colour, its glyph too.
        let delete = find(&drawn, "Delete Permanently…", x..x + 24).expect("the row");
        assert_eq!(delete, (x + 4, y + 2 + 12));
        assert_eq!(drawn[delete].fg, theme::ERROR);
        assert_eq!(drawn[(x + 2, delete.1)].fg, theme::ERROR);
        assert_eq!(drawn[(x + 2, delete.1)].symbol(), TRASH);
        let last = find(&drawn, "Open Trash", x..x + 24).expect("the last row");
        assert_eq!(last, (x + 4, y + 2 + 15));
        assert_eq!(drawn[(x, last.1 + 1)].symbol(), "└");

        // `j` moves the fill along, over the rule.
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char('j')]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(x + 4, y + 5)].bg, theme::TAB);
        assert_eq!(drawn[open].bg, Color::Reset);

        // Closed, it is gone.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "Open With", 0..120), None);
    }

    #[test]
    fn a_dim_row_is_muted_with_its_chevron_wherever_the_keyboard_is() {
        let mut f = crate::app::testing::several();
        press(&mut f.app, &[KeyCode::Char('J')]);
        f.app.handle_key(key(KeyCode::Char('m')));
        let drawn = buffer(&mut f.app, 120, 30);
        let with = find(&drawn, "Open With", 26..73).expect("the row");
        assert_eq!(drawn[with].fg, theme::MUTED);
        assert_eq!(drawn[(with.0 - 2, with.1)].fg, theme::MUTED);
        let rename = find(&drawn, "Rename", 26..73).expect("the row");
        assert_eq!(
            drawn[rename].fg,
            theme::FOREGROUND,
            "several are renamed too"
        );
        press(&mut f.app, &[KeyCode::Down]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!((drawn[with].fg, drawn[with].bg), (theme::MUTED, theme::TAB));
        assert_eq!(drawn[(with.0 + 18, with.1)].symbol(), CHEVRON);
    }

    #[test]
    fn a_submenu_is_titled_in_muted_capitals_and_as_wide_as_the_menu() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End]);
        f.app.handle_key(key(KeyCode::Char('m')));
        let root = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&root, "┌", 26..73).expect("the menu");
        press(
            &mut f.app,
            &[KeyCode::Char('j'), KeyCode::Char('j'), KeyCode::Char('l')],
        );
        assert_eq!(
            f.app.menu.as_ref().map(|menu| menu.sub),
            Some(Sub::Compress)
        );
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(x, y)].symbol(), "┌", "where the menu was");
        assert_eq!(drawn[(x + 24, y)].symbol(), "┐", "as wide");
        let title = (x + 2, y + 1);
        assert_eq!(cells(&drawn, y + 1, title.0..title.0 + 8), "COMPRESS");
        assert_eq!(drawn[title].fg, theme::MUTED);
        assert_eq!(drawn[title].bg, Color::Reset);
        let zip = find(&drawn, ".zip", x..x + 24).expect("the first format");
        assert_eq!(zip, (x + 4, y + 2));
        assert_eq!(drawn[zip].bg, theme::TAB);
        assert_eq!(drawn[(x + 2, y + 2)].symbol(), FILE);
        let xz = find(&drawn, ".tar.xz", x..x + 24).expect("the last format");
        assert_eq!(drawn[(x, xz.1 + 1)].symbol(), "└");

        // `h` goes back to the menu as it was.
        press(&mut f.app, &[KeyCode::Char('h')]);
        assert_eq!(buffer(&mut f.app, 120, 30), {
            let mut back = root.clone();
            // The keyboard is on Compress now.
            for column in x + 1..x + 24 {
                back[(column, y + 1)].bg = Color::Reset;
                back[(column, y + 4)].bg = theme::TAB;
            }
            back
        });
    }

    #[test]
    fn a_menu_with_no_room_below_opens_above_the_row_and_one_too_tall_scrolls() {
        let mut f = fixture();
        for n in 10..50 {
            fs::write(f.home.join(format!("{n}.txt")), "").unwrap();
        }
        press(&mut f.app, &[KeyCode::F(5), KeyCode::End]);
        f.app.handle_key(key(KeyCode::Char('m')));
        let drawn = buffer(&mut f.app, 120, 40);
        let (x, y) = find(&drawn, "notes.txt", 26..73).expect("the item");
        assert!(y > 30, "near the bottom");
        assert_eq!(drawn[(x, y - 1)].symbol(), "└");
        assert_eq!(find(&drawn, "Open Trash", 26..73), Some((x + 4, y - 2)));
        assert_eq!(find(&drawn, "Open ", 26..73), Some((x + 4, y - 17)));
        assert_eq!(drawn[(x, y - 18)].symbol(), "┌");
        press(&mut f.app, &[KeyCode::Esc]);

        // A body with room neither above nor below: as low as it fits,
        // inside the body, the rows around the keyboard showing.
        press(&mut f.app, &[KeyCode::Home, KeyCode::Char('m')]);
        let short = buffer(&mut f.app, 120, 14);
        let lines = screen(&mut f.app, 120, 14);
        let top = lines.iter().position(|line| line.contains('┌')).unwrap();
        let bottom = lines.iter().position(|line| line.contains('└')).unwrap();
        assert_eq!((top, bottom), (2, 11), "the whole body, no more");
        assert!(find(&short, "Open ", 26..73).is_some());
        assert_eq!(find(&short, "Open Trash", 26..73), None);
        press(&mut f.app, &[KeyCode::Down; 30]);
        let short = buffer(&mut f.app, 120, 14);
        let last = find(&short, "Open Trash", 26..73).expect("the last row");
        assert_eq!(short[last].bg, theme::TAB);
        assert_eq!(find(&short, "Open With", 26..73), None);

        // And one too small for anything is still drawn.
        for (width, height) in [(3, 2), (20, 5), (40, 6), (12, 5)] {
            buffer(&mut f.app, width, height);
        }
    }

    #[test]
    fn in_the_grid_and_in_columns_the_menu_opens_at_the_selected_item() {
        let mut f = grid_fixture();
        f.app.handle_key(key(KeyCode::Char('m')));
        let first = buffer(&mut f.app, 62, 26);
        let at_first = find(&first, "Open With", 0..62).expect("the menu");
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Right]);
        f.app.handle_key(key(KeyCode::Char('m')));
        let second = buffer(&mut f.app, 62, 26);
        let at_second = find(&second, "Open With", 0..62).expect("the menu");
        // One tile further right, and so is the menu.
        assert_eq!(at_second.0, at_first.0 + 20);
        assert_eq!(at_second.1, at_first.1);
        press(&mut f.app, &[KeyCode::Esc]);

        // In columns it is in the column of the folder that is showing.
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, f.home.join("Pictures"));
        f.app.handle_key(key(KeyCode::Char('m')));
        let columns = buffer(&mut f.app, 120, 30);
        let (name_x, name_y) = find(&columns, "Trip", 0..120).expect("the item");
        assert!(name_x > 27, "in the second column");
        let open = find(&columns, "Open With", 0..120).expect("the menu");
        assert_eq!(open, (name_x + 4, name_y + 3));
    }

    #[test]
    fn the_tags_submenu_shows_each_tag_with_its_colour_square() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::Char('T')]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "TAGS", 26..120).is_some());
        for (index, name) in crate::tags::NAMES.iter().enumerate() {
            let (x, y) = find(&drawn, name, 26..120).expect("the tag's line");
            assert_eq!(drawn[(x - 2, y)].symbol(), "■", "{name}");
            assert_eq!(drawn[(x - 2, y)].fg, theme::TAGS[index], "{name}");
        }
        assert_eq!(find(&drawn, "Remove All Tags", 26..120), None);
    }

    #[test]
    fn the_menu_goes_below_the_row_then_above_it_then_as_low_as_the_body_allows() {
        let screen = Rect::new(0, 0, 80, 24);
        let body = Rect::new(0, 2, 80, 20);
        let row = |x, y| Rect::new(x, y, 0, 1);
        assert_eq!(
            place(row(30, 5), 20, 10, body, screen),
            Rect::new(30, 6, 20, 10)
        );
        assert_eq!(
            place(row(30, 11), 20, 10, body, screen),
            Rect::new(30, 12, 20, 10)
        );
        assert_eq!(
            place(row(30, 12), 20, 10, body, screen),
            Rect::new(30, 2, 20, 10)
        );
        assert_eq!(
            place(row(30, 9), 20, 16, body, screen),
            Rect::new(30, 6, 20, 16)
        );
        // Never past the right edge.
        assert_eq!(
            place(row(70, 5), 20, 10, body, screen),
            Rect::new(60, 6, 20, 10)
        );
    }
}
