//! The preview pane, right of the list: a head with the item's name and
//! facts, then what it holds: a folder's items, a text's lines, a picture,
//! or what several selected items add up to.

use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::Image;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::list::{self, Look, size_text};
use super::{centered, fit, pad, pad_left, words};
use crate::app::{App, HitMap, Spot};
use crate::format;
use crate::icons;
use crate::model::{Category, Entry};
use crate::syntax::{Run, Runs};
use crate::theme;
use crate::thumbs::Facts;

/// The rows of the head: the name, the facts, a blank row.
const HEAD_ROWS: u16 = 3;
/// The most facts under a picture.
const FACTS: usize = 3;
/// The glyph before several selected items.
const FILES: &str = "\u{f0222}";

/// Draws the pane, which is a spot for the mouse.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect, hits: &mut HitMap) {
    hits.add(area, Spot::Preview);
    // A cell of padding on each side once the pane is at least 7 wide.
    let padding = u16::from(area.width >= 7);
    let inner = Rect {
        x: area.x + padding,
        width: area.width - 2 * padding,
        ..area
    };
    let body = Rect {
        y: area.y + HEAD_ROWS.min(area.height),
        height: area.height.saturating_sub(HEAD_ROWS),
        ..inner
    };
    app.preview_rows = body.height.max(1) as usize;
    let App {
        tabs,
        current,
        facts,
        thumbs,
        highlighter,
        preview_scroll,
        show_hidden,
        sort,
        date_style,
        ..
    } = app;
    let pane = tabs[*current].pane();
    if pane.selection().nth(1).is_some() {
        let selection: Vec<&Entry> = pane.selection().collect();
        let lines = several(&selection, inner.width as usize);
        frame.render_widget(Paragraph::new(lines), inner);
        let rows = |entry: &&Entry| quiet_row(entry, area.width as usize);
        let rows: Vec<Line> = selection.iter().map(rows).collect();
        scrolled(
            frame,
            rows,
            Rect {
                x: area.x,
                width: area.width,
                ..body
            },
            preview_scroll,
        );
        return;
    }
    let Some(entry) = pane.selected() else { return };
    let facts = facts
        .as_ref()
        .filter(|facts| facts.describes(entry, *show_hidden, *sort));
    let width = inner.width as usize;
    let head = vec![
        name_line(entry, width),
        Line::from(Span::styled(
            pad(&summary(entry, *date_style), width),
            theme::muted(),
        )),
    ];
    frame.render_widget(Paragraph::new(head), inner);
    if body.height == 0 {
        return;
    }
    let Some(facts) = facts else { return };
    if let Some(children) = &facts.children {
        match children {
            Ok(children) => {
                let rows = children
                    .iter()
                    .map(|child| quiet_row(child, area.width as usize))
                    .collect();
                scrolled(
                    frame,
                    rows,
                    Rect {
                        x: area.x,
                        width: area.width,
                        ..body
                    },
                    preview_scroll,
                );
            }
            Err(reason) => refused(frame, body, reason),
        }
    } else if !facts.opening.is_empty() {
        let coloured = highlighter
            .as_mut()
            .and_then(|highlighter| highlighter.get(&entry.path, entry.modified, &facts.opening));
        let rows = text_rows(&facts.opening, width, coloured);
        scrolled(frame, rows, body, preview_scroll);
    } else if matches!(
        entry.category(),
        Category::Image | Category::App | Category::Pdf
    ) {
        *preview_scroll = 0;
        let drawn = thumbs
            .as_mut()
            .and_then(|thumbs| picture(frame, thumbs, entry, body));
        let below = drawn.map_or(0, |height| height + 1);
        let facts_area = Rect {
            y: body.y + below.min(body.height),
            height: body.height.saturating_sub(below),
            ..body
        };
        let lines = picture_facts(entry, facts, width);
        frame.render_widget(Paragraph::new(lines), facts_area);
    }
}

/// The icon and the bold name, with where a link points after it.
fn name_line(entry: &Entry, width: usize) -> Line<'static> {
    let mark = if icons::has_marks() {
        format!("{} ", icons::of(entry))
    } else {
        String::new()
    };
    let marks = Some(super::tag_width(entry.tags)).filter(|marks| marks + 2 <= width);
    let name = fit(&format!("{mark}{}", entry.name), width - marks.unwrap_or(0));
    let used = name.width() + marks.unwrap_or(0);
    let mut spans = Vec::new();
    // Only the name is bold, not the glyph or the space after it.
    match name.strip_prefix(&mark) {
        Some(rest) if !mark.is_empty() => {
            let tint = theme::base().fg(theme::ink(icons::tint(entry)));
            spans.push(Span::styled(mark.clone(), tint));
            spans.push(Span::styled(rest.to_string(), theme::title()));
        }
        _ => spans.push(Span::styled(name, theme::title())),
    }
    if marks.is_some() {
        spans.extend(super::tag_marks(entry.tags, Style::default()));
    }
    if let Some(link) = &entry.link {
        let target = fit(
            &format!(" → {}", link.display()),
            width.saturating_sub(used),
        );
        spans.push(Span::styled(target, theme::muted()));
    }
    Line::from(spans)
}

/// `Kind · Size · Date`: folders are "Folder" and say how many items they hold.
fn summary(entry: &Entry, date_style: format::DateStyle) -> String {
    let kind = if entry.is_dir {
        "Folder".to_string()
    } else {
        entry.kind()
    };
    let size = if entry.is_dir {
        size_text(entry)
    } else {
        format::size(entry.size)
    };
    let date = entry
        .modified
        .map(|time| format::date_time(time, date_style));
    let parts = [Some(kind), Some(size).filter(|size| !size.is_empty()), date];
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// A row of a folder's items or of a selection, in the metadata ink, with its size.
pub(super) fn quiet_row(entry: &Entry, width: usize) -> Line<'static> {
    let look = Look {
        quiet: true,
        ..Look::default()
    };
    list::row(entry, width, Some(&size_text(entry)), look, "", None)
}

/// Rows from the preview's scroll on, as many as fit; the scroll is kept
/// where the last row is still in sight.
pub(super) fn scrolled(
    frame: &mut Frame,
    rows: Vec<Line<'static>>,
    area: Rect,
    scroll: &mut usize,
) {
    let height = area.height as usize;
    *scroll = (*scroll).min(rows.len().saturating_sub(height));
    let shown: Vec<Line> = rows.into_iter().skip(*scroll).take(height).collect();
    frame.render_widget(Paragraph::new(shown), area);
}

/// A text's lines under a right-aligned gutter of line numbers, long lines
/// wrapped with the number on their first part only.
pub(super) fn text_rows(
    lines: &[String],
    width: usize,
    coloured: Option<&[Runs]>,
) -> Vec<Line<'static>> {
    let gutter = lines.len().to_string().width();
    let room = width.saturating_sub(gutter + 1).max(1);
    let mut rows = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let plain = [(0, line.clone())];
        let runs = coloured
            .and_then(|coloured| coloured.get(index))
            .map_or(&plain[..], Vec::as_slice);
        for (part, pieces) in wrapped_runs(runs, room).into_iter().enumerate() {
            let number = if part == 0 {
                pad_left(&(index + 1).to_string(), gutter)
            } else {
                " ".repeat(gutter)
            };
            let mut spans = vec![Span::styled(format!("{number} "), theme::muted())];
            for (role, text) in pieces {
                let color = theme::SYNTAX[usize::from(role).min(theme::SYNTAX.len() - 1)];
                spans.push(Span::styled(text, theme::fg(color)));
            }
            rows.push(Line::from(spans));
        }
    }
    rows
}

/// Coloured runs cut into rows of at most `width` cells, each row its runs;
/// an empty line is one row.
fn wrapped_runs(runs: &[Run], width: usize) -> Vec<Runs> {
    let mut rows: Vec<Runs> = vec![Vec::new()];
    let mut used = 0;
    for (role, text) in runs {
        for c in text.chars() {
            let cells = c.width().unwrap_or(0);
            if used + cells > width && used > 0 {
                rows.push(Vec::new());
                used = 0;
            }
            let row = rows.last_mut().expect("never empty");
            match row.last_mut() {
                Some((last, run)) if last == role => run.push(c),
                _ => row.push((*role, c.to_string())),
            }
            used += cells;
        }
    }
    rows
}

/// A folder that can't be read: the lock in the error colour over why.
fn refused(frame: &mut Frame, area: Rect, reason: &str) {
    let width = area.width as usize;
    let said = words(reason, width.max(1));
    let top = area.height.saturating_sub(1 + said.len() as u16) / 2;
    let mut lines = vec![Line::from(Span::styled(
        centered(icons::mark(icons::LOCK, "!"), width),
        theme::error(),
    ))];
    lines.extend(
        said.iter()
            .map(|line| Line::from(Span::styled(centered(line, width), theme::base()))),
    );
    let rows = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    frame.render_widget(Paragraph::new(lines), rows);
}

/// Draws a picture at the top of `area`, in the middle across. Returns how
/// many rows it took, or `None` while it isn't ready. Nothing is read or
/// decoded here; a picture that isn't ready is asked for.
fn picture(
    frame: &mut Frame,
    thumbs: &mut crate::thumbs::Thumbs,
    entry: &Entry,
    area: Rect,
) -> Option<u16> {
    // Room under the picture for a blank row and the facts.
    let height = area.height.saturating_sub(1 + FACTS as u16).max(1);
    let size = Size::new(area.width, height);
    let protocol = thumbs.get(&entry.path, entry.modified, size)?;
    let drawn = protocol.size();
    let rect = Rect {
        x: area.x + area.width.saturating_sub(drawn.width) / 2,
        y: area.y,
        width: drawn.width.min(area.width),
        height: drawn.height.min(height),
    };
    frame.render_widget(Image::new(protocol), rect);
    Some(rect.height)
}

/// Up to three facts under a picture, `Label · value`.
fn picture_facts(entry: &Entry, facts: &Facts, width: usize) -> Vec<Line<'static>> {
    let pixels = facts
        .dimensions
        .map(|(across, down)| ("Pixels", format!("{across} × {down}")));
    // An app bundle's own size is its folder's, nothing like the app's.
    let size = (entry.category() != Category::App).then(|| ("Size", format::size(entry.size)));
    let all = [pixels, size, Some(("Kind", entry.kind()))];
    all.into_iter()
        .flatten()
        .take(FACTS)
        .map(|(label, value)| {
            Line::from(Span::styled(
                fit(&format!("{label} · {value}"), width),
                theme::muted(),
            ))
        })
        .collect()
}

/// The head for several selected items: how many, and their size together.
fn several(selection: &[&Entry], width: usize) -> Vec<Line<'static>> {
    let mark = if icons::has_marks() {
        format!("{FILES} ")
    } else {
        String::new()
    };
    let bytes: u64 = selection
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| entry.size)
        .sum();
    vec![
        Line::from(vec![
            Span::styled(mark.clone(), theme::base()),
            Span::styled(
                fit(
                    &format!("{} items selected", selection.len()),
                    width.saturating_sub(mark.width()),
                ),
                theme::title(),
            ),
        ]),
        Line::from(Span::styled(
            fit(&format!("Combined · {}", format::size(bytes)), width),
            theme::muted(),
        )),
    ]
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{ctrl, fixture, pictures, press, settle, shift};
    use crate::icons;
    use crate::theme;
    use crate::thumbs::testing::painted;
    use crate::ui::testing::{buffer, find};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Modifier;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// The preview's cells on one row, inside its padding, as text.
    fn row(drawn: &Buffer, y: u16) -> String {
        (75..119)
            .map(|x| drawn[(x, y)].symbol())
            .collect::<String>()
    }

    #[test]
    fn a_folder_shows_its_name_bold_its_facts_and_its_items_with_their_sizes() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down]);
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        // The head: the glyph, the name in bold, then what it is in the metadata ink.
        assert_eq!(drawn[(75, 2)].symbol(), icons::FOLDER);
        assert_eq!(find(&drawn, "Pictures", 74..120), Some((77, 2)));
        assert!(drawn[(77, 2)].modifier.contains(Modifier::BOLD));
        assert!(!drawn[(75, 2)].modifier.contains(Modifier::BOLD));
        assert!(
            row(&drawn, 3).starts_with("Folder · 2 items · "),
            "{}",
            row(&drawn, 3)
        );
        assert_eq!(drawn[(75, 3)].fg, theme::MUTED);
        assert_eq!(row(&drawn, 4).trim(), "");
        // Its items from the fourth row, quiet, with their sizes.
        assert_eq!(find(&drawn, "Trip", 74..120), Some((77, 5)));
        assert_eq!(drawn[(77, 5)].fg, theme::MUTED);
        assert!(find(&drawn, "No items", 74..120).is_some_and(|(_, y)| y == 5));
        assert_eq!(find(&drawn, "a.jpg", 74..120), Some((77, 6)));
        assert!(find(&drawn, "3 B", 74..120).is_some_and(|(_, y)| y == 6));
    }

    #[test]
    fn a_text_shows_its_lines_under_a_gutter_of_numbers_wrapped_when_long() {
        let mut f = fixture();
        let long = "y".repeat(50);
        let lines: String = (1..=12)
            .map(|n| {
                if n == 2 {
                    format!("{long}\n")
                } else {
                    format!("line {n}\n")
                }
            })
            .collect();
        fs::write(f.home.join("notes.txt"), lines).unwrap();
        f.app.handle_key(ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::End]);
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        assert!(
            row(&drawn, 3).starts_with("Plain text · "),
            "{}",
            row(&drawn, 3)
        );
        assert_eq!(row(&drawn, 5).trim_end(), " 1 line 1");
        assert_eq!(drawn[(76, 5)].fg, theme::MUTED, "the number");
        assert_eq!(drawn[(78, 5)].fg, theme::FOREGROUND, "the text");
        // 44 cells: a gutter of two, a space, 41 of text.
        assert_eq!(row(&drawn, 6), format!(" 2 {}", "y".repeat(41)));
        assert_eq!(row(&drawn, 7).trim_end(), format!("   {}", "y".repeat(9)));
        assert_eq!(row(&drawn, 8).trim_end(), " 3 line 3");
        assert_eq!(row(&drawn, 17).trim_end(), "12 line 12");
    }

    #[test]
    fn a_picture_shows_with_its_facts_under_it() {
        let (mut f, _thumbnailer) = pictures();
        let pictures = f.home.join("Pictures");
        image::GrayImage::new(1600, 1200)
            .save(pictures.join("harbour.png"))
            .unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let painted_rows: Vec<u16> = (5..28)
            .filter(|y| (75..119).any(|x| painted(&drawn[(x, *y)])))
            .collect();
        let last = *painted_rows.last().expect("the picture");
        assert_eq!(painted_rows[0], 5, "right under the head");
        // A blank row, then the facts.
        assert_eq!(row(&drawn, last + 1).trim(), "");
        assert_eq!(row(&drawn, last + 2).trim(), "Pixels · 1600 × 1200");
        assert!(row(&drawn, last + 3).starts_with("Size · "));
        assert_eq!(row(&drawn, last + 4).trim(), "Kind · PNG image");
        assert_eq!(drawn[(75, last + 2)].fg, theme::MUTED);
    }

    #[test]
    fn an_app_shows_its_icon_with_its_kind_under_it() {
        let (mut f, _thumbnailer) = pictures();
        // The fake draws any file; a file named like an app is one.
        fs::write(f.home.join("Pictures").join("Safari.app"), "x").unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        f.app.tick();
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "Safari.app", 74..120).map(|at| at.1), Some(2));
        let painted_rows: Vec<u16> = (5..28)
            .filter(|y| (75..119).any(|x| painted(&drawn[(x, *y)])))
            .collect();
        let last = *painted_rows.last().expect("the icon");
        assert_eq!(painted_rows[0], 5, "right under the head");
        // A bundle's own size says nothing about the app: only its kind.
        assert_eq!(row(&drawn, last + 1).trim(), "");
        assert_eq!(row(&drawn, last + 2).trim(), "Kind · Application");
        assert_eq!(row(&drawn, last + 3).trim(), "");
    }

    #[test]
    fn a_pdf_shows_its_first_page_with_its_size_and_kind_under_it() {
        let (mut f, _thumbnailer) = pictures();
        // The fake draws any file; Quick Look draws a PDF's first page.
        fs::write(f.home.join("Pictures").join("report.pdf"), "x").unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::End]);
        f.app.tick();
        assert_eq!(f.app.pane().selected().unwrap().name, "report.pdf");
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let painted_rows: Vec<u16> = (5..28)
            .filter(|y| (75..119).any(|x| painted(&drawn[(x, *y)])))
            .collect();
        let last = *painted_rows.last().expect("the page");
        assert_eq!(painted_rows[0], 5, "right under the head");
        assert_eq!(row(&drawn, last + 1).trim(), "");
        assert!(row(&drawn, last + 2).starts_with("Size · "));
        assert_eq!(row(&drawn, last + 3).trim(), "Kind · PDF document");
    }

    #[test]
    fn code_shows_in_bat_s_colours_once_they_are_ready() {
        use crate::syntax::Highlighter;
        use crate::syntax::testing::FakeColourer;
        use std::sync::Arc;
        let mut f = fixture();
        fs::write(f.home.join("main.rs"), "fn main\nlet s = \"hi\"\n").unwrap();
        f.app.highlighter = Some(Highlighter::new(Arc::new(FakeColourer::default())));
        f.app
            .handle_key(crate::app::testing::ctrl(KeyCode::Char('r')));
        press(&mut f.app, &[KeyCode::Down, KeyCode::Down]);
        f.app.tick();
        assert_eq!(f.app.pane().selected().unwrap().name, "main.rs");
        // Plain until the colours come back.
        let first = buffer(&mut f.app, 120, 30);
        let (x, y) = find(&first, "fn main", 74..120).expect("the code");
        assert_eq!(first[(x, y)].fg, theme::FOREGROUND);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(find(&drawn, "fn main", 74..120), Some((x, y)));
        let ink = |at: (u16, u16)| drawn[at].fg;
        // The line number stays muted; `fn` magenta, the rest the foreground.
        assert_eq!(ink((x - 2, y)), theme::MUTED);
        assert_eq!(ink((x, y)), theme::SYNTAX[5]);
        assert_eq!(ink((x + 1, y)), theme::SYNTAX[5]);
        assert_eq!(ink((x + 3, y)), theme::FOREGROUND);
        // A lone `=` is calmed to the foreground; the string green.
        let (s, line) = find(&drawn, "let s = \"hi\"", 74..120).expect("line 2");
        assert_eq!(ink((s + 6, line)), theme::FOREGROUND);
        assert_eq!(ink((s + 8, line)), theme::SYNTAX[2]);
        assert_eq!(ink((s + 11, line)), theme::SYNTAX[2]);
    }

    #[test]
    fn several_selected_items_say_how_many_and_their_size_then_list_them() {
        let mut f = fixture();
        fs::write(f.home.join("Pictures").join("b.jpg"), "four").unwrap();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        f.app.handle_key(shift(KeyCode::Down));
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        assert_eq!(
            row(&drawn, 2).trim(),
            format!("{} 2 items selected", "\u{f0222}")
        );
        assert!(drawn[(77, 2)].modifier.contains(Modifier::BOLD));
        assert_eq!(row(&drawn, 3).trim(), "Combined · 7 B");
        assert_eq!(find(&drawn, "a.jpg", 74..120), Some((77, 5)));
        assert_eq!(find(&drawn, "b.jpg", 74..120), Some((77, 6)));
    }

    #[test]
    fn a_folder_that_cannot_be_read_shows_the_lock_and_why() {
        let mut f = fixture();
        let documents = f.home.join("Documents");
        fs::set_permissions(&documents, fs::Permissions::from_mode(0o000)).unwrap();
        f.app.tick();
        let drawn = buffer(&mut f.app, 120, 30);
        fs::set_permissions(&documents, fs::Permissions::from_mode(0o755)).unwrap();
        let (x, y) = find(&drawn, icons::LOCK, 74..120).expect("the lock");
        assert_eq!(drawn[(x, y)].fg, theme::ERROR);
        let reason = "You don’t have permission to open this folder.";
        let (_, first) = find(&drawn, "You don’t have permission", 74..120).expect("why");
        assert_eq!(first, y + 1);
        let said: String = (first..first + 2)
            .map(|y| row(&drawn, y).trim().to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(said, reason);
    }
}
