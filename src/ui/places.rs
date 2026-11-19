//! The places panel left of the files, a rail: PLACES and DEVICES
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
    let current = app.current_place();
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
                if shows_eject(place, width) {
                    let x = area.x + (width - EJECT_ROOM) as u16;
                    hits.add(Rect::new(x, y, EJECT_ROOM as u16, 1), Spot::Eject(index));
                }
                place_row(place, width, cursor, current == Some(index))
            }
        });
    }
    frame.render_widget(Paragraph::new(drawn), area);
}

/// Cells left free after the widest name, so the panel doesn't feel crowded.
const AIR: usize = 5;

/// The cells a volume's eject button takes at the end of its row, out of
/// its air: the button and a cell after it.
const EJECT_ROOM: usize = 2;

/// The panel's width for what it lists: its widest row (the edge, the
/// glyph and its space, the name, then `AIR`) or heading.
pub fn needed(sections: &[places::Section]) -> u16 {
    let rows = sections
        .iter()
        .flat_map(|section| &section.places)
        .map(|place| {
            let glyph = icons::place(place.kind).width() + 1;
            1 + glyph + place.name.width() + AIR
        });
    let headings = sections.iter().map(|section| section.heading.width() + 2);
    rows.chain(headings).max().unwrap_or(0) as u16
}

/// Whether a place's row, `width` cells wide, ends on an eject button: a
/// volume's does when there is room for it after the edge and the glyph.
fn shows_eject(place: &places::Place, width: usize) -> bool {
    let glyph = icons::place(place.kind).width() + 1;
    place.ejectable && width > 1 + glyph + EJECT_ROOM
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
    let eject = shows_eject(place, width);
    let room = if eject { room - EJECT_ROOM } else { room };
    let name = fit(&place.name, room);
    let gap = room - name.width();
    let mut spans = vec![
        edge,
        Span::styled(glyph, text(glyph_ink)),
        Span::styled(name, text(theme::FOREGROUND)),
        Span::styled(" ".repeat(gap), text(theme::FOREGROUND)),
    ];
    if eject {
        spans.push(Span::styled(icons::eject(), text(theme::MUTED)));
        spans.push(Span::styled(" ", text(theme::FOREGROUND)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use crate::app::Focus;
    use crate::app::testing::{Fixture, alt, ctrl, fixture, key, launch, launch_one_pane};
    use crate::icons;
    use crate::theme;
    use crate::ui::testing::{buffer, find, panes, screen};
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
        let width = panes(&f.app, 120).panel.end as usize;
        let lines = screen(&mut f.app, 120, 30);
        let row = |line: &String| line.chars().take(width).collect::<String>();
        // The body ends over the bottom rule, in every view: in two panes
        // the function key bar is under the panes, not the panel.
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
        let panes = panes(&f.app, 120);
        let panel = || panes.panel.clone();
        assert_eq!(glyph_at(&drawn, "Home", panel()), theme::ICON_FOLDER);
        assert_eq!(glyph_at(&drawn, "Pictures", panel()), theme::ICON_IMAGE);
        assert_eq!(glyph_at(&drawn, "Trash", panel()), theme::MUTED);
        // The list: a folder blue, a text file muted, away from the cursor.
        let files = || panes.files.clone();
        assert_eq!(glyph_at(&drawn, "Pictures", files()), theme::ICON_FOLDER);
        assert_eq!(glyph_at(&drawn, "notes.txt", files()), theme::MUTED);
    }

    /// The fixture with Movies, a file called Music, and two volumes: a
    /// drive and the system's own, a link to `/`.
    fn mounted() -> Fixture {
        let mut f = fixture();
        fs::create_dir(f.home.join("Movies")).unwrap();
        fs::write(f.home.join("Music"), "").unwrap();
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        std::os::unix::fs::symlink("/", f.volumes.join("Macintosh HD")).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        f
    }

    #[test]
    fn the_panel_needs_room_for_its_widest_row_and_heading() {
        use crate::places::{Kind, Place, Section};
        let place = |name: &str| Place {
            name: name.to_string(),
            path: std::path::PathBuf::from("/"),
            kind: Kind::Drive,
            ejectable: false,
        };
        let sections = |names: &[&str], heading| {
            vec![Section {
                heading,
                places: names.iter().map(|name| place(name)).collect(),
            }]
        };
        // The edge, the glyph and its space, the name, then five cells of
        // air before the line.
        assert_eq!(
            super::needed(&sections(&["Home", "Applications"], "PLACES")),
            20
        );
        // A heading wider than every row (here, no rows): its cell each side.
        assert_eq!(super::needed(&sections(&[], "DEVICES")), 9);
        // A short name still gets its air: wider than the heading.
        assert_eq!(super::needed(&sections(&["T7"], "DEVICES")), 10);
        assert_eq!(super::needed(&[]), 0);
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
        // A volume's row ends on its eject button, after the air.
        let ejectable = |row: &str| {
            let name = glyph("Backup", icons::PLACE_DRIVE);
            let button = row.strip_prefix(&name)?.strip_suffix(icons::eject())?;
            (!button.is_empty() && button.trim().is_empty()).then_some(name)
        };
        let views = [
            KeyCode::Char('1'),
            KeyCode::Char('2'),
            KeyCode::Char('3'),
            KeyCode::Char('4'),
        ];
        for view in views {
            f.app.handle_key(ctrl(view));
            let mut rows = panel(&mut f);
            rows[9] = ejectable(&rows[9]).unwrap_or_else(|| rows[9].clone());
            assert_eq!(rows[..10], wanted, "{view:?}");
            assert!(rows[10..].iter().all(String::is_empty), "{view:?}");
            let drawn = buffer(&mut f.app, 120, 30);
            let line = panes(&f.app, 120).panel.end;
            // Headings start where the glyphs do, in the muted ink.
            assert_eq!(find(&drawn, "PLACES", 0..line), Some((1, 2)));
            assert_eq!(find(&drawn, "DEVICES", 0..line), Some((1, 9)));
            for x in 1..7 {
                assert_eq!(drawn[(x, 2)].fg, theme::MUTED, "{x}");
            }
            // The names in the foreground, the glyphs in their kind's colour.
            assert_eq!(drawn[(3, 4)].fg, theme::FOREGROUND);
            assert_eq!(drawn[(1, 4)].fg, theme::ICON_FOLDER);
            // The line right of it, all the way down and into the rules, in
            // two panes too: the function key bar is under the panes.
            let rule = 28;
            for y in 2..rule {
                assert_eq!(drawn[(line, y)].symbol(), "│", "{view:?} {y}");
            }
            assert_eq!(drawn[(line, 1)].symbol(), "┬");
            assert_eq!(drawn[(line, rule)].symbol(), "┴");
            f.app.handle_key(ctrl(KeyCode::Char('1')));
        }
    }

    #[test]
    fn the_place_holding_the_folder_is_on_the_selected_fill() {
        let mut f = fixture();
        f.app.navigate(&f.home.join("Pictures/Trip"));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(cells(&drawn, 5, 3..11), "Pictures");
        for x in panes(&f.app, 120).panel {
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
        let panes = panes(&f.app, 120);
        // Documents, under the panel's cursor.
        assert_eq!(cells(&drawn, 4, 3..12), "Documents");
        assert_eq!(drawn[(0, 4)].symbol(), "▎");
        assert_eq!(drawn[(0, 4)].fg, theme::ACCENT);
        for x in panes.panel {
            assert_eq!(drawn[(x, 4)].bg, theme::CURSOR, "{x}");
        }
        // Home, holding the folder, keeps the selected fill without the edge.
        assert_eq!(drawn[(0, 3)].symbol(), " ");
        assert_eq!(drawn[(10, 3)].bg, theme::SELECTED);
        // The list has no accent edge while the panel has the keyboard.
        let edge = (panes.files.start, 3);
        assert_eq!(drawn[edge].symbol(), " ");
        f.app.handle_key(key(KeyCode::Esc));
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(drawn[(0, 4)].symbol(), " ");
        assert_eq!(drawn[(10, 4)].bg, Color::Reset);
        assert_eq!(drawn[edge].symbol(), "▎");
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
        assert_eq!(cells(&drawn, 3, 0..12), "▎\u{f0219} notes.txt");
        assert_eq!(find(&drawn, "PLACES", 0..120), None);
        // The list runs to the preview's line; the panel's line is gone.
        assert_eq!(drawn[(25, 1)].symbol(), "─");
        assert_eq!(drawn[(25, 10)].symbol(), " ");
        assert_eq!(drawn[(73, 1)].symbol(), "┬");
        // Its cursor row, under the header, fills it up to that line.
        assert_eq!(find(&drawn, "Name ▴", 0..73), Some((3, 2)));
        assert_eq!(drawn[(72, 3)].bg, theme::CURSOR);
        // Without the preview either, the list has the whole width.
        app.handle_key(alt(KeyCode::Char('p')));
        let drawn = buffer(&mut app, 120, 30);
        assert_eq!(drawn[(119, 3)].bg, theme::CURSOR);
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

#[cfg(test)]
mod eject_tests {
    use crate::app::testing::{Fixture, ctrl, fixture, key};
    use crate::app::{App, Focus, Spot};
    use crate::icons;
    use crate::places::{self, Kind};
    use crate::theme;
    use crate::ui::testing::buffer;
    use ratatui::crossterm::event::{
        KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use std::fs;

    /// The fixture with two volumes, Backup and the system's own, and the
    /// panel read again.
    fn mounted() -> Fixture {
        let mut f = fixture();
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        std::os::unix::fs::symlink("/", f.volumes.join("Macintosh HD")).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        f
    }

    fn index_of(app: &App, name: &str) -> Option<usize> {
        places::flat(&app.places).position(|place| place.name == name)
    }

    fn said(app: &App) -> Option<(&str, bool)> {
        (app.message.as_ref()).map(|message| (message.text.as_str(), message.is_error))
    }

    fn click(app: &mut App, x: u16, y: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn a_volume_s_row_ends_on_an_eject_button_and_a_click_on_it_ejects_it() {
        theme::set_depth(theme::Depth::Truecolor);
        let mut f = mounted();
        let backup = index_of(&f.app, "Backup").unwrap();
        let system = index_of(&f.app, "Macintosh HD").unwrap();
        let drawn = buffer(&mut f.app, 120, 30);
        let button = f
            .app
            .hits
            .rect_of(&Spot::Eject(backup))
            .expect("its button");
        // The button and a cell after it, at the end of the row, muted.
        let row = f.app.hits.rect_of(&Spot::Place(backup)).unwrap();
        assert_eq!((button.y, button.width), (row.y, 2));
        assert_eq!(button.x + 2, row.x + row.width);
        assert_eq!(drawn[(button.x, button.y)].symbol(), icons::eject());
        assert_eq!(drawn[(button.x, button.y)].fg, theme::MUTED);
        // The system's own volume and the other places have none.
        assert_eq!(f.app.hits.rect_of(&Spot::Eject(system)), None);
        assert_eq!(f.app.hits.rect_of(&Spot::Eject(0)), None);
        // A click on it ejects the volume, which the panel lists no more.
        let folder = f.app.pane().cwd.clone();
        click(&mut f.app, button.x, button.y);
        assert_eq!(*f.system.ejected.borrow(), [f.volumes.join("Backup")]);
        assert_eq!(said(&f.app), Some(("Ejected Backup", false)));
        assert_eq!(index_of(&f.app, "Backup"), None);
        assert_eq!(f.app.pane().cwd, folder, "a pane elsewhere stays");
        // A click on the row elsewhere opens it, as before.
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        buffer(&mut f.app, 120, 30);
        let row = f.app.hits.rect_of(&Spot::Place(backup)).unwrap();
        click(&mut f.app, row.x + 3, row.y);
        assert_eq!(f.app.pane().cwd, f.volumes.join("Backup"));
        assert_eq!(f.system.ejected.borrow().len(), 1);
    }

    #[test]
    fn a_volume_on_the_mac_s_own_disk_has_no_eject_button_and_is_asked_about_once() {
        let mut f = fixture();
        fs::create_dir(f.volumes.join("Backup")).unwrap();
        fs::create_dir(f.volumes.join("Recovery")).unwrap();
        f.system
            .internal
            .borrow_mut()
            .push(f.volumes.join("Recovery"));
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        // Each volume asked about once, however often the panel is read.
        assert_eq!(f.system.internal_asked.get(), 2);
        buffer(&mut f.app, 120, 30);
        let recovery = index_of(&f.app, "Recovery").expect("still listed");
        let backup = index_of(&f.app, "Backup").unwrap();
        assert_eq!(f.app.hits.rect_of(&Spot::Eject(recovery)), None);
        assert!(f.app.hits.rect_of(&Spot::Eject(backup)).is_some());
        // `e` on it does nothing.
        f.app.handle_key(key(KeyCode::BackTab));
        f.app.places_cursor = recovery;
        f.app.handle_key(key(KeyCode::Char('e')));
        assert!(f.system.ejected.borrow().is_empty());
        assert!(said(&f.app).is_none_or(|(text, _)| !text.contains("eject")));
        assert_eq!(index_of(&f.app, "Recovery"), Some(recovery));
        // Entering the panel again reads it without asking again; Ctrl+R asks.
        f.app.handle_key(key(KeyCode::Esc));
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.system.internal_asked.get(), 2);
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        assert_eq!(f.system.internal_asked.get(), 4);
    }

    #[test]
    fn e_in_the_panel_ejects_the_volume_under_its_cursor_and_panes_in_it_go_home() {
        let mut f = mounted();
        let card = f.volumes.join("Backup").join("Photos");
        fs::create_dir(&card).unwrap();
        f.app.navigate(&card);
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.app.focus, Focus::Places);
        // On a place that is no volume, `e` does nothing.
        f.app.places_cursor = 0;
        f.app.handle_key(key(KeyCode::Char('e')));
        assert!(f.system.ejected.borrow().is_empty());
        let system = index_of(&f.app, "Macintosh HD").unwrap();
        f.app.places_cursor = system;
        f.app.handle_key(key(KeyCode::Char('e')));
        assert!(f.system.ejected.borrow().is_empty());
        assert_eq!(f.app.pane().cwd, card);
        // Something using it: it says so, and the volume stays.
        let backup = index_of(&f.app, "Backup").unwrap();
        f.app.places_cursor = backup;
        f.system.eject_fails.set(true);
        f.app.handle_key(key(KeyCode::Char('e')));
        assert_eq!(
            said(&f.app),
            Some(("Couldn't eject Backup. Something may be using it.", true))
        );
        assert_eq!(index_of(&f.app, "Backup"), Some(backup));
        assert_eq!(f.app.pane().cwd, card);
        // Ejected, the pane that showed it goes home.
        f.system.eject_fails.set(false);
        f.app.handle_key(key(KeyCode::Char('e')));
        assert_eq!(*f.system.ejected.borrow(), [f.volumes.join("Backup")]);
        assert_eq!(f.app.pane().cwd, f.home);
        assert!(!(places::flat(&f.app.places)).any(|place| place.ejectable));
        assert!((places::flat(&f.app.places)).any(|place| place.kind == Kind::Drive));
        assert_eq!(f.app.focus, Focus::Places);
    }
}

#[cfg(test)]
mod pinned_tests {
    use crate::app::Focus;
    use crate::app::testing::{
        ctrl, finish_search, fixture, key, launch, launch_one_pane, press, type_text,
    };
    use crate::icons;
    use crate::places::{self, Kind};
    use crate::theme;
    use crate::ui::testing::{buffer, panes, screen};
    use ratatui::crossterm::event::KeyCode;
    use std::fs;

    fn said(app: &crate::app::App) -> Option<&str> {
        app.message.as_ref().map(|message| message.text.as_str())
    }

    /// The panel's headings and names, trimmed, at 120 by 30.
    fn panel(app: &mut crate::app::App) -> Vec<String> {
        let width = panes(app, 120).panel.end as usize;
        let lines = screen(app, 120, 30);
        (lines[2..28].iter())
            .map(|line| {
                line.chars()
                    .take(width)
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .filter(|line| !line.is_empty())
            .collect()
    }

    #[test]
    fn ctrl_p_pins_the_folder_showing_under_pinned_between_places_and_the_rest() {
        let mut f = fixture();
        let proj = f.home.join("Work").join("Proj");
        fs::create_dir_all(&proj).unwrap();
        f.app.navigate(&proj);
        f.app.handle_key(ctrl(KeyCode::Char('p')));
        assert_eq!(said(&f.app), Some("Pinned Proj"));
        let rows = panel(&mut f.app);
        let trash = rows.iter().position(|row| row.ends_with("Trash")).unwrap();
        assert_eq!(rows[trash + 1], "PINNED");
        assert_eq!(rows[trash + 2], format!("{} Proj", icons::PLACE_PIN));
        // In the folders' blue, lifted while the pane is in it.
        let drawn = buffer(&mut f.app, 120, 30);
        // In the panel's rows, below the path in the header.
        let column = panes(&f.app, 120).panel;
        let (x, y) = (2..28)
            .find_map(|y| {
                let row: String = column.clone().map(|x| drawn[(x, y)].symbol()).collect();
                row.find("Proj")
                    .map(|at| (column.start + row[..at].chars().count() as u16, y))
            })
            .expect("the pinned row");
        assert_eq!(drawn[(x - 2, y)].fg, theme::ICON_FOLDER);
        assert_eq!(drawn[(x, y)].bg, theme::SELECTED);
        // A folder the panel lists already is not pinned again.
        f.app.navigate(&f.home.clone());
        f.app.handle_key(ctrl(KeyCode::Char('p')));
        assert_eq!(said(&f.app), Some("Home is already in the panel."));
        assert_eq!(panel_names(&f.app, Kind::Pinned), ["Proj"]);
        // From the panel it opens like a place, and Ctrl+P unpins it.
        f.app.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.app.focus, Focus::Places);
        let at = places::flat(&f.app.places)
            .position(|place| place.kind == Kind::Pinned)
            .unwrap();
        while f.app.places_cursor < at {
            press(&mut f.app, &[KeyCode::Char('j')]);
        }
        press(&mut f.app, &[KeyCode::Enter]);
        assert_eq!(f.app.pane().cwd, proj);
        f.app.handle_key(key(KeyCode::BackTab));
        f.app.places_cursor = at;
        f.app.handle_key(ctrl(KeyCode::Char('p')));
        assert_eq!(said(&f.app), Some("Unpinned Proj"));
        assert!(!panel(&mut f.app).contains(&"PINNED".to_string()));
        // Where no folder shows, nothing is pinned.
        press(&mut f.app, &[KeyCode::Esc, KeyCode::Char('f')]);
        type_text(&mut f.app, "notes");
        finish_search(&mut f.app);
        // Enter keeps what Find found and moves the keyboard into it.
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.field.is_none());
        f.app.handle_key(ctrl(KeyCode::Char('p')));
        assert_eq!(said(&f.app), Some("Only folders can be pinned."));
    }

    fn panel_names(app: &crate::app::App, kind: Kind) -> Vec<String> {
        (places::flat(&app.places))
            .filter(|place| place.kind == kind)
            .map(|place| place.name.clone())
            .collect()
    }

    #[test]
    fn pins_are_remembered_and_one_whose_folder_is_gone_is_kept_out_of_sight() {
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let (a, b) = (f.home.join("Alpha"), f.home.join("Beta"));
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        for folder in [&b, &a] {
            app.navigate(folder);
            app.handle_key(ctrl(KeyCode::Char('p')));
        }
        // In the order pinned.
        let again = launch(&f.home, Some(state.path()));
        assert_eq!(panel_names(&again, Kind::Pinned), ["Beta", "Alpha"]);
        let file = fs::read_to_string(state.path().join("pinned")).unwrap();
        assert_eq!(file, format!("{}\n{}\n", b.display(), a.display()));
        // Gone, it isn't listed; back, it is.
        fs::remove_dir(&b).unwrap();
        let again = launch(&f.home, Some(state.path()));
        assert_eq!(panel_names(&again, Kind::Pinned), ["Alpha"]);
        assert_eq!(
            fs::read_to_string(state.path().join("pinned")).unwrap(),
            file
        );
        fs::create_dir(&b).unwrap();
        let again = launch(&f.home, Some(state.path()));
        assert_eq!(panel_names(&again, Kind::Pinned), ["Beta", "Alpha"]);
    }
}
