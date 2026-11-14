//! The places panel left of the files, flea's rail: PLACES and DEVICES
//! under muted headings, a row for each place, the one holding the folder
//! on the selected fill and, while the panel has the keyboard, its cursor
//! row on the cursor fill with the accent edge.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::fit;
use crate::app::{App, Focus, HitMap, Spot};
use crate::icons;
use crate::places::{self, Section};
use crate::theme;

/// A line of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Heading(&'static str),
    /// A place, by its place among them all.
    Place(usize),
    /// Between two sections.
    Blank,
}

/// The panel's lines: each section's heading and places, a blank line
/// between two sections.
fn rows(sections: &[Section]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut index = 0;
    for (number, section) in sections.iter().enumerate() {
        if number > 0 {
            rows.push(Row::Blank);
        }
        rows.push(Row::Heading(section.heading));
        for _ in &section.places {
            rows.push(Row::Place(index));
            index += 1;
        }
    }
    rows
}

/// Draws the panel in `area`; each place is a spot for the mouse. Scrolled
/// as the wheel left it, kept in range, and while the panel has the
/// keyboard so its cursor shows, with its section's heading when it is on
/// the first place.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, hits: &mut HitMap) {
    hits.add(area, Spot::Places);
    let lines = rows(&app.places);
    let height = area.height as usize;
    let focused = app.focus == Focus::Places;
    let mut offset = app.places_offset.min(lines.len().saturating_sub(height));
    let cursor = lines
        .iter()
        .position(|row| *row == Row::Place(app.places_cursor));
    if let Some(cursor) = cursor.filter(|_| focused) {
        let heading = cursor
            .checked_sub(1)
            .filter(|above| matches!(lines[*above], Row::Heading(_)));
        let top = heading.unwrap_or(cursor);
        if top < offset {
            offset = top;
        } else if height > 0 && cursor >= offset + height {
            offset = cursor + 1 - height;
        }
    }
    app.places_offset = offset;
    let current = places::current(&app.places, &app.pane().cwd);
    let all: Vec<&places::Place> = places::flat(&app.places).collect();
    let width = area.width as usize;
    let mut drawn = Vec::new();
    for row in lines.iter().skip(offset).take(height) {
        let y = area.y + drawn.len() as u16;
        drawn.push(match *row {
            Row::Blank => Line::default(),
            Row::Heading(heading) => Line::from(Span::styled(
                fit(&format!(" {heading}"), width),
                theme::muted(),
            )),
            Row::Place(index) => {
                hits.add(Rect::new(area.x, y, area.width, 1), Spot::Place(index));
                let cursor = focused && index == app.places_cursor;
                let place = all[index];
                place_row(place, width, cursor, current == Some(index))
            }
        });
    }
    frame.render_widget(Paragraph::new(drawn), area);
}

/// A place's row: the edge, the glyph, a space and the name, filled to the
/// width. `cursor` is the panel's cursor while it has the keyboard; `here`
/// is the place holding the active pane's folder.
fn place_row(place: &places::Place, width: usize, cursor: bool, here: bool) -> Line<'static> {
    let fill = theme::row_fill(cursor || here, cursor, false);
    let text = |color| fill.fg(theme::ink(color));
    if width == 0 {
        return Line::default();
    }
    let edge = if cursor && !theme::depth().reverses() {
        Span::styled("▎", text(theme::ACCENT))
    } else {
        Span::styled(" ", text(theme::FOREGROUND))
    };
    let glyph_ink = icons::place_tint(place.kind);
    let glyph = fit(&format!("{} ", icons::place(place.kind)), width - 1);
    let room = width - 1 - glyph.width();
    let name = fit(&place.name, room);
    let gap = room - name.width();
    Line::from(vec![
        edge,
        Span::styled(glyph, text(glyph_ink)),
        Span::styled(name, text(theme::FOREGROUND)),
        Span::styled(" ".repeat(gap), text(theme::FOREGROUND)),
    ])
}

#[cfg(test)]
mod tests {
    use crate::app::Focus;
    use crate::app::testing::{Fixture, alt, ctrl, fixture, key, launch, launch_one_pane};
    use crate::icons;
    use crate::theme;
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;
    use std::fs;

    /// The cells of one row from one column to another, as text.
    fn cells(drawn: &Buffer, y: u16, columns: std::ops::Range<u16>) -> String {
        columns.map(|x| drawn[(x, y)].symbol()).collect()
    }

    /// The panel's rows at 120 by 30, each trimmed.
    fn panel(f: &mut Fixture) -> Vec<String> {
        let lines = screen(&mut f.app, 120, 30);
        let row = |line: &String| line.chars().take(25).collect::<String>();
        lines[2..28]
            .iter()
            .map(|line| row(line).trim().to_string())
            .collect()
    }

    #[test]
    fn glyphs_take_the_colour_of_their_kind_in_the_panel_and_the_list() {
        theme::set_depth(theme::Depth::Truecolor);
        let mut f = fixture();
        let drawn = buffer(&mut f.app, 120, 30);
        // The panel: Home and Pictures by what they hold, the Trash muted.
        let glyph_at = |drawn: &Buffer, name: &str, columns: std::ops::Range<u16>| {
            let (x, y) = find(drawn, name, columns).unwrap_or_else(|| panic!("{name}"));
            drawn[(x - 2, y)].fg
        };
        assert_eq!(glyph_at(&drawn, "Home", 0..25), theme::ICON_FOLDER);
        assert_eq!(glyph_at(&drawn, "Pictures", 0..25), theme::ICON_IMAGE);
        assert_eq!(glyph_at(&drawn, "Trash", 0..25), theme::MUTED);
        // The list: a folder blue, a text file muted, away from the cursor.
        assert_eq!(glyph_at(&drawn, "Pictures", 26..73), theme::ICON_FOLDER);
        assert_eq!(glyph_at(&drawn, "notes.txt", 26..73), theme::MUTED);
    }

    /// The fixture with Movies, a file called Music, and two volumes: a
    /// drive and the system's own, a link to `/`.
    fn mounted() -> Fixture {
        let mut f = fixture();
        fs::create_dir(f.home.join("Movies")).unwrap();
        fs::write(f.home.join("Music"), "").unwrap();
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        std::os::unix::fs::symlink("/", f.volumes.join("Macintosh HD")).unwrap();
        f.app.handle_key(key(KeyCode::F(5)));
        f
    }

    #[test]
    fn the_panel_lists_places_then_devices_under_muted_headings_in_every_view() {
        let mut f = mounted();
        let glyph = |text: &str, glyph: &str| format!("{glyph} {text}");
        let wanted = [
            "PLACES".to_string(),
            glyph("Home", icons::PLACE_HOME),
            glyph("Documents", icons::PLACE_DOCUMENTS),
            glyph("Pictures", icons::PLACE_PICTURES),
            glyph("Movies", icons::PLACE_MOVIES),
            glyph("Trash", icons::PLACE_TRASH),
            String::new(),
            "DEVICES".to_string(),
            glyph("Macintosh HD", icons::PLACE_DRIVE),
            glyph("Backup", icons::PLACE_DRIVE),
        ];
        let views = [
            KeyCode::Char('1'),
            KeyCode::Char('2'),
            KeyCode::Char('3'),
            KeyCode::Char('4'),
        ];
        for view in views {
            f.app.handle_key(ctrl(view));
            let rows = panel(&mut f);
            assert_eq!(rows[..10], wanted, "{view:?}");
            assert!(rows[10..].iter().all(String::is_empty), "{view:?}");
            let drawn = buffer(&mut f.app, 120, 30);
            // Headings start where the glyphs do, in the muted ink.
            assert_eq!(find(&drawn, "PLACES", 0..25), Some((1, 2)));
            assert_eq!(find(&drawn, "DEVICES", 0..25), Some((1, 9)));
            for x in 1..7 {
                assert_eq!(drawn[(x, 2)].fg, theme::MUTED, "{x}");
            }
            // The names in the foreground, the glyphs in their kind's colour.
            assert_eq!(drawn[(3, 4)].fg, theme::FOREGROUND);
            assert_eq!(drawn[(1, 4)].fg, theme::ICON_FOLDER);
            // The line right of it, all the way down and into the rules.
            for y in 2..28 {
                assert_eq!(drawn[(25, y)].symbol(), "│", "{view:?} {y}");
            }
            assert_eq!(drawn[(25, 1)].symbol(), "┬");
            assert_eq!(drawn[(25, 28)].symbol(), "┴");
            f.app.handle_key(ctrl(KeyCode::Char('1')));
        }
    }

    #[test]
    fn the_place_holding_the_folder_is_on_the_selected_fill() {
        let mut f = fixture();
        f.app.navigate(&f.home.join("Pictures/Trip"));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(cells(&drawn, 5, 3..11), "Pictures");
        for x in 0..25 {
            assert_eq!(drawn[(x, 5)].bg, theme::SELECTED, "{x}");
        }
        assert_eq!(drawn[(0, 5)].symbol(), " ", "no edge without the keyboard");
        assert_eq!(
            drawn[(1, 5)].fg,
            theme::ICON_IMAGE,
            "its glyph keeps its colour"
        );
        // Home holds it too, but less deeply.
        assert_eq!(drawn[(5, 3)].bg, Color::Reset);
        // In the trash, the trash; nowhere near a place, none.
        f.app.navigate(&f.trash);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(cells(&drawn, 6, 3..8), "Trash");
        assert_eq!(drawn[(10, 6)].bg, theme::SELECTED);
        f.app.navigate(std::path::Path::new("/"));
        let drawn = buffer(&mut f.app, 120, 30);
        for y in 2..10 {
            assert_eq!(drawn[(10, y)].bg, Color::Reset, "{y}");
        }
    }

    #[test]
    fn with_the_keyboard_its_cursor_row_has_the_cursor_fill_and_the_accent_edge() {
        let mut f = fixture();
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.app.focus, Focus::Places);
        f.app.handle_key(key(KeyCode::Char('j')));
        let drawn = buffer(&mut f.app, 120, 30);
        // Documents, under the panel's cursor.
        assert_eq!(cells(&drawn, 4, 3..12), "Documents");
        assert_eq!(drawn[(0, 4)].symbol(), "▎");
        assert_eq!(drawn[(0, 4)].fg, theme::ACCENT);
        for x in 0..25 {
            assert_eq!(drawn[(x, 4)].bg, theme::CURSOR, "{x}");
        }
        // Home, holding the folder, keeps the selected fill without the edge.
        assert_eq!(drawn[(0, 3)].symbol(), " ");
        assert_eq!(drawn[(10, 3)].bg, theme::SELECTED);
        // The list has no accent edge while the panel has the keyboard.
        assert_eq!(drawn[(26, 2)].symbol(), " ");
        f.app.handle_key(key(KeyCode::Esc));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(0, 4)].symbol(), " ");
        assert_eq!(drawn[(10, 4)].bg, Color::Reset);
        assert_eq!(drawn[(26, 2)].symbol(), "▎");
    }

    #[test]
    fn a_long_panel_scrolls_to_keep_its_cursor_in_sight() {
        let mut f = fixture();
        for index in 0..20 {
            fs::create_dir(f.volumes.join(format!("Disk {index:02}"))).unwrap();
        }
        f.app.handle_key(key(KeyCode::BackTab));
        f.app.handle_key(key(KeyCode::Char('G')));
        // Fourteen rows: ten of body.
        let drawn = buffer(&mut f.app, 120, 14);
        assert_eq!(find(&drawn, "Disk 19", 0..25), Some((3, 11)));
        assert_eq!(drawn[(0, 11)].symbol(), "▎");
        assert_eq!(find(&drawn, "PLACES", 0..25), None);
        f.app.handle_key(key(KeyCode::Char('g')));
        let drawn = buffer(&mut f.app, 120, 14);
        assert_eq!(find(&drawn, "PLACES", 0..25), Some((1, 2)));
        assert_eq!(drawn[(0, 3)].symbol(), "▎");
    }

    #[test]
    fn ctrl_b_hides_the_panel_the_list_takes_its_room_and_it_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        fs::write(home.join("notes.txt"), "hello").unwrap();
        let state = home.join(".config");
        let mut app = launch_one_pane(&home, Some(&state));
        assert!(app.places_visible);
        // The keyboard in the panel goes to the list when it hides.
        app.handle_key(key(KeyCode::BackTab));
        app.handle_key(ctrl(KeyCode::Char('b')));
        assert!(!app.places_visible);
        assert_eq!(app.focus, Focus::List);
        let drawn = buffer(&mut app, 120, 30);
        assert_eq!(cells(&drawn, 2, 0..12), "▎\u{f0219} notes.txt");
        assert_eq!(find(&drawn, "PLACES", 0..120), None);
        // The list runs to the preview's line; the panel's line is gone.
        assert_eq!(drawn[(25, 1)].symbol(), "─");
        assert_eq!(drawn[(25, 10)].symbol(), " ");
        assert_eq!(drawn[(73, 1)].symbol(), "┬");
        assert_eq!(find(&drawn, "5 B", 0..73), Some((68, 2)));
        // Without the preview either, the list has the whole width.
        app.handle_key(alt(KeyCode::Char('p')));
        let drawn = buffer(&mut app, 120, 30);
        assert_eq!(find(&drawn, "5 B", 0..120), Some((115, 2)));
        app.handle_key(alt(KeyCode::Char('p')));
        let saved = fs::read_to_string(state.join("state")).unwrap();
        assert!(saved.contains("places=off\n"), "{saved}");
        let mut again = launch(&home, Some(&state));
        assert!(!again.places_visible);
        assert_eq!(find(&buffer(&mut again, 120, 30), "PLACES", 0..120), None);
        // And back.
        again.handle_key(ctrl(KeyCode::Char('b')));
        assert_eq!(
            find(&buffer(&mut again, 120, 30), "PLACES", 0..25),
            Some((1, 2))
        );
        assert!(launch(&home, Some(&state)).places_visible);
    }
}
