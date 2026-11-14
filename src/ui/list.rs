//! The list: flea's rows, an edge, a glyph, the name and the detail column
//! (`I`), and the parent folder's column on its left.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{centered, empty_text, fit, scroll};
use crate::app::{App, Detail, HitMap, Pane, Spot};
use crate::format;
use crate::format::DateStyle;
use crate::icons;
use crate::model::{self, Entry};
use crate::theme;

/// What a row is, which decides its colours.
#[derive(Clone, Copy, Debug, Default)]
pub struct Look {
    /// It is the row the cursor is on.
    pub cursor: bool,
    /// Its list has the keyboard.
    pub focused: bool,
    /// It is one of several selected.
    pub marked: bool,
    /// It is in the parent folder's column: muted.
    pub parent: bool,
    /// It is in the preview: in the metadata ink.
    pub quiet: bool,
}

/// One row `width` cells wide: the edge, the glyph, the name (with where a
/// link points, or the folder a search found it in, after it), and `size`
/// when it fits. What `query` matches in the name is bold.
pub fn row(
    entry: &Entry,
    width: usize,
    size: Option<&str>,
    look: Look,
    query: &str,
    folder: Option<&str>,
) -> Line<'static> {
    let (ink, meta) = inks(entry, look);
    let fill = theme::row_fill(look.cursor, look.focused, look.marked);
    let text = |color| fill.fg(theme::ink(color));
    if width == 0 {
        return Line::default();
    }
    let reverses = theme::depth().reverses();
    let edge = if look.cursor && look.focused && !reverses {
        Span::styled("▎", text(theme::ACCENT))
    } else {
        Span::styled(" ", text(ink))
    };
    if width < 4 {
        let name = fit(&entry.name, width - 1);
        return Line::from(vec![edge, Span::styled(name, text(ink))]);
    }
    let size = size.filter(|size| !size.is_empty() && size.width() + 6 <= width);
    let name_width = width - 3 - size.map_or(1, |size| size.width() + 3);
    let name = fit(&entry.name, entry.name.width().min(name_width));
    let room = name_width - name.width();
    let after = match (folder, &entry.link) {
        (Some(folder), _) => fit(&format!(" {folder}"), room),
        (None, Some(link)) => fit(&format!(" → {}", link.display()), room),
        (None, None) => String::new(),
    };
    let mut spans = vec![
        edge,
        Span::styled(format!("{} ", icons::of(entry)), text(meta)),
    ];
    spans.extend(bold_match(&name, query, text(ink)));
    let after_width = after.width();
    spans.push(Span::styled(after, text(meta)));
    spans.push(Span::styled(" ".repeat(room - after_width), text(ink)));
    match size {
        Some(size) => {
            spans.push(Span::styled(" ", text(ink)));
            spans.push(Span::styled(format!("{size}  "), text(meta)));
        }
        None => spans.push(Span::styled(" ", text(ink))),
    }
    Line::from(spans)
}

/// The colour of an item's name and of what goes with it (the glyph, the
/// detail, where a link points), the first rule that fits winning: what
/// can't be read muted, under the cursor or marked the foreground, links
/// cyan, programs green, everything else the foreground.
pub fn inks(entry: &Entry, look: Look) -> (Color, Color) {
    let lifted = look.cursor || look.marked;
    // The preview's and the parent's rows are never read from here; what
    // can't be read shows as such only in the list itself.
    let dimmed = !look.parent && !look.quiet && entry.unreadable();
    let ink = if dimmed {
        theme::MUTED
    } else if lifted {
        theme::FOREGROUND
    } else if look.parent || look.quiet {
        theme::MUTED
    } else if entry.link.is_some() {
        theme::SYMLINK
    } else if !entry.is_dir && entry.mode & 0o111 != 0 {
        theme::EXECUTABLE
    } else {
        theme::FOREGROUND
    };
    let meta = if lifted && !dimmed {
        theme::FOREGROUND
    } else {
        theme::MUTED
    };
    (ink, meta)
}

/// A name in pieces, what `query` matches in it bold.
pub fn bold_match(name: &str, query: &str, style: Style) -> Vec<Span<'static>> {
    match match_range(name, query) {
        Some((start, end)) => vec![
            Span::styled(name[..start].to_string(), style),
            Span::styled(
                name[start..end].to_string(),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(name[end..].to_string(), style),
        ],
        None => vec![Span::styled(name.to_string(), style)],
    }
}

/// Where `query` first appears in `text`, whatever the case, as byte offsets.
fn match_range(text: &str, query: &str) -> Option<(usize, usize)> {
    if query.is_empty() {
        return None;
    }
    let lower: Vec<(usize, char)> = text
        .char_indices()
        .flat_map(|(at, c)| c.to_lowercase().map(move |lower| (at, lower)))
        .collect();
    let wanted: Vec<char> = query.to_lowercase().chars().collect();
    let start = lower
        .windows(wanted.len())
        .position(|window| window.iter().map(|(_, c)| *c).eq(wanted.iter().copied()))?;
    let first = lower[start].0;
    let last = lower[start + wanted.len() - 1].0;
    let end = last + text[last..].chars().next().map_or(0, char::len_utf8);
    Some((first, end))
}

/// Draws one pane's items as rows; `focused` is whether it has the keyboard.
/// Each row is a spot for the mouse.
pub fn draw(
    frame: &mut Frame,
    app: &mut App,
    index: usize,
    area: Rect,
    focused: bool,
    hits: &mut HitMap,
) {
    let caption = app.caption;
    let (detail, style) = (app.detail, app.date_style);
    let now = std::time::SystemTime::now();
    let pane = &mut app.tab_mut().panes[index];
    let rows = area.height as usize;
    pane.page_rows = rows.max(1);
    if pane.entries.is_empty() {
        empty(frame, area, pane, caption);
        return;
    }
    pane.offset = scroll(pane.offset, pane.cursor, rows);

    let query = pane
        .results()
        .map(|search| search.input.text())
        .unwrap_or_default();
    let searched = pane.results().is_some();
    let width = area.width as usize;
    let mut lines = Vec::new();
    for (item, entry) in pane.entries.iter().enumerate().skip(pane.offset).take(rows) {
        let y = area.y + lines.len() as u16;
        let spot = Spot::Item {
            pane: index,
            index: item,
        };
        hits.add(Rect::new(area.x, y, area.width, 1), spot);
        let look = Look {
            cursor: item == pane.cursor,
            focused,
            marked: pane.is_marked(item),
            ..Look::default()
        };
        // What a search found says which folder below the searched one it is in.
        let folder = entry
            .path
            .parent()
            .filter(|_| searched)
            .and_then(|parent| parent.strip_prefix(&pane.cwd).ok())
            .map(|folder| folder.display().to_string())
            .filter(|folder| !folder.is_empty());
        let value = detail_text(entry, detail, now, style);
        lines.push(row(
            entry,
            width,
            Some(&value),
            look,
            &query,
            folder.as_deref(),
        ));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The parent folder's items, left of the list: muted, without sizes, the
/// folder the list shows on the cursor's fill. Empty at `/`. Each row is a
/// spot for the mouse.
pub fn draw_parent(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    let pane = app.pane();
    let current = pane.parent.iter().position(|entry| entry.path == pane.cwd);
    let rows = area.height as usize;
    let offset = scroll(0, current.unwrap_or(0), rows);
    let mut lines = Vec::new();
    for (index, entry) in pane.parent.iter().enumerate().skip(offset).take(rows) {
        let y = area.y + lines.len() as u16;
        hits.add(Rect::new(area.x, y, area.width, 1), Spot::Parent(index));
        let look = Look {
            cursor: current == Some(index),
            parent: true,
            ..Look::default()
        };
        lines.push(row(entry, area.width as usize, None, look, "", None));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// What the detail column says about an item: its size, when it was
/// changed, its mode the way `ls -l` writes it, or who owns it.
pub fn detail_text(
    entry: &Entry,
    detail: Detail,
    now: std::time::SystemTime,
    style: DateStyle,
) -> String {
    match detail {
        Detail::Size => size_text(entry),
        Detail::Modified => entry
            .modified
            .map(|time| format::short_date(time, now, style))
            .unwrap_or_default(),
        Detail::Mode => format::permissions(entry.mode),
        Detail::Owner => model::owner(entry.uid),
    }
}

/// A file's size, or how many things a folder holds.
pub fn size_text(entry: &Entry) -> String {
    match entry.item_count() {
        Some(count) => format::items(count),
        None if entry.is_dir => String::new(),
        None => format::size(entry.size),
    }
}

/// What a pane with no items says, in the middle: a glyph over flea's
/// caption for an empty folder, or the search mark over what matched nothing.
pub fn empty(frame: &mut Frame, area: Rect, pane: &Pane, caption: usize) {
    let text = empty_text(pane, caption);
    if text.is_empty() {
        return;
    }
    let glyph = if pane.results().is_some() {
        icons::mark(icons::SEARCH, "›")
    } else {
        icons::mark(icons::FOLDER, "›")
    };
    notice(frame, area, glyph, &text, theme::muted());
}

/// A glyph over a line of text, in the middle of an area.
pub fn notice(frame: &mut Frame, area: Rect, glyph: &str, text: &str, style: Style) {
    let top = area.height.saturating_sub(3) / 2;
    let width = area.width as usize;
    let lines = vec![
        Line::from(Span::styled(centered(glyph, width), style)),
        Line::from(Span::styled(centered(text, width), style)),
    ];
    let rows = Rect {
        y: area.y + top,
        height: 2.min(area.height.saturating_sub(top)),
        ..area
    };
    frame.render_widget(Paragraph::new(lines), rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn entry(dir: &Path, name: &str) -> Entry {
        Entry::from_path(dir.join(name)).unwrap()
    }

    #[test]
    fn a_row_fills_its_width_and_drops_the_size_when_it_does_not_fit() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let notes = entry(dir.path(), "notes.txt");
        let row = |width| text(&row(&notes, width, Some("5 B"), Look::default(), "", None));
        let glyph = icons::TEXT;
        assert_eq!(row(20), format!(" {glyph} notes.txt   5 B  "));
        // The size needs its own cells and six more; then the name is cut.
        assert_eq!(row(10), format!(" {glyph} … 5 B  "));
        assert_eq!(row(8), format!(" {glyph} not… "));
        assert_eq!(row(8).chars().count(), 8);
        // Narrower than four: the edge and the name.
        assert_eq!(row(3), " n…");
        assert_eq!(row(0), "");
    }

    #[test]
    fn a_link_says_where_it_points_and_a_search_result_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.jpg"), "").unwrap();
        std::os::unix::fs::symlink("a.jpg", dir.path().join("link")).unwrap();
        let link = entry(dir.path(), "link");
        let line = row(&link, 24, None, Look::default(), "", None);
        assert_eq!(
            text(&line),
            format!(" {} link → a.jpg{}", "\u{eaee}", " ".repeat(9))
        );
        let found = entry(dir.path(), "a.jpg");
        let line = row(&found, 24, None, Look::default(), "", Some("Trip/Day"));
        assert_eq!(
            text(&line),
            format!(" {} a.jpg Trip/Day{}", "\u{eaea}", " ".repeat(7))
        );
        // Cut short with the name kept whole.
        let line = row(&found, 14, None, Look::default(), "", Some("Trip/Day"));
        assert_eq!(text(&line), format!(" {} a.jpg Tri… ", "\u{eaea}"));
    }

    #[test]
    fn what_the_query_matches_is_found_whatever_its_case() {
        assert_eq!(match_range("Harbour.JPG", "jpg"), Some((8, 11)));
        assert_eq!(match_range("café menu", "É M"), Some((3, 7)));
        assert_eq!(match_range("notes", ""), None);
        assert_eq!(match_range("notes", "x"), None);
    }
}
