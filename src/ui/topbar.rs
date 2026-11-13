//! The top bar: where you are, the path or the search being typed, and the
//! view toggles.

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::pad;
use crate::app::{App, Field, HitMap, Pane, Spot, Toggle, ViewMode};
use crate::input::Input;
use crate::theme;

/// The widest the search field gets, and the room the breadcrumbs leave it.
const SEARCH_WIDTH: u16 = 30;

/// Draws the bar; each breadcrumb, field and switch is a spot for the mouse.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    let [row, rule] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let toggles = toggles(app);
    let toggles_width = toggles
        .iter()
        .map(|span| span.content.width())
        .sum::<usize>() as u16;
    let [path, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(toggles_width + 1)]).areas(row);

    if let Some(Field::Path(input)) = &app.field {
        // In place of the breadcrumbs, as far as the toggles.
        let area = Rect {
            x: path.x + 2,
            width: path.width.saturating_sub(3),
            ..path
        };
        field(frame, area, "", input, true);
        hits.add(area, Spot::PathField);
    } else {
        draw_crumbs(frame, app, path, hits);
    }
    let switches = [Toggle::View, Toggle::Sort, Toggle::Hidden, Toggle::Details];
    let mut x = right.x;
    for (span, switch) in toggles.iter().zip(switches) {
        let width = span.content.width() as u16;
        hits.add(
            Rect::new(x, right.y, width, 1).intersection(right),
            Spot::Toggle(switch),
        );
        x = x.saturating_add(width);
    }
    frame.render_widget(
        Paragraph::new(Line::from(toggles)).alignment(Alignment::Left),
        right,
    );
    frame.render_widget(
        Paragraph::new("─".repeat(rule.width as usize)).style(theme::rule()),
        rule,
    );
}

/// Where the pane is, and after it the search field when one is open.
fn draw_crumbs(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    let pane = app.pane();
    let search = pane.search();
    // A search keeps room for its field: its folder's crumbs give way first.
    let kept = if search.is_some() {
        SEARCH_WIDTH + 2
    } else {
        0
    };
    let room = area.width.saturating_sub(4 + kept) as usize;
    let spans = path_spans(app, app.active, room, true, area, hits);
    let used = spans.iter().map(|span| span.content.width()).sum::<usize>() as u16;
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    if let Some(search) = search {
        let x = area.x + used + 2;
        let area = Rect {
            x,
            width: SEARCH_WIDTH.min(area.right().saturating_sub(x + 1)),
            ..area
        };
        let typing = matches!(app.field, Some(Field::Search));
        field(frame, area, "/", &search.input, typing);
        hits.add(area, Spot::SearchField);
    }
}

/// One pane's path on a row of its own above its files, for when two panes
/// share the window: bright in the active pane, dim in the other.
pub fn draw_pane_path(frame: &mut Frame, app: &App, pane: usize, area: Rect, hits: &mut HitMap) {
    let room = area.width.saturating_sub(4) as usize;
    let spans = path_spans(app, pane, room, pane == app.active, area, hits);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Where a pane is, as crumbs that fit in `room` after two cells of air,
/// drawn on the first row of `area`. The last one is bright where `lit`;
/// elsewhere all of them are dim. Each one that stands for a folder is a
/// spot for the mouse.
fn path_spans(
    app: &App,
    index: usize,
    room: usize,
    lit: bool,
    area: Rect,
    hits: &mut HitMap,
) -> Vec<Span<'static>> {
    let crumbs = crumbs(app, &app.panes[index], room);
    let mut spans = vec![Span::raw("  ")];
    let mut x = area.x.saturating_add(2);
    let last = crumbs.len().saturating_sub(1);
    for (at, (crumb, path)) in crumbs.into_iter().enumerate() {
        if at > 0 {
            spans.push(Span::styled(" / ", theme::faint()));
            x = x.saturating_add(3);
        }
        let style = if lit && at == last {
            theme::title()
        } else {
            theme::dim()
        };
        let width = crumb.width() as u16;
        if let Some(path) = path {
            let cells = Rect::new(x, area.y, width, 1).intersection(area);
            hits.add(cells, Spot::Crumb { pane: index, path });
        }
        x = x.saturating_add(width);
        spans.push(Span::styled(crumb, style));
    }
    spans
}

/// A pane's breadcrumbs that fit in `room`, each with the folder it stands
/// for: none for the `…` in place of the ones left out, nor for Recents.
fn crumbs(app: &App, pane: &Pane, room: usize) -> Vec<(String, Option<PathBuf>)> {
    if pane.recents() {
        // Not a folder: it has no way down from home to show.
        return vec![("Recents".to_string(), None)];
    }
    let shown = breadcrumbs(&pane.cwd, &app.home, room);
    let cut = shown != breadcrumbs(&pane.cwd, &app.home, usize::MAX);
    let root = if pane.cwd.starts_with(&app.home) {
        app.home.clone()
    } else {
        PathBuf::from("/")
    };
    let last = shown.len() - 1;
    let folder = |at: usize| match at {
        0 => Some(root.clone()),
        1 if cut => None,
        // The last one is the pane's folder; each before it, its parent.
        _ => pane.cwd.ancestors().nth(last - at).map(Path::to_path_buf),
    };
    let crumbs = shown.into_iter().enumerate();
    crumbs.map(|(at, crumb)| (crumb, folder(at))).collect()
}

/// A field in the bar: what is being typed after its `lead`, with the
/// terminal's cursor in it while it has the keyboard.
fn field(frame: &mut Frame, area: Rect, lead: &str, input: &Input, typing: bool) {
    let before = lead.width() as u16;
    if area.width <= before {
        return;
    }
    let (text, column) = input.window((area.width - before) as usize);
    let shown = pad(&format!("{lead}{text}"), area.width as usize);
    frame.render_widget(Paragraph::new(shown).style(theme::bar_field(typing)), area);
    if typing {
        frame.set_cursor_position(Position::new(area.x + before + column, area.y));
    }
}

/// View, sort, hidden files and details; the ones switched on are lit.
fn toggles(app: &App) -> Vec<Span<'static>> {
    let view = match app.view {
        ViewMode::Grid => "▦",
        ViewMode::List => "≡",
        ViewMode::Columns => "▥",
    };
    let lit = |on: bool| if on { theme::selected() } else { theme::base() };
    vec![
        Span::styled(format!(" {view} "), theme::base()),
        Span::styled(" ⇅ ", theme::base()),
        Span::styled(" ◉ ", lit(app.show_hidden)),
        Span::styled(" ◨ ", lit(app.details_visible)),
    ]
}

/// `~ / Pictures / Lisbon 2026`, dropping middle folders when the bar is narrow.
pub fn breadcrumbs(cwd: &Path, home: &Path, width: usize) -> Vec<String> {
    let (root, rest) = match cwd.strip_prefix(home) {
        Ok(rest) => ("~", rest),
        Err(_) => ("/", cwd.strip_prefix("/").unwrap_or(cwd)),
    };
    let mut crumbs = vec![root.to_string()];
    crumbs.extend(
        rest.components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );

    let total = |crumbs: &[String]| {
        crumbs.iter().map(|crumb| crumb.width()).sum::<usize>() + 3 * (crumbs.len() - 1)
    };
    while total(&crumbs) > width && crumbs.len() > 2 {
        if crumbs[1] == "…" {
            if crumbs.len() == 3 {
                break;
            }
            crumbs.remove(2);
        } else {
            crumbs[1] = "…".to_string();
        }
    }
    crumbs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumbs_start_at_home_or_the_root() {
        let home = Path::new("/Users/me");
        assert_eq!(breadcrumbs(home, home, 80), ["~"]);
        assert_eq!(
            breadcrumbs(Path::new("/Users/me/Pictures/Lisbon 2026"), home, 80),
            ["~", "Pictures", "Lisbon 2026"]
        );
        assert_eq!(
            breadcrumbs(Path::new("/Volumes/USB"), home, 80),
            ["/", "Volumes", "USB"]
        );
        assert_eq!(breadcrumbs(Path::new("/"), home, 80), ["/"]);
    }

    #[test]
    fn breadcrumbs_drop_the_middle_when_narrow() {
        let home = Path::new("/Users/me");
        let deep = Path::new("/Users/me/Pictures/Lisbon 2026/Sintra day trip");
        assert_eq!(breadcrumbs(deep, home, 30), ["~", "…", "Sintra day trip"]);
    }

    #[test]
    fn recents_is_a_breadcrumb_of_its_own() {
        use crate::app::testing::{fixture, press};
        use crate::ui::testing::screen;
        use ratatui::crossterm::event::KeyCode;
        let mut f = fixture();
        // The sidebar's third place.
        press(
            &mut f.app,
            &[
                KeyCode::BackTab,
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Enter,
            ],
        );
        let lines = screen(&mut f.app, 120, 24);
        assert!(lines[0].starts_with("  Recents  "), "{}", lines[0]);
        assert!(!lines[0].contains('~'), "{}", lines[0]);
    }

    #[test]
    fn ctrl_l_turns_the_breadcrumbs_into_a_field_holding_the_path() {
        use crate::app::testing::{ctrl, fixture, press, type_text};
        use crate::theme;
        use crate::ui::testing::{buffer, cursor, find};
        use ratatui::crossterm::event::KeyCode;
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter]);
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures ", 0..120), Some((2, 0)));
        assert_eq!(find(&drawn, "~ / Pictures", 0..120), None);
        // Accent on a lighter ground, as far as the toggles: it reads as a field.
        for x in [2, 11, 12, 60, 105] {
            assert_eq!(drawn[(x, 0)].fg, theme::ACCENT, "{x}");
            assert_eq!(drawn[(x, 0)].bg, theme::LIGHTER_BACKGROUND, "{x}");
        }
        assert_eq!(drawn[(1, 0)].bg, theme::BACKGROUND);
        let (toggles, _) = find(&drawn, "≡", 0..120).expect("the toggles stay");
        assert_eq!(drawn[(toggles - 1, 0)].bg, theme::BACKGROUND);
        assert_eq!(cursor(&mut f.app, 120, 24), (12, 0), "after the path");

        // What is typed shows where the cursor is, and the cursor moves along.
        type_text(&mut f.app, "/Tr");
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures/Tr ", 0..120), Some((2, 0)));
        assert_eq!(cursor(&mut f.app, 120, 24), (15, 0));

        // Closed, the breadcrumbs are back.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~ / Pictures", 0..120), Some((2, 0)));
        assert_eq!(drawn[(2, 0)].bg, theme::BACKGROUND);
    }

    #[test]
    fn a_path_longer_than_the_bar_scrolls_to_keep_the_cursor_in_sight() {
        use crate::app::testing::{ctrl, fixture, type_text};
        use crate::ui::testing::{cursor, screen};
        use ratatui::crossterm::event::KeyCode;
        let mut f = fixture();
        f.app.handle_key(ctrl(KeyCode::Char('l')));
        type_text(&mut f.app, &"/abcdefghi".repeat(6));
        // 27 cells of bar, 13 of them the toggles: a field of 11, the last
        // of them for the cursor.
        let lines = screen(&mut f.app, 27, 10);
        assert!(lines[0].starts_with("  /abcdefghi  "), "{}", lines[0]);
        assert_eq!(cursor(&mut f.app, 27, 10), (12, 0));
    }

    #[test]
    fn slash_puts_a_field_after_the_breadcrumbs_of_the_folder_being_searched() {
        use crate::app::testing::{finish_search, fixture, press, type_text};
        use crate::theme;
        use crate::ui::testing::{buffer, cursor, find};
        use ratatui::crossterm::event::KeyCode;
        let mut f = fixture();
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Enter, KeyCode::Char('/')],
        );
        // Open and empty: the folder's breadcrumbs, then the field.
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~ / Pictures  / ", 0..120), Some((2, 0)));
        assert_eq!(cursor(&mut f.app, 120, 24), (17, 0));

        type_text(&mut f.app, "jp");
        finish_search(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~ / Pictures  /jp ", 0..120), Some((2, 0)));
        assert_eq!(drawn[(13, 0)].fg, theme::BRIGHT_FOREGROUND, "Pictures");
        assert_eq!(drawn[(15, 0)].bg, theme::BACKGROUND, "the gap");
        for x in [16, 17, 30] {
            assert_eq!(drawn[(x, 0)].fg, theme::ACCENT, "{x}");
            assert_eq!(drawn[(x, 0)].bg, theme::LIGHTER_BACKGROUND, "{x}");
        }
        assert_eq!(cursor(&mut f.app, 120, 24), (19, 0), "after what was typed");

        // With the keyboard on what was found the field stays, unlit.
        press(&mut f.app, &[KeyCode::Tab]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~ / Pictures  /jp ", 0..120), Some((2, 0)));
        assert_eq!(drawn[(16, 0)].fg, theme::FOREGROUND);
        assert_eq!(drawn[(16, 0)].bg, theme::LIGHTER_BACKGROUND);

        // Left, it is gone.
        press(&mut f.app, &[KeyCode::Esc]);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "/jp", 0..120), None);
        assert_eq!(drawn[(16, 0)].bg, theme::BACKGROUND);
    }
}
