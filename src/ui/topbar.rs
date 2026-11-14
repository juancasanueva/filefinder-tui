//! The header: where the pane is on the left, the parent part muted and the
//! folder itself in the foreground, then the tabs, the view buttons and the
//! sort order on the right.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{fit, pad};
use crate::app::{App, HitMap, Spot, ViewMode};
use crate::icons;
use crate::model::SortMode;
use crate::theme;

/// The view buttons, in order: what Ctrl+1 to Ctrl+4 switch to.
const VIEWS: [ViewMode; 4] = [
    ViewMode::List,
    ViewMode::Columns,
    ViewMode::Grid,
    ViewMode::Dual,
];
/// The cells the buttons take: four glyphs, one space apart.
const BUTTONS_WIDTH: usize = 7;
/// The cells of path the buttons always leave; with fewer they are dropped,
/// before the sort label is.
const PATH_BEFORE_BUTTONS: usize = 24;

/// The cells the view buttons and the gap before them take in `inner` cells
/// of header beside a sort label `sort` cells wide: none when there isn't
/// room for them and enough of the path.
fn buttons_room(inner: usize, sort: usize) -> usize {
    let room = BUTTONS_WIDTH + 2;
    if inner >= sort + 2 + room + PATH_BEFORE_BUTTONS {
        room
    } else {
        0
    }
}

/// The sort order as the header says it: the key, then `▴` ascending or
/// `▾` reversed.
pub fn sort_label(sort: SortMode) -> String {
    let arrow = if sort.reversed { "▾" } else { "▴" };
    format!("{} {arrow}", sort.key.label())
}

/// A tab's label in the strip: ` N name `, with `~` for home and `/` for
/// the root.
pub fn tab_label(app: &App, index: usize) -> String {
    let cwd = &app.tabs[index].pane().cwd;
    let name = if *cwd == app.home {
        "~".to_string()
    } else {
        match cwd.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => cwd.display().to_string(),
        }
    };
    format!(" {} {name} ", index + 1)
}

/// The tab strip in `inner` cells of header, flea's way: none with one tab;
/// else the room left of the view buttons and the sort label, the path's
/// share first and then
/// the labels, as many as fit from the one before the tab showing, the last
/// one cut with `…` when it doesn't fit whole. Each label with its tab.
pub fn tab_strip(app: &App, inner: usize) -> (usize, Vec<(usize, String)>) {
    if app.tabs.len() < 2 {
        return (0, Vec::new());
    }
    let sort = sort_label(app.sort).width();
    let available = inner
        .saturating_sub(sort + 2 + buttons_room(inner, sort))
        .max(1);
    let width = |index: usize| tab_label(app, index).width();
    // As many before the tab showing as fit whole with it.
    let mut first = app.current;
    let mut used = width(first).min(available);
    while first > 0 && used + width(first - 1) <= available {
        first -= 1;
        used += width(first);
    }
    let mut labels = Vec::new();
    let mut used = 0;
    for index in first..app.tabs.len() {
        if used == available {
            break;
        }
        let cells = width(index).min(available - used);
        labels.push((index, fit(&tab_label(app, index), cells)));
        used += cells;
    }
    (available - used, labels)
}

/// Draws the header row; each folder of the path, each tab's label and
/// each view button is a spot for the mouse.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    // One blank cell on each side.
    let inner = (area.width as usize).saturating_sub(2);
    let sort = sort_label(app.sort);
    let sort = sort.as_str();
    let sort_width = sort.width().min(inner);
    let buttons = buttons_room(inner, sort_width);
    let room = inner - sort_width - buttons;
    let gap = room.min(2);
    let (strip_path, labels) = tab_strip(app, inner);
    let path_width = if labels.is_empty() {
        room - gap
    } else {
        strip_path
    };
    let left = Rect::new(area.x + 1, area.y, path_width as u16, 1);
    // The labels follow the path's share, each a spot for the mouse.
    let mut x = left.right();
    for (index, label) in labels {
        let cells = label.width() as u16;
        let rect = Rect::new(x, area.y, cells, 1).intersection(area);
        let style = theme::tab_label(index == app.current);
        frame.render_widget(Paragraph::new(label).style(style), rect);
        hits.add(rect, Spot::Tab(index));
        x += cells;
    }

    let spans = crumbs(app, app.active(), path_width, true, left, hits);
    frame.render_widget(Paragraph::new(Line::from(spans)), left);

    let sort_x = area.right().saturating_sub(1 + sort_width as u16);
    if buttons > 0 {
        draw_buttons(frame, app, sort_x - 2 - BUTTONS_WIDTH as u16, area.y, hits);
    }
    let sort_area = Rect::new(sort_x, area.y, sort_width as u16, 1);
    frame.render_widget(
        Paragraph::new(sort_cells(sort, sort_width)).style(theme::muted()),
        sort_area,
    );
}

/// The four view buttons from `x`, one space apart: the view showing on the
/// tab fill in the foreground, the others muted. Each is a spot for the mouse.
fn draw_buttons(frame: &mut Frame, app: &App, x: u16, y: u16, hits: &mut HitMap) {
    for (slot, view) in VIEWS.into_iter().enumerate() {
        let rect = Rect::new(x + 2 * slot as u16, y, 1, 1);
        let style = theme::tab_label(view == app.view());
        frame.render_widget(Paragraph::new(icons::view(view)).style(style), rect);
        hits.add(rect, Spot::View(view));
    }
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
    let spans = crumbs(
        app,
        pane,
        row.width as usize,
        pane == app.active(),
        row,
        hits,
    );
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
    let cwd = &app.panes()[index].cwd;
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

#[cfg(test)]
mod tests {
    use crate::app::testing::{finish_search, fixture, press, type_text};
    use crate::model::{SortKey, SortMode};
    use crate::theme;
    use crate::ui::testing::{buffer, cursor, find, screen};
    use ratatui::crossterm::event::KeyCode;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::style::Color;

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
        let sorted = |key, reversed| SortMode { key, reversed };
        for (sort, label) in [
            (sorted(SortKey::Name, false), "Name ▴"),
            (sorted(SortKey::Kind, false), "Kind ▴"),
            (sorted(SortKey::Modified, true), "Modified ▾"),
            (sorted(SortKey::Modified, false), "Modified ▴"),
            (sorted(SortKey::Size, true), "Size ▾"),
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
    fn find_leaves_the_header_to_the_path_of_the_folder_being_searched() {
        let mut f = fixture();
        press(
            &mut f.app,
            &[KeyCode::Down, KeyCode::Enter, KeyCode::Char('f')],
        );
        type_text(&mut f.app, "jp");
        finish_search(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&drawn, "~/Pictures ", 0..120), Some((1, 0)));
        assert_eq!(find(&drawn, "Find jp", 0..120), Some((1, 23)));
        assert_eq!(cursor(&mut f.app, 120, 24), (8, 23), "on the bottom row");
    }

    #[test]
    fn the_tab_strip_shows_with_two_tabs_between_the_path_and_the_sort_label() {
        let mut f = fixture();
        let one = screen(&mut f.app, 120, 30);
        let buttons = format!(
            "{} {} {} {}",
            crate::icons::VIEW_LIST,
            crate::icons::VIEW_COLUMNS,
            crate::icons::VIEW_GRID,
            crate::icons::VIEW_DUAL
        );
        assert_eq!(
            one[0].trim_end(),
            format!(" ~{}{buttons}  Name ▴", " ".repeat(102))
        );
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('O')]);
        let drawn = buffer(&mut f.app, 120, 30);
        let header = screen(&mut f.app, 120, 30).remove(0);
        // The path's share, the labels packed against the view buttons.
        assert_eq!(
            header,
            format!(
                " ~/Pictures{} 1 ~  2 Pictures   {buttons}  Name ▴ ",
                " ".repeat(74)
            )
        );
        // The tab showing on the tab fill in the foreground, the other muted.
        for x in 85..90 {
            assert_eq!(
                (drawn[(x, 0)].fg, drawn[(x, 0)].bg),
                (theme::MUTED, Color::Reset),
                "{x}"
            );
        }
        for x in 90..102 {
            assert_eq!(
                (drawn[(x, 0)].fg, drawn[(x, 0)].bg),
                (theme::FOREGROUND, theme::TAB),
                "{x}"
            );
        }
        assert_eq!(drawn[(102, 0)].bg, Color::Reset);
        assert_eq!(drawn[(113, 0)].fg, theme::MUTED, "the sort label");

        // At the root the label is a slash; closed down to one, no strip.
        f.app.navigate(std::path::Path::new("/"));
        assert!(screen(&mut f.app, 120, 30)[0].contains(" 1 ~  2 /   "));
        press(&mut f.app, &[KeyCode::Char('w')]);
        assert_eq!(screen(&mut f.app, 120, 30)[0], one[0]);
    }

    #[test]
    fn the_strip_scrolls_to_keep_the_tab_showing_in_sight() {
        let mut f = fixture();
        for _ in 0..8 {
            press(&mut f.app, &[KeyCode::Char('t')]);
        }
        // Thirty cells for the strip: the ninth tab and the five before it,
        // with no room left for the path.
        let header = |app: &mut crate::app::App| screen(app, 40, 10).remove(0);
        assert_eq!(
            header(&mut f.app),
            "  4 ~  5 ~  6 ~  7 ~  8 ~  9 ~   Name ▴ "
        );
        // The first: it and those after it, as far as there is room.
        press(&mut f.app, &[KeyCode::Char('1')]);
        assert_eq!(
            header(&mut f.app),
            "  1 ~  2 ~  3 ~  4 ~  5 ~  6 ~   Name ▴ "
        );
        // Two cells more: the next label cut with an ellipsis.
        let wider = screen(&mut f.app, 42, 10).remove(0);
        assert_eq!(wider, "  1 ~  2 ~  3 ~  4 ~  5 ~  6 ~  …  Name ▴ ");
        // The fifth: the ones before it that fit whole, then it.
        press(&mut f.app, &[KeyCode::Char('5')]);
        assert_eq!(
            header(&mut f.app),
            "  1 ~  2 ~  3 ~  4 ~  5 ~  6 ~   Name ▴ "
        );
        press(&mut f.app, &[KeyCode::Char('8')]);
        assert_eq!(
            header(&mut f.app),
            "  3 ~  4 ~  5 ~  6 ~  7 ~  8 ~   Name ▴ "
        );
    }

    #[test]
    fn the_view_buttons_sit_before_the_sort_label_with_the_current_one_lit() {
        use crate::app::ViewMode;
        use crate::app::testing::ctrl;
        use crate::icons;
        let mut f = fixture();
        let buttons = [
            (104, ViewMode::List, icons::VIEW_LIST),
            (106, ViewMode::Columns, icons::VIEW_COLUMNS),
            (108, ViewMode::Grid, icons::VIEW_GRID),
            (110, ViewMode::Dual, icons::VIEW_DUAL),
        ];
        for (code, lit) in [('1', 104), ('2', 106), ('3', 108), ('4', 110)] {
            f.app.handle_key(ctrl(KeyCode::Char(code)));
            let drawn = buffer(&mut f.app, 120, 30);
            for (x, view, glyph) in buttons {
                assert_eq!(drawn[(x, 0)].symbol(), glyph, "{view:?}");
                let style = (drawn[(x, 0)].fg, drawn[(x, 0)].bg);
                if x == lit {
                    assert_eq!(style, (theme::FOREGROUND, theme::TAB), "{code} {view:?}");
                } else {
                    assert_eq!(style, (theme::MUTED, Color::Reset), "{code} {view:?}");
                }
            }
            // One space apart, two cells before the sort label.
            for x in [105, 107, 109, 111, 112] {
                assert_eq!(drawn[(x, 0)].symbol(), " ", "{x}");
            }
            assert_eq!(find(&drawn, "Name ▴", 100..120), Some((113, 0)));
            if code == '4' {
                f.app.handle_key(ctrl(KeyCode::Char('4')));
            }
        }
        // With tabs, the strip ends two cells before the buttons.
        f.app.handle_key(ctrl(KeyCode::Char('1')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('O')]);
        let header = screen(&mut f.app, 120, 30).remove(0);
        let glyphs: String = [icons::VIEW_LIST, icons::VIEW_COLUMNS, icons::VIEW_GRID]
            .iter()
            .map(|glyph| format!("{glyph} "))
            .collect();
        let wanted = format!(
            " ~/Pictures{} 1 ~  2 Pictures   {glyphs}{}  Name ▴ ",
            " ".repeat(74),
            icons::VIEW_DUAL
        );
        assert_eq!(header, wanted);
    }

    #[test]
    fn a_click_on_a_view_button_does_what_ctrl_1_to_4_do() {
        use crate::app::ViewMode;
        let mut f = fixture();
        let click = |app: &mut crate::app::App, column| {
            buffer(app, 120, 30);
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
        };
        click(&mut f.app, 108);
        assert_eq!(f.app.view(), ViewMode::Grid);
        click(&mut f.app, 110);
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::Dual, 2));
        // Again, like Ctrl+4: back to the view the two panes came from.
        click(&mut f.app, 110);
        assert_eq!((f.app.view(), f.app.panes().len()), (ViewMode::Grid, 1));
        click(&mut f.app, 106);
        assert_eq!(f.app.view(), ViewMode::Columns);
        click(&mut f.app, 104);
        assert_eq!(f.app.view(), ViewMode::List);
        // Between the buttons, nothing.
        click(&mut f.app, 107);
        assert_eq!(f.app.view(), ViewMode::List);
    }

    #[test]
    fn a_narrow_header_drops_the_buttons_before_the_sort_label() {
        use crate::icons;
        let mut f = fixture();
        let has_buttons = |app: &mut crate::app::App, width| {
            let header = screen(app, width, 10).remove(0);
            header.contains(icons::VIEW_LIST)
        };
        // Room for them and 24 cells of path: 6 + 2 + 7 + 2 + 24 inside.
        assert!(has_buttons(&mut f.app, 43));
        assert!(!has_buttons(&mut f.app, 42));
        assert_eq!(
            screen(&mut f.app, 42, 10)[0],
            format!(" ~{}Name ▴ ", " ".repeat(33))
        );
        // A longer sort label needs more.
        press(&mut f.app, &[KeyCode::Char('s'), KeyCode::Char('s')]);
        assert!(!has_buttons(&mut f.app, 46));
        assert!(has_buttons(&mut f.app, 47));
    }

    #[test]
    fn a_click_on_a_tab_label_shows_that_tab() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Char('O')]);
        buffer(&mut f.app, 120, 30);
        let click = |app: &mut crate::app::App, column| {
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
        };
        click(&mut f.app, 86);
        assert_eq!(
            (f.app.current, f.app.pane().cwd.clone()),
            (0, f.home.clone())
        );
        buffer(&mut f.app, 120, 30);
        click(&mut f.app, 101);
        assert_eq!(f.app.current, 1);
        // Between the labels and the view buttons, nothing.
        buffer(&mut f.app, 120, 30);
        click(&mut f.app, 103);
        assert_eq!(f.app.current, 1);
    }
}
