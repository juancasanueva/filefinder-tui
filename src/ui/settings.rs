//! The settings card: flea's rail of sections on the left, the section's
//! rows on the right, label and value, in a framed card over the window.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{card, fit};
use crate::app::{App, HitMap, Spot, ViewMode};
use crate::icons;
use crate::settings::{self, NAME, Row, SECTIONS, Section, TAGLINE, Value};
use crate::theme;

/// The rail's cells: the edge, a glyph, a space, the longest name, air.
const RAIL: u16 = 10;
/// The rows' cells, right of the rail's line.
const ROWS: u16 = 52;
/// The About section's head: the name, the tagline and a blank row.
const ABOUT_HEAD: usize = 3;

fn glyph(section: Section) -> &'static str {
    match section {
        Section::View => icons::view(ViewMode::List),
        Section::About => icons::mark(icons::ABOUT, "i"),
    }
}

/// How many body rows a section takes: its head, its rows, a blank row
/// above each heading but the first, and its "Saved" row with the one
/// above it.
fn height(app: &App, section: Section) -> usize {
    let rows = settings::rows(app, section);
    let headings = (rows.iter())
        .filter(|row| matches!(row, Row::Heading(_)))
        .count();
    let head = if section == Section::About {
        ABOUT_HEAD
    } else {
        0
    };
    head + rows.len() + headings.saturating_sub(1) + 2
}

fn ink(color: Color) -> Style {
    Style::default().fg(theme::ink(color))
}

/// Draws the settings card when it is open: in the middle, its title the
/// section's name, flea's rail on the left and the section's rows on the
/// right. Each section and row is a spot for the mouse, and so is the card.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    let Some(open) = app.settings else { return };
    // As tall as the tallest section, so it doesn't jump between them.
    let lines = SECTIONS.iter().map(|s| height(app, *s)).max().unwrap_or(0);
    let card = card::layout(area, RAIL + 2 + ROWS, lines, &[], 0);
    card::draw_frame(frame, &card, open.section.name());
    hits.add(card.outer, Spot::SettingsCard);
    let body = card.body;
    if body.width <= RAIL + 3 || body.height == 0 {
        return;
    }
    let reverses = theme::depth().reverses();
    for (index, section) in SECTIONS.into_iter().enumerate() {
        let y = body.y + index as u16;
        if y >= body.bottom() {
            break;
        }
        let current = section == open.section;
        let lit = current && open.in_rail;
        // The rail's current section on the selected fill, on the cursor's
        // while the rail has the keyboard.
        let fill = match (lit, current) {
            (true, _) => theme::row_fill(true, true, false),
            (false, true) => theme::row_fill(true, false, false),
            (false, false) => theme::row_fill(false, false, false),
        };
        let edge = match lit && !reverses {
            true => Span::styled("▎", fill.fg(theme::ink(theme::ACCENT))),
            false => Span::styled(" ", fill),
        };
        let ink = if current {
            theme::FOREGROUND
        } else {
            theme::MUTED
        };
        let text = fit(
            &format!("{} {}", glyph(section), section.name()),
            RAIL as usize - 1,
        );
        let pad = " ".repeat(RAIL as usize - 1 - text.width());
        let rect = Rect::new(body.x, y, RAIL, 1);
        let spans = vec![
            edge,
            Span::styled(format!("{text}{pad}"), fill.fg(theme::ink(ink))),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
        hits.add(rect, Spot::SettingsSection(section));
    }
    // The line between the rail and the rows.
    let line = vec![Line::from(Span::styled("│", theme::frame())); body.height as usize];
    frame.render_widget(
        Paragraph::new(line),
        Rect::new(body.x + RAIL, body.y, 1, body.height),
    );

    let pane = Rect::new(
        body.x + RAIL + 2,
        body.y,
        body.width - RAIL - 2,
        body.height,
    );
    let width = pane.width as usize;
    let put = |frame: &mut Frame, y: u16, line: Line<'static>| {
        if y < pane.bottom() {
            frame.render_widget(Paragraph::new(line), Rect::new(pane.x, y, pane.width, 1));
        }
    };
    // The section's lines, each with the row it is when it is one.
    let mut lines: Vec<(Line<'static>, Option<usize>)> = Vec::new();
    if open.section == Section::About {
        let bold = ink(theme::FOREGROUND).add_modifier(Modifier::BOLD);
        let head = vec![
            Span::raw(" "),
            Span::styled(
                format!("{} ", icons::mark(icons::FOLDER, "›")),
                ink(theme::ACCENT),
            ),
            Span::styled(fit(NAME, width.saturating_sub(4)), bold),
        ];
        let tagline = fit(TAGLINE, width.saturating_sub(1));
        lines.push((Line::from(head), None));
        lines.push((
            Line::from(Span::styled(format!(" {tagline}"), ink(theme::MUTED))),
            None,
        ));
        lines.push((Line::default(), None));
    }
    let mut item = 0;
    let mut cursor_line = 0;
    for row in settings::rows(app, open.section) {
        match row {
            Row::Heading(text) => {
                // A blank row between sections.
                if !lines.is_empty() {
                    lines.push((Line::default(), None));
                }
                let heading = Span::styled(format!(" {text}"), ink(theme::MUTED));
                lines.push((Line::from(heading), None));
            }
            Row::Item {
                label,
                value,
                caption,
                ..
            } => {
                let lit = !open.in_rail && item == open.cursor;
                if item == open.cursor {
                    cursor_line = lines.len();
                }
                lines.push((item_line(label, caption, &value, lit, width), Some(item)));
                item += 1;
            }
        }
    }
    // A card shorter than its section scrolls, the keyboard's row in sight;
    // the last two rows stay for "Saved".
    let shown = (pane.height as usize).saturating_sub(2).max(1);
    let top = (cursor_line + 1).saturating_sub(shown);
    for (at, (line, row)) in lines.into_iter().skip(top).take(shown).enumerate() {
        let y = pane.y + at as u16;
        put(frame, y, line);
        if let Some(row) = row
            && y < pane.bottom()
        {
            hits.add(Rect::new(pane.x, y, pane.width, 1), Spot::SettingsRow(row));
        }
    }
    if open.saved {
        let saved = Line::from(Span::styled(" Saved", ink(theme::MUTED)));
        put(frame, pane.bottom().saturating_sub(1), saved);
    }
}

/// A row `width` cells wide: the edge, the label and its caption on the
/// left, the value on the right a cell in from the end; on the cursor's
/// fill with the accent edge where the keyboard is.
fn item_line(
    label: &str,
    caption: Option<&str>,
    value: &Value,
    lit: bool,
    width: usize,
) -> Line<'static> {
    let fill = theme::row_fill(lit, true, false);
    let text = |color| fill.fg(theme::ink(color));
    let mut right: Vec<Span<'static>> = match value {
        Value::Choice(choice) => vec![
            Span::styled(choice.to_string(), text(theme::FOREGROUND)),
            Span::styled(" ›", text(theme::MUTED)),
        ],
        Value::Check(true) => vec![Span::styled("[x]", text(theme::FOREGROUND))],
        Value::Check(false) => vec![Span::styled("[ ]", text(theme::MUTED))],
        Value::Text(words) => vec![Span::styled(words.clone(), text(theme::MUTED))],
    };
    right.push(Span::styled(" ", text(theme::FOREGROUND)));
    let right_width: usize = right.iter().map(|span| span.width()).sum();
    let edge = match lit && !theme::depth().reverses() {
        true => Span::styled("▎", text(theme::ACCENT)),
        false => Span::styled(" ", text(theme::FOREGROUND)),
    };
    let room = width.saturating_sub(1 + right_width + 1);
    let label = fit(label, room);
    let mut spans = vec![edge, Span::styled(label.clone(), text(theme::FOREGROUND))];
    let mut used = 1 + label.width();
    if let Some(caption) = caption {
        let caption = fit(&format!("  {caption}"), room.saturating_sub(label.width()));
        used += caption.width();
        spans.push(Span::styled(caption, text(theme::DIMMED)));
    }
    let gap = width.saturating_sub(used + right_width);
    spans.push(Span::styled(" ".repeat(gap), text(theme::FOREGROUND)));
    spans.extend(right);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use crate::app::Spot;
    use crate::app::testing::{fixture, press};
    use crate::icons;
    use crate::settings::{Section, TAGLINE};
    use crate::theme;
    use crate::ui::testing::{buffer, find};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{
        KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::style::Modifier;

    fn mouse(kind: MouseEventKind, (column, row): (u16, u16)) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(app: &mut crate::app::App, at: (u16, u16)) {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), at));
    }

    /// Where a piece of text starts inside the card, reading down: the
    /// window behind it is no place to look.
    fn find_in(drawn: &Buffer, app: &crate::app::App, text: &str) -> Option<(u16, u16)> {
        let card = app.hits.rect_of(&Spot::SettingsCard).expect("the card");
        let cells: Vec<&str> = text.split_inclusive(|_| true).collect();
        (card.y..card.bottom()).find_map(|y| {
            (card.x..card.right())
                .find(|x| {
                    let end = x + cells.len() as u16;
                    end <= card.right()
                        && (cells.iter().zip(*x..end))
                            .all(|(cell, x)| drawn[(x, y)].symbol() == *cell)
                })
                .map(|x| (x, y))
        })
    }

    /// The text of one row of the screen.
    fn line(drawn: &Buffer, y: u16) -> String {
        (0..drawn.area.width)
            .map(|x| drawn[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn the_gear_after_the_view_buttons_opens_the_settings_and_says_so() {
        use crate::app::ViewMode;
        let mut f = fixture();
        let drawn = buffer(&mut f.app, 120, 30);
        let gear = find(&drawn, icons::GEAR, 0..120).expect("the gear");
        let dual = find(&drawn, icons::view(ViewMode::Dual), 0..120).expect("the last view button");
        assert_eq!(gear.1, 0);
        // A chip like the view buttons, a cell after the last of them.
        assert_eq!(gear.0, dual.0 + 4);
        assert_eq!(drawn[(gear.0 - 1, 0)].bg, theme::TAB);
        f.app.handle_mouse(mouse(MouseEventKind::Moved, gear));
        let shown = buffer(&mut f.app, 120, 30);
        assert!(find(&shown, "Settings  ,", 0..120).is_some(), "its tooltip");
        click(&mut f.app, gear);
        assert!(f.app.settings.is_some());
        assert_eq!(f.app.tip, None);
    }

    #[test]
    fn the_card_shows_the_rail_and_the_view_rows_with_their_values_on_the_right() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char(',')]);
        let drawn = buffer(&mut f.app, 120, 30);
        // The section's name as the title, bold, and "esc".
        let (tx, ty) = find_in(&drawn, &f.app, "View").expect("the title");
        assert!(drawn[(tx, ty)].modifier.contains(Modifier::BOLD));
        assert!(line(&drawn, ty).contains("esc"));
        // The rail under it: View on the selected fill, About after it.
        let (_, about) = find_in(&drawn, &f.app, "About").expect("the rail");
        // View in the rail, right above About and lined up with it.
        let (ax, _) = find_in(&drawn, &f.app, "About").unwrap();
        let (vx, vy) = (ax, about - 1);
        assert_eq!(
            line(&drawn, vy).get(..).map(|l| l.contains("View")),
            Some(true)
        );
        assert_eq!(drawn[(vx, vy)].symbol(), "V");
        assert_eq!(drawn[(vx, vy)].bg, theme::SELECTED);
        assert!(line(&drawn, vy).contains(icons::view(crate::app::ViewMode::List)));
        assert!(line(&drawn, about).contains(icons::ABOUT));
        // The rows: a heading, labels on the left, values on the right.
        let (hx, hy) = find_in(&drawn, &f.app, "VIEW").expect("the heading");
        assert_eq!(drawn[(hx, hy)].fg, theme::MUTED);
        let row = |label: &str| {
            let (x, y) = find_in(&drawn, &f.app, label).expect(label);
            (x, y, line(&drawn, y))
        };
        let (lx, ly, text) = row("Last-used view");
        assert!(text.contains("List ›"), "{text}");
        let (value_x, _) = find_in(&drawn, &f.app, "List ›").unwrap();
        assert!(value_x > lx + 20);
        assert_eq!(drawn[(value_x, ly)].fg, theme::FOREGROUND);
        // The keyboard's row: the cursor fill and the accent edge.
        assert_eq!(drawn[(lx, ly)].bg, theme::CURSOR);
        assert_eq!(drawn[(lx - 1, ly)].symbol(), "▎");
        assert!(row("Preview pane").2.contains("[x]"));
        assert!(row("Places panel").2.contains("[x]"));
        assert!(row("Detail column").2.contains("columns view"));
        assert!(row("Detail column").2.contains("Size ›"));
        assert!(row("Sort by").2.contains("Name ›"));
        assert!(row("Order").2.contains("Ascending ›"));
        assert!(row("Show hidden files").2.contains("[ ]"));
        assert!(find_in(&drawn, &f.app, "SORTING").is_some());
        assert!(find_in(&drawn, &f.app, "Saved").is_none());
        // After a change, "Saved".
        press(&mut f.app, &[KeyCode::Char('j'), KeyCode::Char(' ')]);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(row_of(&drawn, &f.app, "Preview pane").contains("[ ]"));
        let (sx, sy) = find_in(&drawn, &f.app, "Saved").expect("Saved");
        assert_eq!(drawn[(sx, sy)].fg, theme::MUTED);
        // With the keyboard in the rail, its row has the edge and the fill.
        press(&mut f.app, &[KeyCode::Tab]);
        let drawn = buffer(&mut f.app, 120, 30);
        let (rx, ry) = (vx, vy);
        // The edge, the glyph, a space, the name.
        assert_eq!(drawn[(rx - 3, ry)].symbol(), "▎");
        assert_eq!(drawn[(rx + 1, ry)].bg, theme::CURSOR);
        // Everything behind it is dimmed.
        assert_eq!(drawn[(1, 0)].fg, theme::DIMMED);
    }

    fn row_of(drawn: &Buffer, app: &crate::app::App, label: &str) -> String {
        let (_, y) = find_in(drawn, app, label).expect(label);
        line(drawn, y)
    }

    #[test]
    fn a_blank_row_stands_between_sections() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char(',')]);
        let drawn = buffer(&mut f.app, 120, 30);
        let (_, view) = find_in(&drawn, &f.app, "VIEW").unwrap();
        let (_, detail) = find_in(&drawn, &f.app, "Detail column").unwrap();
        // The section after the view's rows is the columns'.
        let (sx, sorting) = find_in(&drawn, &f.app, "LIST VIEW COLUMNS").unwrap();
        // None above the first heading: it is the first row of the body.
        assert_eq!(
            view,
            f.app
                .hits
                .rect_of(&crate::app::Spot::SettingsSection(Section::View))
                .unwrap()
                .y
        );
        assert_eq!(sorting, detail + 2);
        let blank: String = (sx..sx + 20)
            .map(|x| drawn[(x, detail + 1)].symbol())
            .collect();
        assert_eq!(blank.trim(), "");
        // The rows under it keep their place among the items: the gap is no row.
        let (_, size) = find_in(&drawn, &f.app, "Size  ").unwrap();
        assert_eq!(size, sorting + 1);
        let card = f.app.hits.rect_of(&crate::app::Spot::SettingsCard).unwrap();
        let (x, y) = (sx, detail + 1);
        assert!(card.contains(ratatui::layout::Position::new(x, y)));
        click(&mut f.app, (x, y));
        assert!(f.app.settings.is_some(), "a click on the gap does nothing");
        assert_eq!(f.app.settings.map(|s| s.saved), Some(false));
    }

    #[test]
    fn a_short_window_scrolls_the_rows_to_keep_the_keyboard_s_row_in_sight() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char(',')]);
        let drawn = buffer(&mut f.app, 120, 18);
        assert!(find_in(&drawn, &f.app, "Last-used view").is_some());
        assert!(find_in(&drawn, &f.app, "Space previews in").is_none());
        // Down to the last row: it shows, on the cursor's fill.
        for _ in 0..20 {
            press(&mut f.app, &[KeyCode::Char('j')]);
        }
        let drawn = buffer(&mut f.app, 120, 18);
        let (x, y) = find_in(&drawn, &f.app, "Space previews in").expect("the last row");
        assert_eq!(drawn[(x, y)].bg, theme::CURSOR);
        assert!(find_in(&drawn, &f.app, "Last-used view").is_none());
        // A click on it is a click on that row.
        click(&mut f.app, (x, y));
        assert_eq!(f.app.space_look, crate::app::SpaceLook::QuickLook);
        // Back up, the first row again.
        for _ in 0..20 {
            press(&mut f.app, &[KeyCode::Char('k')]);
        }
        let drawn = buffer(&mut f.app, 120, 18);
        assert!(find_in(&drawn, &f.app, "Last-used view").is_some());
    }

    #[test]
    fn about_shows_the_name_the_tagline_the_version_and_its_rows() {
        let mut f = fixture();
        press(
            &mut f.app,
            &[KeyCode::Char(','), KeyCode::Tab, KeyCode::Char('j')],
        );
        let drawn = buffer(&mut f.app, 120, 30);
        let (nx, ny) = find_in(&drawn, &f.app, "filefinder-tui").expect("the name");
        assert!(drawn[(nx, ny)].modifier.contains(Modifier::BOLD));
        assert!(line(&drawn, ny).contains(icons::FOLDER));
        let (gx, gy) = find_in(&drawn, &f.app, TAGLINE).expect("the tagline");
        assert_eq!((gy, drawn[(gx, gy)].fg), (ny + 1, theme::MUTED));
        assert!(row_of(&drawn, &f.app, "Version").contains(env!("CARGO_PKG_VERSION")));
        assert!(row_of(&drawn, &f.app, "Based on").contains("flea by GM · MIT"));
        assert!(row_of(&drawn, &f.app, "Keyboard sheet").contains('?'));
        assert!(row_of(&drawn, &f.app, "Report an issue").contains("Open"));
        assert!(find_in(&drawn, &f.app, "Last-used view").is_none());
    }

    #[test]
    fn clicks_pick_a_section_change_a_row_and_outside_close_the_card() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char(',')]);
        let drawn = buffer(&mut f.app, 120, 30);
        let about = find_in(&drawn, &f.app, "About").unwrap();
        click(&mut f.app, about);
        assert_eq!(f.app.settings.map(|s| s.section), Some(Section::About));
        assert!(f.app.hits.at(about.0, about.1) == Some(&Spot::SettingsSection(Section::About)));
        let drawn = buffer(&mut f.app, 120, 30);
        let _ = drawn;
        click(&mut f.app, (about.0, about.1 - 1));
        assert_eq!(f.app.settings.map(|s| s.section), Some(Section::View));
        let drawn = buffer(&mut f.app, 120, 30);
        // A click on a row puts the keyboard there and does what Enter does.
        let places = find_in(&drawn, &f.app, "Places panel").unwrap();
        click(&mut f.app, places);
        assert!(!f.app.places_visible);
        let open = f.app.settings.unwrap();
        assert_eq!((open.cursor, open.in_rail, open.saved), (2, false, true));
        // A click inside the card but on nothing keeps it; outside closes it.
        let drawn = buffer(&mut f.app, 120, 30);
        let (hx, hy) = find_in(&drawn, &f.app, "VIEW").unwrap();
        click(&mut f.app, (hx, hy));
        assert!(f.app.settings.is_some());
        click(&mut f.app, (1, 29));
        assert_eq!(f.app.settings, None);
    }
}
