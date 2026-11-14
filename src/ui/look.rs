//! The in-terminal Quick Look card: Space. It sits nine cells in from each
//! side (down to a card 24 cells wide) between the window's rules: a bar
//! with the item's glyph, its bold name, one fact and a `×`, a rule, a
//! blank row, then the picture, the text or the folder's items.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::preview::{quiet_row, scrolled, text_rows};
use super::{fit, picture};
use crate::app::{App, HitMap, Look, Spot};
use crate::icons;
use crate::model::Category;
use crate::theme;
use crate::thumbs::Facts;

/// How far in from each side the card sits, and how narrow it gets.
const MARGIN: u16 = 9;
const NARROWEST: u16 = 24;
/// The bar, the rule and a blank row sit over the body, inside the frame.
const ABOVE: u16 = 3;

/// The card's frame on a window this big: from the row under the top rule
/// to the one over the bottom rule.
pub fn outer(area: Rect) -> Rect {
    let margin = MARGIN.min(area.width.saturating_sub(NARROWEST) / 2);
    Rect {
        x: area.x + margin,
        y: area.y + 2,
        width: area.width.saturating_sub(2 * margin),
        height: area.height.saturating_sub(4),
    }
}

/// How much bigger or smaller than the card the picture is drawn, in
/// percent, for each step of `+` and `-`.
fn scale(zoom: i8) -> u16 {
    match zoom {
        ..=-3 => 25,
        -2 => 50,
        -1 => 75,
        0 => 100,
        1 => 150,
        2 => 200,
        _ => 300,
    }
}

/// The fact the bar names: a picture's size, or how many lines a text has.
pub fn fact(facts: Option<&Facts>) -> String {
    let Some(facts) = facts else {
        return String::new();
    };
    if let Some((across, down)) = facts.dimensions {
        return format!("{across} × {down}");
    }
    match facts.lines {
        Some(1) => "1 line".to_string(),
        Some(lines) => format!("{lines} lines"),
        None => String::new(),
    }
}

/// Draws the card for the item under the cursor, and keeps where it and
/// its `×` are.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, hits: &mut HitMap) {
    let outer = outer(area);
    if outer.width < 4 || outer.height < 3 {
        return;
    }
    frame.render_widget(Clear, outer);
    let inside = (outer.width - 2) as usize;
    let edge = |left: &str, right: &str| {
        Line::from(Span::styled(
            format!("{left}{}{right}", "─".repeat(inside)),
            theme::frame(),
        ))
    };
    let mut lines = vec![edge("┌", "┐")];
    for _ in 2..outer.height {
        lines.push(Line::from(vec![
            Span::styled("│", theme::frame()),
            Span::styled(" ".repeat(inside), theme::base()),
            Span::styled("│", theme::frame()),
        ]));
    }
    lines.push(edge("└", "┘"));
    frame.render_widget(Paragraph::new(lines), outer);
    hits.add(outer, Spot::Look);

    let padding = if inside >= 10 { 2 } else { 0 };
    let width = (inside as u16).saturating_sub(2 * padding);
    let left = outer.x + 1 + padding;
    let row = |y: u16| Rect::new(left, outer.y + y, width, 1);
    let rows_inside = outer.height - 2;

    let App {
        tabs,
        current,
        facts,
        thumbs,
        highlighter,
        look,
        show_hidden,
        sort,
        ..
    } = app;
    let Some(look) = look.as_mut() else { return };
    let Some(entry) = tabs[*current].pane().selected() else {
        return;
    };
    let facts = facts
        .as_ref()
        .filter(|facts| facts.describes(entry, *show_hidden, *sort));
    if rows_inside >= 1 {
        frame.render_widget(bar(entry, &fact(facts), width as usize), row(1));
        let close = Rect::new(left + width.saturating_sub(2), outer.y + 1, 1, 1);
        hits.add(close, Spot::LookClose);
    }
    if rows_inside >= 2 {
        let rule = "─".repeat(width as usize);
        frame.render_widget(Span::styled(rule, theme::card_rule()), row(2));
    }
    let body = Rect {
        x: left,
        y: outer.y + 1 + ABOVE,
        width,
        height: rows_inside.saturating_sub(ABOVE),
    };
    look.rows = body.height.max(1) as usize;
    if body.height == 0 {
        return;
    }
    let Some(facts) = facts else { return };
    if let Some(Ok(children)) = &facts.children {
        let rows = children
            .iter()
            .map(|child| quiet_row(child, width as usize))
            .collect();
        scrolled(frame, rows, body, &mut look.scroll);
    } else if !facts.opening.is_empty() {
        let coloured = highlighter
            .as_mut()
            .and_then(|highlighter| highlighter.get(&entry.path, entry.modified, &facts.opening));
        let rows = text_rows(&facts.opening, width as usize, coloured);
        scrolled(frame, rows, body, &mut look.scroll);
    } else if matches!(
        entry.category(),
        Category::Image | Category::App | Category::Pdf
    ) && let Some(thumbs) = thumbs.as_mut()
    {
        // The card is in front of everything: its picture may show.
        thumbs.cover(false);
        let drawn = picture(frame, Some(thumbs), entry, zoomed(body, look), body);
        if !drawn && look.zoom > 0 {
            // A terminal that can't show part of a picture shows it whole.
            picture(frame, Some(thumbs), entry, body, body);
        }
    }
}

/// Where the picture goes at the card's zoom: the body's size scaled,
/// around the body's middle, as far as the window's edges allow.
fn zoomed(body: Rect, look: &Look) -> Rect {
    let percent = i32::from(scale(look.zoom));
    let side = |start: u16, length: u16| {
        let wanted = (i32::from(length) * percent / 100).max(1);
        let start = i32::from(start) + (i32::from(length) - wanted) / 2;
        // Cut as much off the far end as the near one loses at the edge.
        let lost = (-start).max(0);
        let start = start.max(0);
        let length = (wanted - 2 * lost).clamp(1, i32::from(u16::MAX) - start);
        (start as u16, length as u16)
    };
    let (x, width) = side(body.x, body.width);
    let (y, height) = side(body.y, body.height);
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// The bar: the glyph and the bold name, the fact in the metadata ink, and
/// the `×` two cells from the end.
fn bar(entry: &crate::model::Entry, fact: &str, width: usize) -> Line<'static> {
    let mark = if icons::has_marks() {
        format!("{} ", icons::of(entry))
    } else {
        String::new()
    };
    let room = width.saturating_sub(3);
    let head = (mark.width() + entry.name.width()).min(room);
    let name = fit(&format!("{mark}{}", entry.name), head);
    let rest = room - name.width().min(room);
    let fact = if fact.is_empty() || rest < 3 {
        String::new()
    } else {
        format!("  {}", fit(fact, rest - 2))
    };
    let gap = width.saturating_sub(2 + name.width() + fact.width());
    let mut spans = Vec::new();
    match name.strip_prefix(&mark) {
        Some(rest) if !mark.is_empty() => {
            let tint = theme::base().fg(theme::ink(icons::tint(entry)));
            spans.push(Span::styled(mark.clone(), tint));
            spans.push(Span::styled(rest.to_string(), theme::title()));
        }
        _ => spans.push(Span::styled(name, theme::title())),
    }
    spans.push(Span::styled(fact, theme::muted()));
    spans.push(Span::raw(" ".repeat(gap)));
    if width >= 2 {
        spans.push(Span::styled("×", theme::base()));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::{fixture, pictures, press, settle};
    use crate::thumbs::testing::painted;
    use crate::ui::testing::{buffer, find};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::{Color, Modifier};
    use std::fs;

    /// The cells of one row from one column to another, as text.
    fn cells(drawn: &Buffer, y: u16, columns: std::ops::Range<u16>) -> String {
        columns.map(|x| drawn[(x, y)].symbol()).collect()
    }

    #[test]
    fn the_card_sits_nine_cells_in_from_row_3_with_its_bar_rule_and_text() {
        let mut f = fixture();
        let lines: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        fs::write(f.home.join("notes.txt"), lines).unwrap();
        press(
            &mut f.app,
            &[KeyCode::F(5), KeyCode::End, KeyCode::Char(' ')],
        );
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        // The frame, from the row under the top rule to the one over the
        // bottom rule, nine cells in from each side.
        assert_eq!(drawn[(9, 2)].symbol(), "┌");
        assert_eq!(drawn[(110, 2)].symbol(), "┐");
        assert_eq!(drawn[(9, 27)].symbol(), "└");
        assert_eq!(drawn[(110, 27)].symbol(), "┘");
        assert_eq!(drawn[(9, 15)].symbol(), "│");
        for corner in [(9, 2), (110, 27)] {
            assert_eq!(
                (drawn[corner].fg, drawn[corner].bg),
                (theme::MUTED, Color::Reset)
            );
        }
        // The bar, two cells in: the glyph, the bold name, the fact in the
        // metadata ink, and the `×` two cells from the end.
        assert_eq!(drawn[(12, 3)].symbol(), icons::TEXT);
        assert_eq!(find(&drawn, "notes.txt", 10..110), Some((14, 3)));
        assert!(drawn[(14, 3)].modifier.contains(Modifier::BOLD));
        assert_eq!(cells(&drawn, 3, 23..31), "  12 lin");
        assert_eq!(drawn[(25, 3)].fg, theme::MUTED);
        assert_eq!(drawn[(106, 3)].symbol(), "×");
        assert_eq!(drawn[(107, 3)].symbol(), " ");
        // The rule, a blank row, then the text under its gutter.
        assert_eq!(cells(&drawn, 4, 12..108), "─".repeat(96));
        assert_eq!(drawn[(12, 4)].fg, theme::CARD_RULE);
        assert_eq!(cells(&drawn, 5, 10..110).trim(), "");
        assert_eq!(find(&drawn, " 1 line 1", 10..110), Some((12, 6)));
        assert_eq!(drawn[(13, 6)].fg, theme::MUTED, "the gutter");
        assert_eq!(drawn[(15, 6)].fg, theme::FOREGROUND);
        assert_eq!(find(&drawn, "12 line 12", 10..110), Some((12, 17)));
        // Behind it, everything is dimmed.
        assert_eq!(drawn[(1, 0)].fg, theme::DIMMED);
        assert_eq!(find(&drawn, "3 items", 0..9), Some((1, 29)));
        assert_eq!(drawn[(1, 29)].fg, theme::DIMMED);
    }

    #[test]
    fn the_card_follows_the_cursor_and_shows_a_folder_s_items_or_a_short_line() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::Char(' ')]);
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "notes.txt  1 line", 10..110), Some((14, 3)));
        assert_eq!(find(&drawn, "1 hello", 10..110), Some((12, 6)));
        // `k` moves the list's cursor to Pictures, and the card with it.
        press(&mut f.app, &[KeyCode::Char('k')]);
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "Pictures", 10..60).is_some_and(|(_, y)| y == 3));
        assert!(find(&drawn, "×", 100..110).is_some());
        let trip = find(&drawn, "Trip", 10..110).expect("its items");
        assert_eq!(trip.1, 6);
        assert!(find(&drawn, "a.jpg", 10..110).is_some_and(|(_, y)| y == 7));
    }

    #[test]
    fn an_app_shows_its_icon_in_the_card() {
        let (mut f, _thumbnailer) = pictures();
        // The fake draws any file; a file named like an app is one.
        fs::write(f.home.join("Pictures").join("Safari.app"), "x").unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        press(&mut f.app, &[KeyCode::Char(' ')]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "Safari.app", 10..110).is_some());
        let painted: Vec<(u16, u16)> = (0..120)
            .flat_map(|x| (0..30).map(move |y| (x, y)))
            .filter(|at| painted(&drawn[*at]))
            .collect();
        assert!(!painted.is_empty(), "the icon");
        assert!(
            painted
                .iter()
                .all(|(x, y)| (12..108).contains(x) && (6..27).contains(y)),
            "inside the card's body"
        );
    }

    #[test]
    fn a_pdf_shows_its_first_page_in_the_card() {
        let (mut f, _thumbnailer) = pictures();
        // The fake draws any file; Quick Look draws a PDF's first page.
        fs::write(f.home.join("Pictures").join("report.pdf"), "x").unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        press(&mut f.app, &[KeyCode::Char(' ')]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "report.pdf", 10..110).is_some());
        let painted: Vec<(u16, u16)> = (0..120)
            .flat_map(|x| (0..30).map(move |y| (x, y)))
            .filter(|at| painted(&drawn[*at]))
            .collect();
        assert!(!painted.is_empty(), "the page");
        assert!(
            painted
                .iter()
                .all(|(x, y)| (12..108).contains(x) && (6..27).contains(y)),
            "inside the card's body"
        );
    }

    #[test]
    fn a_picture_fills_the_card_with_its_size_in_the_bar_and_zooms() {
        let (mut f, _thumbnailer) = pictures();
        let pictures = f.home.join("Pictures");
        image::GrayImage::new(1600, 1200)
            .save(pictures.join("harbour.png"))
            .unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        press(&mut f.app, &[KeyCode::Char(' ')]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(find(&drawn, "harbour.png  1600 × 1200", 10..110).is_some());
        let painted_cells = |drawn: &Buffer| {
            let cells = (0..120).flat_map(|x| (0..30).map(move |y| (x, y)));
            cells.filter(|at| painted(&drawn[*at])).collect::<Vec<_>>()
        };
        let fit = painted_cells(&drawn);
        assert!(!fit.is_empty(), "the picture");
        // Inside the card's body, and as tall as it.
        assert!(
            fit.iter()
                .all(|(x, y)| (12..108).contains(x) && (6..27).contains(y))
        );
        let span = |cells: &[(u16, u16)]| {
            let mut rows: Vec<u16> = cells.iter().map(|(_, y)| *y).collect();
            let mut columns: Vec<u16> = cells.iter().map(|(x, _)| *x).collect();
            rows.sort();
            rows.dedup();
            columns.sort();
            columns.dedup();
            (columns.len(), rows.len())
        };
        // As big as the card lets it be, never past its own size: the
        // fake's 400 by 300 pixels are 40 by 15 half blocks.
        let (across, down) = span(&fit);
        assert_eq!((across, down), (40, 15));

        // `-` draws it smaller, `0` fits it again.
        press(&mut f.app, &[KeyCode::Char('-'), KeyCode::Char('-')]);
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let half = painted_cells(&buffer(&mut f.app, 120, 30));
        assert!(!half.is_empty() && half.len() < fit.len() / 2);
        let (half_across, half_down) = span(&half);
        assert!(
            half_across < across && half_down <= 10,
            "half the card's height"
        );
        press(&mut f.app, &[KeyCode::Char('0')]);
        assert_eq!(painted_cells(&buffer(&mut f.app, 120, 30)), fit);
        // `+` asks for it bigger than the card, as much of it as shows; a
        // picture smaller than the card stays its own size.
        press(&mut f.app, &[KeyCode::Char('+')]);
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let big = painted_cells(&buffer(&mut f.app, 120, 30));
        assert!(
            big.iter()
                .all(|(x, y)| (12..108).contains(x) && (6..27).contains(y))
        );
        assert_eq!(big, fit);
    }

    #[test]
    fn a_narrow_window_keeps_the_card_24_cells_wide_and_a_tiny_one_draws() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::End, KeyCode::Char(' ')]);
        f.app.tick();
        let drawn = buffer(&mut f.app, 30, 12);
        assert_eq!(drawn[(3, 2)].symbol(), "┌");
        assert_eq!(drawn[(26, 2)].symbol(), "┐");
        assert_eq!(outer(Rect::new(0, 0, 20, 12)), Rect::new(0, 2, 20, 8));
        for (width, height) in [(3, 2), (12, 5), (13, 6), (40, 7)] {
            buffer(&mut f.app, width, height);
        }
    }
}
