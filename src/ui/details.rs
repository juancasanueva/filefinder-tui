//! The details pane: a large preview of the selected item, what it is, and an Open button.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::{fit, grid, pad, picture};
use crate::app::{App, Focus};
use crate::format;
use crate::model::Entry;
use crate::theme;
use crate::thumbs::{Facts, Thumbs};

/// The fewest rows the pane needs before a third of them go to the preview.
const ROWS_FOR_A_PREVIEW: u16 = 12;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(4),
        y: area.y + 1,
        height: area.height.saturating_sub(2),
    };
    let Some(entry) = app.panes[app.active].selected() else {
        frame.render_widget(
            Paragraph::new("Nothing selected").style(theme::dim()),
            inner,
        );
        return;
    };
    // What was read about the item, unless it was about another one.
    let facts = app.facts.as_ref().filter(|facts| facts.describes(entry));
    let preview_rows = if inner.height >= ROWS_FOR_A_PREVIEW {
        inner.height / 3
    } else {
        0
    };
    // A blank row keeps the preview, when there is one, off the name.
    let [preview, _, info, button] = Layout::vertical([
        Constraint::Length(preview_rows),
        Constraint::Length(preview_rows.min(1)),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(inner);
    let width = info.width as usize;

    let summary = match entry.item_count() {
        Some(count) => format!("Folder · {}", format::items(count)),
        None if entry.is_dir => "Folder".to_string(),
        None => format!("{} · {}", entry.kind(), format::size(entry.size)),
    };
    let mut lines = vec![
        Line::from(Span::styled(entry.name.clone(), theme::title())),
        Line::from(Span::styled(fit(&summary, width), theme::dim())),
        Line::default(),
    ];
    if let Some(modified) = entry.modified {
        lines.push(field(
            "Modified",
            &format::date_time(modified, app.date_style),
            width,
        ));
    }
    if let Some((across, down)) = facts.and_then(|facts| facts.dimensions) {
        lines.push(field("Dimensions", &format!("{across} × {down}"), width));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), info);

    let label = if app.focus == Focus::Details {
        "› Open"
    } else {
        "Open"
    };
    let text = format!(
        " {}⏎ ",
        pad(label, (button.width as usize).saturating_sub(3))
    );
    frame.render_widget(Paragraph::new(text).style(theme::button()), button);

    if preview_rows > 0 {
        draw_preview(frame, app.thumbs.as_mut(), entry, facts, preview);
    }
}

/// The top of the pane: a text file's opening lines, a file's picture, or the
/// stand-in the grid would show.
fn draw_preview(
    frame: &mut Frame,
    thumbs: Option<&mut Thumbs>,
    entry: &Entry,
    facts: Option<&Facts>,
    area: Rect,
) {
    let opening = facts.map(|facts| facts.opening.as_slice()).unwrap_or(&[]);
    if !opening.is_empty() {
        let lines: Vec<Line> = opening
            .iter()
            .take(area.height as usize)
            .map(|line| Line::from(fit(line, area.width as usize)))
            .collect();
        frame.render_widget(Paragraph::new(lines).style(theme::dim()), area);
    } else if entry.is_dir {
        grid::folder_card(frame, area);
    } else if !picture(frame, thumbs, entry, area, area) {
        grid::stand_in(frame, area, area, entry);
    }
}

/// A label on the left and its value against the right edge.
fn field(label: &str, value: &str, width: usize) -> Line<'static> {
    let gap = width.saturating_sub(label.width() + value.width()).max(1);
    Line::from(vec![
        Span::styled(label.to_string(), theme::dim()),
        Span::raw(" ".repeat(gap)),
        Span::styled(value.to_string(), theme::base()),
    ])
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{fixture, pictures, press, settle};
    use crate::icons::Icon;
    use crate::theme;
    use crate::thumbs::testing::painted;
    use crate::ui::testing::{buffer, find};
    use ratatui::crossterm::event::KeyCode;
    use std::fs;

    #[test]
    fn a_picture_shows_large_on_top_with_its_dimensions_under_modified() {
        let (mut f, _thumbnailer) = pictures();
        let pictures = f.home.join("Pictures");
        image::GrayImage::new(1600, 1200)
            .save(pictures.join("harbour.png"))
            .unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        f.app.tick();
        buffer(&mut f.app, 120, 24);
        settle(&mut f.app);
        let shown = buffer(&mut f.app, 120, 24);

        // Six rows on top of the pane, the picture in the middle of its 29 cells.
        for (x, y) in [(95, 3), (110, 3), (95, 8), (110, 8)] {
            assert!(painted(&shown[(x, y)]), "{x}, {y}");
        }
        for (x, y) in [(94, 3), (111, 3), (95, 2), (95, 9)] {
            assert!(!painted(&shown[(x, y)]), "{x}, {y}");
        }
        assert_eq!(find(&shown, "harbour.png", 86..120), Some((89, 10)));
        assert_eq!(find(&shown, "PNG image · ", 86..120), Some((89, 11)));
        assert_eq!(find(&shown, "Modified", 86..120), Some((89, 13)));
        assert_eq!(find(&shown, "Dimensions", 86..120), Some((89, 14)));
        assert_eq!(shown[(89, 14)].fg, theme::DIM);
        // Against the right edge, like the date above it.
        assert_eq!(find(&shown, "1600 × 1200", 86..120), Some((107, 14)));
        assert_eq!(shown[(107, 14)].fg, theme::FOREGROUND);

        // A file that is no picture inside has no dimensions to show.
        press(&mut f.app, &[KeyCode::Up]);
        f.app.tick();
        let plain = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&plain, "a.jpg", 86..120), Some((89, 10)));
        assert_eq!(find(&plain, "Dimensions", 86..120), None);
    }

    #[test]
    fn a_text_file_shows_its_opening_lines_dim_in_place_of_the_picture() {
        let (mut f, thumbnailer) = pictures();
        let notes = f.home.join("notes.txt");
        fs::write(
            &notes,
            "First line\n\tindented\n\nfourth, and long enough to be cut",
        )
        .unwrap();
        press(&mut f.app, &[KeyCode::End]);
        f.app.tick();
        buffer(&mut f.app, 120, 24);
        settle(&mut f.app);
        let shown = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&shown, "First line", 86..120), Some((89, 3)));
        assert_eq!(shown[(89, 3)].fg, theme::DIM);
        assert_eq!(find(&shown, "    indented", 86..120), Some((89, 4)));
        assert_eq!(
            find(&shown, "fourth, and long enough to b…", 86..120),
            Some((89, 6))
        );
        assert_eq!(find(&shown, "notes.txt", 86..120), Some((89, 10)));
        assert!(
            thumbnailer.asked().is_empty(),
            "its lines are its preview: no picture is asked for"
        );

        // Not text inside: no lines, the stand-in instead.
        fs::write(&notes, [0xff, 0xfe, b'h', b'i', b'\n']).unwrap();
        press(&mut f.app, &[KeyCode::F(5)]);
        f.app.tick();
        let binary = buffer(&mut f.app, 120, 24);
        assert_eq!(find(&binary, "hi", 86..120), None);
        assert_eq!(binary[(89, 3)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(find(&binary, Icon::Text.glyph(), 86..120), Some((103, 5)));
    }

    #[test]
    fn without_pictures_the_stand_in_sits_on_top_and_a_short_pane_drops_it() {
        let mut f = fixture();
        let folder = buffer(&mut f.app, 120, 24);
        // Documents: the folder's card with its tab, then the name as before.
        assert_eq!(folder[(89, 3)].symbol(), "▄");
        assert_eq!(folder[(89, 4)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(folder[(117, 8)].bg, theme::LIGHTER_BACKGROUND);
        assert_eq!(folder[(89, 9)].bg, theme::BACKGROUND);
        assert_eq!(find(&folder, "Documents", 86..120), Some((89, 10)));
        assert_eq!(find(&folder, "Folder · No items", 86..120), Some((89, 11)));
        assert_eq!(find(&folder, "Open", 86..120), Some((90, 22)));

        // Too few rows for a preview and the rest: the rest wins.
        let short = buffer(&mut f.app, 120, 14);
        assert_eq!(find(&short, "Documents", 86..120), Some((89, 3)));
        assert_eq!(find(&short, "Folder · No items", 86..120), Some((89, 4)));
        assert_eq!(find(&short, "Open", 86..120), Some((90, 12)));
    }

    #[test]
    fn a_picture_that_arrives_under_the_shortcuts_waits_for_them_to_go() {
        let (mut f, _thumbnailer) = pictures();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        buffer(&mut f.app, 120, 24);
        press(&mut f.app, &[KeyCode::Char('?')]);
        settle(&mut f.app);
        // The overlay ends at column 102; the preview goes on to 110.
        let covered = buffer(&mut f.app, 120, 24);
        assert!(!painted(&covered[(105, 5)]));
        assert_eq!(covered[(105, 5)].bg, theme::LIGHTER_BACKGROUND);
        press(&mut f.app, &[KeyCode::Esc]);
        let clear = buffer(&mut f.app, 120, 24);
        assert!(painted(&clear[(95, 5)]) && painted(&clear[(105, 5)]));
        // Seen once, it stays when the shortcuts come back.
        press(&mut f.app, &[KeyCode::Char('?')]);
        let again = buffer(&mut f.app, 120, 24);
        assert!(painted(&again[(105, 5)]));
    }
}
