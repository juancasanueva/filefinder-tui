//! The header: where the pane is on the left, the parent part muted and the
//! folder itself in the foreground, and the sort order on the right. The
//! path field (Ctrl+L) takes the path's place, and the search field follows it.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{fit, pad};
use crate::app::{App, Field, HitMap, Spot};
use crate::input::Input;
use crate::model::SortMode;
use crate::theme;

/// The widest the search field gets, and the room the path leaves it.
const SEARCH_WIDTH: u16 = 30;

/// The sort order as the header says it: the column, then `▴` ascending or
/// `▾` reversed.
pub fn sort_label(sort: SortMode) -> &'static str {
    match sort {
        SortMode::Name => "Name ▴",
        SortMode::Kind => "Kind ▴",
        SortMode::Newest => "Modified ▾",
        SortMode::Oldest => "Modified ▴",
        SortMode::Size => "Size ▾",
    }
}

/// Draws the header row; each folder of the path and each field is a spot
/// for the mouse.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    // One blank cell on each side.
    let inner = (area.width as usize).saturating_sub(2);
    let sort = sort_label(app.sort);
    let sort_width = sort.width().min(inner);
    let room = inner - sort_width;
    let gap = room.min(2);
    let path_width = room - gap;
    let left = Rect::new(area.x + 1, area.y, path_width as u16, 1);

    if let Some(Field::Path(input)) = &app.field {
        field(frame, left, "", input, true);
        hits.add(left, Spot::PathField);
    } else if let Some(search) = app.pane().search() {
        // A search keeps room for its field: the path gives way first.
        let kept = (SEARCH_WIDTH + 2) as usize;
        let crumbs_width = path_width.saturating_sub(kept);
        let spans = crumbs(app, app.active, crumbs_width, true, left, hits);
        let used = spans.iter().map(|span| span.content.width()).sum::<usize>() as u16;
        frame.render_widget(Paragraph::new(Line::from(spans)), left);
        let x = left.x + used + 2;
        let room = Rect {
            x,
            width: SEARCH_WIDTH.min(left.right().saturating_sub(x)),
            ..left
        };
        let typing = matches!(app.field, Some(Field::Search));
        field(frame, room, "/", &search.input, typing);
        hits.add(room, Spot::SearchField);
    } else {
        let spans = crumbs(app, app.active, path_width, true, left, hits);
        frame.render_widget(Paragraph::new(Line::from(spans)), left);
    }

    let sort_area = Rect::new(
        area.right().saturating_sub(1 + sort_width as u16),
        area.y,
        sort_width as u16,
        1,
    );
    frame.render_widget(
        Paragraph::new(sort_cells(sort, sort_width)).style(theme::muted()),
        sort_area,
    );
}

/// The label fitted to `width` with its arrow kept in the last cell.
fn sort_cells(sort: &str, width: usize) -> String {
    match sort.rsplit_once(' ') {
        Some((label, arrow)) if width >= 2 => format!("{} {arrow}", fit(label, width - 2)),
        Some((label, _)) => fit(label, width),
        None => fit(sort, width),
    }
}

/// One pane's path on a row of its own above its files, for when two panes
/// share the window: the folder in the foreground in the active pane only.
pub fn draw_pane_path(frame: &mut Frame, app: &App, pane: usize, area: Rect, hits: &mut HitMap) {
    let row = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let spans = crumbs(app, pane, row.width as usize, pane == app.active, row, hits);
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
}

/// Where a pane is in `width` cells from the left of `area`: `~` for home,
/// the parent part muted, the folder itself in the foreground where `lit`.
/// Too long, the parent is cut with `…` before its last `/` and the folder's
/// name is kept. Each folder whose name shows whole is a spot for the mouse.
fn crumbs(
    app: &App,
    index: usize,
    width: usize,
    lit: bool,
    area: Rect,
    hits: &mut HitMap,
) -> Vec<Span<'static>> {
    let cwd = &app.panes[index].cwd;
    let path = app.shown_path(cwd);
    let split = path.rfind('/').map_or(0, |slash| slash + 1);
    let (parent, name) = path.split_at(split);
    let name_style = if lit { theme::base() } else { theme::muted() };
    let spot = |hits: &mut HitMap, from: usize, cells: usize, folder: &Path| {
        let rect = Rect::new(area.x + from as u16, area.y, cells as u16, 1).intersection(area);
        let spot = Spot::Crumb {
            pane: index,
            path: folder.to_path_buf(),
        };
        hits.add(rect, spot);
    };
    if path.width() <= width {
        for folder in cwd.ancestors() {
            let shown = app.shown_path(folder);
            if !path.starts_with(&shown) {
                break;
            }
            let start = match shown.rfind('/') {
                Some(_) if shown == "/" => 0,
                Some(slash) => shown[..=slash].width(),
                None => 0,
            };
            spot(hits, start, shown.width() - start, folder);
            if folder == app.home {
                break;
            }
        }
        return vec![
            Span::styled(parent.to_string(), theme::muted()),
            Span::styled(name.to_string(), name_style),
        ];
    }
    let name_width = name.width().min(width);
    let parent_width = width - name_width;
    let parent = if parent_width >= 2 {
        format!("{}/", pad(parent, parent_width - 1))
    } else {
        " ".repeat(parent_width)
    };
    spot(hits, parent_width, name_width, cwd);
    vec![
        Span::styled(parent, theme::muted()),
        Span::styled(fit(name, name_width), name_style),
    ]
}

/// A field in the header: what is being typed after its `lead`, with the
/// terminal's cursor in it while it has the keyboard.
fn field(frame: &mut Frame, area: Rect, lead: &str, input: &Input, typing: bool) {
    let before = lead.width() as u16;
    if area.width <= before {
        return;
    }
    let (text, column) = input.window((area.width - before) as usize);
    let shown = pad(&format!("{lead}{text}"), area.width as usize);
    let style: Style = if typing {
        theme::field()
    } else {
        theme::on(theme::MUTED, theme::FIELD)
    };
    frame.render_widget(Paragraph::new(shown).style(style), area);
    if typing {
        frame.set_cursor_position(Position::new(area.x + before + column, area.y));
    }
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{ctrl, finish_search, fixture, press, type_text};
    use crate::model::SortMode;
    use crate::theme;
    use crate::ui::testing::{buffer, cursor, find, screen};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn the_path_shows_home_as_a_tilde_with_the_parent_muted_and_the_folder_in_the_foreground() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Enter]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "~/Pictures/Trip", 0..120), Some((1, 0)));
        assert_eq!(drawn[(0, 0)].symbol(), " ", "one blank cell");
        for x in 1..12 {
            assert_eq!(drawn[(x, 0)].fg, theme::MUTED, "{x}");
        }
        for x in 12..16 {
            assert_eq!(drawn[(x, 0)].fg, theme::FOREGROUND, "{x}");
        }
        // Home itself is one tilde, in the foreground.
        press(&mut f.app, &[KeyCode::Backspace, KeyCode::Backspace]);
        let home = buffer(&mut f.app, 120, 30);
        assert_eq!(home[(1, 0)].symbol(), "~");
        assert_eq!(home[(1, 0)].fg, theme::FOREGROUND);
        assert_eq!(home[(2, 0)].symbol(), " ");
    }

    #[test]
    fn a_path_too_long_is_cut_in_the_parent_and_keeps_the_folders_name() {
        let mut f = fixture();
        let deep = f.home.join("Documents/Projects 2026/Quarterly reports");
        std::fs::create_dir_all(&deep).unwrap();
        f.app.navigate(&deep);
        // 30 cells: 28 inside, "Name ▴" and its gap take 8, 20 for the path.
        let lines = screen(&mut f.app, 30, 10);
        assert_eq!(lines[0], " ~…/Quarterly reports  Name ▴ ");
        let drawn = buffer(&mut f.app, 30, 10);
        assert_eq!(drawn[(2, 0)].fg, theme::MUTED);
        assert_eq!(drawn[(4, 0)].fg, theme::FOREGROUND);
        // Narrower still, the name itself is cut.
        let lines = screen(&mut f.app, 20, 10);
        assert_eq!(lines[0], " Quarterly…  Name ▴ ");
    }

    #[test]
    fn the_sort_label_sits_on_the_right_in_the_metadata_ink() {
        let mut f = fixture();
        for (sort, label) in [
            (SortMode::Name, "Name ▴"),
            (SortMode::Kind, "Kind ▴"),
            (SortMode::Newest, "Modified ▾"),
            (SortMode::Oldest, "Modified ▴"),
            (SortMode::Size, "Size ▾"),
        ] {
            f.app.sort = sort;
            let drawn = buffer(&mut f.app, 80, 24);
            let x = 80 - 1 - label.chars().count() as u16;
            assert_eq!(find(&drawn, label, 0..80), Some((x, 0)), "{label}");
            assert_eq!(drawn[(x, 0)].fg, theme::MUTED);
            assert_eq!(drawn[(79, 0)].symbol(), " ");
        }
    }

    #[test]
    fn ctrl_l_turns_the_path_into_a_field_holding_it() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures ", 0..120), Some((1, 0)));
        // On the field's fill, as far as the gap before the sort label.
        for x in [1, 10, 11, 60, 110] {
            assert_eq!(drawn[(x, 0)].fg, theme::FOREGROUND, "{x}");
            assert_eq!(drawn[(x, 0)].bg, theme::FIELD, "{x}");
        }
        assert_eq!(drawn[(111, 0)].bg, ratatui::style::Color::Reset);
        assert_eq!(find(&drawn, "Name ▴", 0..120), Some((113, 0)));
        assert_eq!(cursor(&mut f.app, 120, 24), (11, 0), "after the path");

        // What is typed shows where the cursor is, and the cursor moves along.
        type_text(&mut f.app, "/Tr");
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures/Tr ", 0..120), Some((1, 0)));
        assert_eq!(cursor(&mut f.app, 120, 24), (14, 0));

        // Closed, the path is back.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures", 0..120), Some((1, 0)));
        assert_eq!(drawn[(1, 0)].bg, ratatui::style::Color::Reset);
    }

    #[test]
    fn a_path_longer_than_the_field_scrolls_to_keep_the_cursor_in_sight() {
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        type_text(&mut f.app, &"/abcdefghi".repeat(6));
        // 30 cells: 28 inside, the sort label and its gap take 8, a field of
        // 20, the last of them for the cursor.
        let lines = screen(&mut f.app, 30, 10);
        assert!(
            lines[0].starts_with(" abcdefghi/abcdefghi   Name"),
            "{}",
            lines[0]
        );
        assert_eq!(cursor(&mut f.app, 30, 10), (20, 0));
    }

    #[test]
    fn slash_puts_a_field_after_the_path_of_the_folder_being_searched() {
        let mut f = fixture();
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Enter, KeyCode::Char('/')],
        );
        // Open and empty: the folder's path, then the field.
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures  / ", 0..120), Some((1, 0)));
        assert_eq!(cursor(&mut f.app, 120, 24), (14, 0));

        type_text(&mut f.app, "jp");
        finish_search(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures  /jp ", 0..120), Some((1, 0)));
        assert_eq!(drawn[(3, 0)].fg, theme::FOREGROUND, "Pictures");
        assert_eq!(drawn[(11, 0)].bg, ratatui::style::Color::Reset, "the gap");
        for x in [13, 14, 30] {
            assert_eq!(drawn[(x, 0)].fg, theme::FOREGROUND, "{x}");
            assert_eq!(drawn[(x, 0)].bg, theme::FIELD, "{x}");
        }
        assert_eq!(cursor(&mut f.app, 120, 24), (16, 0), "after what was typed");

        // With the keyboard on what was found the field stays, muted.
        press(&mut f.app, &[KeyCode::Tab]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures  /jp ", 0..120), Some((1, 0)));
        assert_eq!(drawn[(13, 0)].fg, theme::MUTED);
        assert_eq!(drawn[(13, 0)].bg, theme::FIELD);

        // Left, it is gone.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "/jp", 0..120), None);
        assert_eq!(drawn[(13, 0)].bg, ratatui::style::Color::Reset);
    }
}
