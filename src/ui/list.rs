//! The list: flea's rows, an edge, a glyph, the name and the detail column
//! (`I`).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::{AppRows, app_icon, centered, empty_text, fit, note_app, scroll, tag_marks, tag_width};
use crate::app::{App, Detail, HitMap, Pane, Spot};
use crate::format;
use crate::format::DateStyle;
use crate::icons;
use crate::matcher::Query;
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
    /// It is in a column of a folder above, in the columns view: muted.
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
    // The tags go right after the name, when two cells of it still show.
    let marks = Some(tag_width(entry.tags)).filter(|marks| marks + 2 <= name_width);
    let marks_width = marks.unwrap_or(0);
    let name_room = name_width - marks_width;
    let name = fit(&entry.name, entry.name.width().min(name_room));
    let room = name_room - name.width();
    let after = match (folder, &entry.link) {
        (Some(folder), _) => fit(&format!(" {folder}"), room),
        (None, Some(link)) => fit(&format!(" → {}", link.display()), room),
        (None, None) => String::new(),
    };
    let mut spans = vec![
        edge,
        Span::styled(format!("{} ", icons::of(entry)), text(icons::tint(entry))),
    ];
    spans.extend(bold_match(&name, query, text(ink)));
    if marks.is_some() {
        spans.extend(tag_marks(entry.tags, fill));
    }
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
    // The preview's and the columns above are never read from here; what
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

/// A name in pieces, what `query` matches in it bold: the filter's match,
/// exact or close, or what Find looked for. A name cut short keeps what of
/// it shows.
pub fn bold_match(name: &str, query: &str, style: Style) -> Vec<Span<'static>> {
    let bold = bold_chars(name, query);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = (String::new(), false);
    for (index, c) in name.chars().enumerate() {
        let lit = bold.contains(&index);
        if lit != run.1 && !run.0.is_empty() {
            let text = std::mem::take(&mut run.0);
            spans.push(styled(text, style, run.1));
        }
        run = (std::mem::take(&mut run.0), lit);
        run.0.push(c);
    }
    spans.push(styled(run.0, style, run.1));
    spans
}

fn styled(text: String, style: Style, bold: bool) -> Span<'static> {
    match bold {
        true => Span::styled(text, style.add_modifier(Modifier::BOLD)),
        false => Span::styled(text, style),
    }
}

/// Which characters of a name `query` matches, by place.
fn bold_chars(name: &str, query: &str) -> Vec<usize> {
    let Some(query) = Query::new(query) else {
        return Vec::new();
    };
    let shown = name.trim_end_matches('…');
    let runs = query.runs(shown);
    runs.iter()
        .flat_map(|run| run.start..run.start + run.len)
        .collect()
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

    let query = pane.matching();
    let width = area.width as usize;
    let mut lines = Vec::new();
    let mut apps = AppRows::new();
    for (item, entry) in pane.entries.iter().enumerate().skip(pane.offset).take(rows) {
        let y = area.y + lines.len() as u16;
        let spot = Spot::Item {
            pane: index,
            index: item,
        };
        let line = Rect::new(area.x, y, area.width, 1);
        hits.add(line, spot);
        note_app(&mut apps, entry, line);
        let look = Look {
            cursor: item == pane.cursor,
            focused,
            marked: pane.is_marked(item),
            ..Look::default()
        };
        // What was found below says which folder it is in.
        let folder = pane.place_of(entry);
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
    for (entry, line) in apps {
        app_icon(frame, app.thumbs.as_mut(), &entry, line);
    }
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
    let glyph = if !pane.matching().is_empty() {
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
    use crate::app::testing::{ctrl, key, pictures_through, settle};
    use crate::ui::testing::{buffer, find, panes};
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;
    use ratatui_image::picker::ProtocolType;
    use std::fs;
    use std::path::Path;

    /// Whether a cell holds a piece of a picture the terminal draws itself.
    fn pictured(cell: &ratatui::buffer::Cell) -> bool {
        cell.symbol().contains('\u{10EEEE}')
    }

    /// The cells left of a name in a range of columns, where its glyph is.
    fn glyph_slot(drawn: &Buffer, name: &str, columns: std::ops::Range<u16>) -> [(u16, u16); 2] {
        let (x, y) = find(drawn, name, columns).expect(name);
        [(x - 2, y), (x - 1, y)]
    }

    #[test]
    fn an_app_shows_its_icon_where_its_glyph_was_in_the_list_and_the_columns() {
        for view in ['1', '2'] {
            let (mut f, _thumbnailer) = pictures_through(ProtocolType::Kitty);
            fs::write(f.home.join("Safari.app"), "x").unwrap();
            f.app.handle_key(key(KeyCode::F(5)));
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let columns = panes(&f.app, 120).files;
            // Until the icon is ready, the glyph shows.
            let first = buffer(&mut f.app, 120, 30);
            let [glyph, space] = glyph_slot(&first, "Safari.app", columns.clone());
            assert_eq!(
                first[glyph].symbol(),
                icons::of(&entry(&f.home, "Safari.app"))
            );
            assert_eq!(first[space].symbol(), " ");
            settle(&mut f.app);
            let drawn = buffer(&mut f.app, 120, 30);
            for at in glyph_slot(&drawn, "Safari.app", columns.clone()) {
                assert!(pictured(&drawn[at]), "{view} {at:?}");
            }
            // Other files keep their glyphs.
            for at in glyph_slot(&drawn, "notes.txt", columns.clone()) {
                assert!(!pictured(&drawn[at]), "{view} {at:?}");
            }
        }
        // A terminal that draws pictures with half blocks keeps the glyph:
        // two cells of them make no icon.
        let (mut f, _thumbnailer) = pictures_through(ProtocolType::Halfblocks);
        fs::write(f.home.join("Safari.app"), "x").unwrap();
        f.app.handle_key(key(KeyCode::F(5)));
        buffer(&mut f.app, 120, 30);
        settle(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let files = panes(&f.app, 120).files;
        let [glyph, _] = glyph_slot(&drawn, "Safari.app", files);
        assert_eq!(
            drawn[glyph].symbol(),
            icons::of(&entry(&f.home, "Safari.app"))
        );
    }

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
            format!(" {} link → a.jpg{}", "\u{f1177}", " ".repeat(9))
        );
        let found = entry(dir.path(), "a.jpg");
        let line = row(&found, 24, None, Look::default(), "", Some("Trip/Day"));
        assert_eq!(
            text(&line),
            format!(" {} a.jpg Trip/Day{}", "\u{f021f}", " ".repeat(7))
        );
        // Cut short with the name kept whole.
        let line = row(&found, 14, None, Look::default(), "", Some("Trip/Day"));
        assert_eq!(text(&line), format!(" {} a.jpg Tri… ", "\u{f021f}"));
    }

    fn tag(path: &Path, labels: &[&str]) {
        let labels: Vec<String> = labels.iter().map(|label| label.to_string()).collect();
        crate::tags::write(path, Some(&crate::tags::encode(&labels))).unwrap();
    }

    #[test]
    fn tags_show_as_squares_in_their_colours_after_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        fs::write(&path, "hello").unwrap();
        tag(&path, &["Blue\n4", "Work", "Red\n6"]);
        let notes = entry(dir.path(), "notes.txt");
        let glyph = icons::TEXT;
        let line = row(&notes, 24, Some("5 B"), Look::default(), "", None);
        // In the order of the seven, whatever order they were added in.
        assert_eq!(text(&line), format!(" {glyph} notes.txt ■■    5 B  "));
        let squares: Vec<Option<Color>> = (line.spans.iter())
            .filter(|span| span.content == "■")
            .map(|span| span.style.fg)
            .collect();
        assert_eq!(squares, [Some(theme::TAGS[0]), Some(theme::TAGS[4])]);
        // A name too long for them is cut first.
        let line = row(&notes, 20, Some("5 B"), Look::default(), "", None);
        assert_eq!(text(&line), format!(" {glyph} notes.t… ■■ 5 B  "));
        // Without colour, each is its digit.
        theme::set_depth(theme::Depth::Mono);
        let line = row(&notes, 24, Some("5 B"), Look::default(), "", None);
        theme::set_depth(theme::Depth::Truecolor);
        assert_eq!(text(&line), format!(" {glyph} notes.txt 15    5 B  "));
        // In 256 colours, the nearest of the palette.
        theme::set_depth(theme::Depth::Indexed);
        let line = row(&notes, 24, Some("5 B"), Look::default(), "", None);
        theme::set_depth(theme::Depth::Truecolor);
        let first = line.spans.iter().find(|span| span.content == "■").unwrap();
        assert!(matches!(first.style.fg, Some(Color::Indexed(_))));
    }

    #[test]
    fn tags_show_in_the_list_the_columns_the_grid_and_the_preview_head() {
        use crate::app::testing::{ctrl, fixture, key, press};
        use crate::ui::testing::{buffer, find};
        use ratatui::crossterm::event::KeyCode;
        let mut f = fixture();
        tag(&f.home.join("notes.txt"), &["Green\n2"]);
        press(&mut f.app, &[KeyCode::F(5), KeyCode::End]);
        let green = theme::TAGS[3];
        let after = |drawn: &ratatui::buffer::Buffer, at: (u16, u16)| {
            let cell = &drawn[(at.0 + "notes.txt".len() as u16 + 1, at.1)];
            (cell.symbol().to_string(), cell.fg)
        };
        let square = ("■".to_string(), green);
        // The list, and the preview's head on the right.
        let drawn = buffer(&mut f.app, 120, 30);
        let panes = crate::ui::testing::panes(&f.app, 120);
        let listed = find(&drawn, "notes.txt", panes.files).expect("the row");
        assert_eq!(after(&drawn, listed), square);
        let head = find(&drawn, "notes.txt", panes.preview).expect("the preview's head");
        assert_eq!(after(&drawn, head), square);
        // The columns.
        f.app.handle_key(ctrl(KeyCode::Char('2')));
        let drawn = buffer(&mut f.app, 120, 30);
        let listed = find(&drawn, "notes.txt", 0..80).expect("the row");
        assert_eq!(after(&drawn, listed), square);
        // The grid's caption.
        f.app.handle_key(ctrl(KeyCode::Char('3')));
        f.app.handle_key(key(KeyCode::F(5)));
        let drawn = buffer(&mut f.app, 120, 30);
        let caption = find(&drawn, "notes.txt", 0..80).expect("the caption");
        assert_eq!(after(&drawn, caption), square);
    }

    #[test]
    fn what_the_query_matches_is_bold_whatever_its_case_and_however_close() {
        let bold = |name: &str, query: &str| -> Vec<(String, bool)> {
            let spans = bold_match(name, query, Style::default());
            let lit = |span: &Span| span.style.add_modifier.contains(Modifier::BOLD);
            spans
                .iter()
                .map(|span| (span.content.to_string(), lit(span)))
                .collect()
        };
        let pieces = |pieces: &[(&str, bool)]| -> Vec<(String, bool)> {
            pieces
                .iter()
                .map(|(text, lit)| (text.to_string(), *lit))
                .collect()
        };
        assert_eq!(
            bold("Harbour.JPG", "jpg"),
            pieces(&[("Harbour.", false), ("JPG", true)])
        );
        assert_eq!(
            bold("café menu", "É M"),
            pieces(&[("caf", false), ("é m", true), ("enu", false)])
        );
        assert_eq!(bold("notes", ""), pieces(&[("notes", false)]));
        assert_eq!(bold("notes", "x"), pieces(&[("notes", false)]));
        // A close match lights its letters.
        assert_eq!(
            bold("a_b", "ab"),
            pieces(&[("a", true), ("_", false), ("b", true)])
        );
    }
}
