//! The list: rows of an edge, a glyph, the name and the detail column
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
use crate::model::{SortKey, SortMode};
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
    let detail = size.map(|size| vec![(size.to_string(), None)]);
    painted_row(entry, width, detail.as_deref(), look, query, folder)
}

/// A piece of a row's detail and its colour; with none, the metadata ink.
pub type Piece = (String, Option<Color>);

/// A row like [`row`], its detail in pieces, each in its own colour (the
/// metadata ink for those without one, and for all of an item that can't
/// be read).
pub fn painted_row(
    entry: &Entry,
    width: usize,
    detail: Option<&[Piece]>,
    look: Look,
    query: &str,
    folder: Option<&str>,
) -> Line<'static> {
    let (ink, meta) = inks(entry, look);
    let plain = !look.parent && !look.quiet && entry.unreadable();
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
    let cells = |pieces: &[Piece]| pieces.iter().map(|(text, _)| text.width()).sum::<usize>();
    let size = detail.filter(|pieces| cells(pieces) > 0 && cells(pieces) + 6 <= width);
    let name_width = width - 3 - size.map_or(1, |pieces| cells(pieces) + 3);
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
        Some(pieces) => {
            spans.push(Span::styled(" ", text(ink)));
            for (piece, color) in pieces {
                let color = color.filter(|_| !plain).unwrap_or(meta);
                spans.push(Span::styled(piece.clone(), text(color)));
            }
            spans.push(Span::styled("  ", text(meta)));
        }
        None => spans.push(Span::styled(" ", text(ink))),
    }
    Line::from(spans)
}

/// A column after the names in the list and the two panes, in the order
/// they stand; the last ones give way first when a pane is narrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Size,
    User,
    Permissions,
    Kind,
    Modified,
}

pub const COLUMNS: [Column; 5] = [
    Column::Size,
    Column::User,
    Column::Permissions,
    Column::Kind,
    Column::Modified,
];

/// The fewest cells a name keeps before a column gives way to it.
const NAME_ROOM: usize = 20;
/// The cells between two columns.
const GAP: usize = 2;
/// The cells of a row around its name and columns: the edge, the glyph and
/// its space, a space before the columns and two after them.
const ROW_CHROME: usize = 6;

impl Column {
    /// What the header says over it.
    pub fn title(self) -> &'static str {
        match self {
            Column::Size => "Size",
            Column::User => "User",
            Column::Permissions => "Permissions",
            Column::Kind => "Kind",
            Column::Modified => "Modified",
        }
    }

    /// The sort key it shows, when there is one.
    fn key(self) -> Option<SortKey> {
        match self {
            Column::Size => Some(SortKey::Size),
            Column::Kind => Some(SortKey::Kind),
            Column::Modified => Some(SortKey::Modified),
            Column::User | Column::Permissions => None,
        }
    }

    /// The widest it grows; longer values are cut with `…`.
    fn widest(self) -> usize {
        match self {
            Column::Size => 11,
            Column::User => 16,
            Column::Permissions => 10,
            Column::Kind => 20,
            Column::Modified => 16,
        }
    }

    /// Sizes line up on their right, everything else on its left.
    fn right(self) -> bool {
        self == Column::Size
    }

    /// What it says about an item.
    pub fn value(self, entry: &Entry, now: std::time::SystemTime, style: DateStyle) -> String {
        match self {
            Column::Size => size_text(entry),
            Column::User => model::owner(entry.uid),
            Column::Permissions => format::permissions(entry.mode),
            Column::Kind => entry.kind(),
            Column::Modified => detail_text(entry, Detail::Modified, now, style),
        }
    }

    /// The cells its title needs, with room for the sort arrow when it sorts.
    fn title_width(self) -> usize {
        self.title().width() + if self.key().is_some() { 2 } else { 0 }
    }
}

/// The columns a list `width` cells wide shows for `entries`, of those
/// `keep` says (in [`COLUMNS`]' order), each with its
/// width: its widest value or its title, up to [`Column::widest`]. Columns
/// stand while the name keeps [`NAME_ROOM`] cells.
pub fn table(
    entries: &[Entry],
    width: usize,
    now: std::time::SystemTime,
    style: DateStyle,
    keep: [bool; 5],
) -> Vec<(Column, usize)> {
    let mut shown = Vec::new();
    let mut used = ROW_CHROME + NAME_ROOM;
    let kept = COLUMNS.into_iter().zip(keep).filter(|(_, kept)| *kept);
    for (column, _) in kept {
        let widest = (entries.iter())
            .map(|entry| column.value(entry, now, style).width())
            .max()
            .unwrap_or(0);
        let cells = widest.min(column.widest()).max(column.title_width());
        let needs = cells + if shown.is_empty() { 0 } else { GAP };
        if used + needs > width {
            break;
        }
        used += needs;
        shown.push((column, cells));
    }
    shown
}

/// Texts set in a table's columns, a gap apart: each cut to its column and
/// lined up on its side.
fn set(table: &[(Column, usize)], texts: impl Iterator<Item = String>) -> String {
    let cells: Vec<String> = (table.iter().zip(texts))
        .map(|(&(column, width), text)| {
            let text = fit(&text, width);
            let pad = " ".repeat(width - text.width());
            match column.right() {
                true => format!("{pad}{text}"),
                false => format!("{text}{pad}"),
            }
        })
        .collect();
    cells.join(&" ".repeat(GAP))
}

/// An item's values set in a table's columns like [`set`], in eza's
/// colours: a mode letter by letter (`d` blue, `l` the link colour, `r`
/// yellow, `w` red, `x` green, `-` dimmed), a size's number green and its
/// unit muted (a folder's count muted), its owner yellow when it is the user
/// running the app, its kind in its glyph's colour, its date blue.
pub fn paint(
    table: &[(Column, usize)],
    entry: &Entry,
    now: std::time::SystemTime,
    style: DateStyle,
    me: u32,
) -> Vec<Piece> {
    let (red, yellow, green, blue) = (
        theme::TAGS[0],
        theme::TAGS[2],
        theme::TAGS[3],
        theme::TAGS[4],
    );
    let mut pieces = Vec::new();
    for (at, &(column, width)) in table.iter().enumerate() {
        if at > 0 {
            pieces.push((" ".repeat(GAP), None));
        }
        let text = fit(&column.value(entry, now, style), width);
        let pad = (" ".repeat(width - text.width()), None);
        if column.right() {
            pieces.push(pad.clone());
        }
        let cut = text.ends_with('…');
        match column {
            Column::Size if entry.is_dir || cut => pieces.push((text, None)),
            Column::Size => match text.split_once(' ') {
                Some((number, unit)) => {
                    pieces.push((number.to_string(), Some(green)));
                    pieces.push((format!(" {unit}"), None));
                }
                None => pieces.push((text, Some(green))),
            },
            Column::User => pieces.push((text, (entry.uid == me).then_some(yellow))),
            Column::Permissions => pieces.extend(text.chars().enumerate().map(|(at, c)| {
                let color = match c {
                    'd' if at == 0 => theme::TAGS[4],
                    'l' if at == 0 => theme::SYMLINK,
                    'r' => yellow,
                    'w' => red,
                    'x' | 's' | 't' | 'S' | 'T' => green,
                    _ => theme::DIMMED,
                };
                (c.to_string(), Some(color))
            })),
            Column::Kind => pieces.push((text, Some(icons::tint(entry)))),
            Column::Modified => pieces.push((text, Some(blue))),
        }
        if !column.right() {
            pieces.push(pad);
        }
    }
    pieces
}

/// A title, with `▴` or `▾` after it when the items are sorted by it.
fn titled(title: &str, key: Option<SortKey>, sort: SortMode) -> String {
    match key == Some(sort.key) {
        true => format!("{title} {}", if sort.reversed { "▾" } else { "▴" }),
        false => title.to_string(),
    }
}

/// The header over a list `width` cells wide: "Name" over the names, then
/// each column's title over it, bold in the muted colour; the sorted one in
/// the foreground with its arrow. Drawn on the tab fill.
pub fn header(table: &[(Column, usize)], width: usize, sort: SortMode) -> Line<'static> {
    let parts = header_parts(table, width, sort);
    Line::from(parts.into_iter().map(|(span, _)| span).collect::<Vec<_>>())
}

/// Where a click on the header sorts: the cells of each title that sorts,
/// from the list's left edge, with its key.
pub fn sort_spots(
    table: &[(Column, usize)],
    width: usize,
    sort: SortMode,
) -> Vec<(std::ops::Range<usize>, SortKey)> {
    let mut at = 0;
    let mut spots = Vec::new();
    for (span, key) in header_parts(table, width, sort) {
        let cells = span.width();
        if let Some(key) = key {
            spots.push((at..at + cells, key));
        }
        at += cells;
    }
    spots
}

/// The header's pieces, each title with the key it sorts by.
fn header_parts(
    table: &[(Column, usize)],
    width: usize,
    sort: SortMode,
) -> Vec<(Span<'static>, Option<SortKey>)> {
    let muted = Style::default().fg(theme::ink(theme::MUTED));
    let lit = Style::default().fg(theme::ink(theme::FOREGROUND));
    let style = |key: Option<SortKey>| {
        let ink = if key.is_some() && key == Some(sort.key) {
            lit
        } else {
            muted
        };
        ink.add_modifier(Modifier::BOLD)
    };
    // The same cells a row gives its name and its columns.
    let columns: usize = table.iter().map(|(_, cells)| cells + GAP).sum();
    let around = if table.is_empty() {
        1
    } else {
        columns - GAP + 3
    };
    if width < 3 + around {
        return Vec::new();
    }
    let name_width = width - 3 - around;
    let name = fit(&titled("Name", Some(SortKey::Name), sort), name_width);
    let name = format!("{name}{}", " ".repeat(name_width - name.width()));
    let mut parts = vec![
        (Span::raw("   "), None),
        (
            Span::styled(name, style(Some(SortKey::Name))),
            Some(SortKey::Name),
        ),
        (Span::raw(" "), None),
    ];
    for (at, &(column, _)) in table.iter().enumerate() {
        if at > 0 {
            parts.push((Span::raw(" ".repeat(GAP)), None));
        }
        let title = titled(column.title(), column.key(), sort);
        let text = set(&table[at..=at], std::iter::once(title));
        parts.push((Span::styled(text, style(column.key())), column.key()));
    }
    if !table.is_empty() {
        parts.push((Span::raw("  "), None));
    }
    parts
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
    let (sort, style, keep) = (app.sort, app.date_style, app.columns);
    let now = std::time::SystemTime::now();
    let me = crate::ops::user_id();
    let pane = &mut app.tab_mut().panes[index];
    // The header takes the top row once there is a row for items under it.
    let header = area.height >= 2;
    let (head, area) = match header {
        true => (
            Rect { height: 1, ..area },
            Rect {
                y: area.y + 1,
                height: area.height - 1,
                ..area
            },
        ),
        false => (Rect { height: 0, ..area }, area),
    };
    let rows = area.height as usize;
    let width = area.width as usize;
    pane.page_rows = rows.max(1);
    pane.offset = scroll(pane.offset, pane.cursor, rows);
    // The columns fit what shows, so they don't grow for what is out of sight.
    let end = (pane.offset + rows).min(pane.entries.len());
    let table = table(
        &pane.entries[pane.offset.min(end)..end],
        width,
        now,
        style,
        keep,
    );
    if header {
        // On the tab fill, so it reads as the header and not as an item.
        let fill = Style::default().bg(theme::ink(theme::TAB));
        let line = self::header(&table, width, sort);
        frame.render_widget(Paragraph::new(line).style(fill), head);
        for (cells, key) in sort_spots(&table, width, sort) {
            let x = head.x + cells.start as u16;
            hits.add(Rect::new(x, head.y, cells.len() as u16, 1), Spot::Sort(key));
        }
    }
    if pane.entries.is_empty() {
        empty(frame, area, pane, caption);
        return;
    }

    let query = pane.matching();
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
        let detail = paint(&table, entry, now, style, me);
        lines.push(painted_row(
            entry,
            width,
            Some(&detail),
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

/// What a pane with no items says, in the middle: a glyph over a
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

    /// Where a piece of text starts in one row, within a range of columns.
    fn x_in(drawn: &Buffer, y: u16, text: &str, columns: std::ops::Range<u16>) -> Option<u16> {
        let cells: Vec<&str> = text.split_inclusive(|_| true).collect();
        columns.clone().find(|x| {
            let end = x + cells.len() as u16;
            end <= columns.end
                && (cells.iter().zip(*x..end)).all(|(cell, x)| drawn[(x, y)].symbol() == *cell)
        })
    }

    #[test]
    fn the_list_shows_size_user_permissions_kind_and_modified_under_a_header() {
        use crate::app::testing::{fixture, press};
        use std::os::unix::fs::PermissionsExt;
        let mut f = fixture();
        let path = f.home.join("report.pdf");
        fs::write(&path, vec![0u8; 2048]).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        f.app.handle_key(key(KeyCode::F(5)));
        let user = model::owner(entry(&f.home, "report.pdf").uid);
        let drawn = buffer(&mut f.app, 200, 30);
        let files = panes(&f.app, 200).files;
        let (_, top) = find(&drawn, "Permissions", files.clone()).expect("the header");
        let title = |text: &str| x_in(&drawn, top, text, files.clone()).expect(text);
        let titles = ["Size", "User", "Permissions", "Kind", "Modified"].map(title);
        assert!(titles.is_sorted(), "{titles:?}");
        let (name, y) = find(&drawn, "report.pdf", files.clone()).expect("the row");
        assert!(y > top);
        let value = |text: &str| x_in(&drawn, y, text, files.clone()).expect(text);
        // Each value starts under its title; the size ends under its own.
        assert_eq!(title("Name ▴"), name);
        assert_eq!(value("2.0 KB") + 6, title("Size") + 4);
        assert_eq!(value(&user), title("User"));
        assert_eq!(value("-rw-r-----"), title("Permissions"));
        assert_eq!(value("PDF document"), title("Kind"));
        // The sorted column carries the arrow.
        press(&mut f.app, &[KeyCode::Char('s')]);
        let drawn = buffer(&mut f.app, 200, 30);
        assert!(x_in(&drawn, top, "Size ▴", files.clone()).is_some());
        assert!(x_in(&drawn, top, "Name ▴", files).is_none());
    }

    #[test]
    fn the_header_titles_are_bold_on_a_fill_of_their_own() {
        use crate::app::testing::fixture;
        let mut f = fixture();
        let drawn = buffer(&mut f.app, 200, 30);
        let files = panes(&f.app, 200).files;
        let (_, top) = find(&drawn, "Permissions", files.clone()).expect("the header");
        for title in ["Name ▴", "Size", "User", "Permissions", "Kind", "Modified"] {
            let x = x_in(&drawn, top, title, files.clone()).expect(title);
            for x in x..x + title.chars().count() as u16 {
                let modifier = drawn[(x, top)].modifier;
                assert!(modifier.contains(Modifier::BOLD), "{title} at {x}");
            }
        }
        // The whole row, gaps included, on the tab fill: not the cursor's.
        for x in files {
            assert_eq!(drawn[(x, top)].bg, theme::TAB, "{x}");
        }
        assert_ne!(theme::TAB, theme::CURSOR);
    }

    #[test]
    fn a_click_on_a_title_sorts_by_it_and_again_the_other_way() {
        use crate::app::testing::{fixture, launch, launch_one_pane};
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let f = fixture();
        let state = tempfile::tempdir().unwrap();
        let mut app = launch_one_pane(&f.home, Some(state.path()));
        let click = |app: &mut App, title: &str| {
            let drawn = buffer(app, 200, 30);
            let files = panes(app, 200).files;
            let (_, top) = find(&drawn, "Permissions", files.clone()).expect("the header");
            let (x, y) = (x_in(&drawn, top, title, files).expect(title), top);
            app.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            });
            (app.sort.key, app.sort.reversed)
        };
        assert_eq!(click(&mut app, "Size"), (SortKey::Size, false));
        assert_eq!(click(&mut app, "Size ▴"), (SortKey::Size, true));
        assert_eq!(click(&mut app, "Modified"), (SortKey::Modified, false));
        assert_eq!(click(&mut app, "Kind"), (SortKey::Kind, false));
        // What can't be sorted by leaves the order as it is.
        assert_eq!(click(&mut app, "User"), (SortKey::Kind, false));
        assert_eq!(click(&mut app, "Permissions"), (SortKey::Kind, false));
        assert_eq!(click(&mut app, "Name"), (SortKey::Name, false));
        // The items follow, and the order is remembered like `s` and `S`.
        let names: Vec<String> = (app.pane().entries.iter())
            .map(|entry| entry.name.clone())
            .collect();
        assert_eq!(names.first().map(String::as_str), Some("Documents"));
        assert_eq!(click(&mut app, "Name ▴"), (SortKey::Name, true));
        let again = launch(&f.home, Some(state.path()));
        assert_eq!((again.sort.key, again.sort.reversed), (SortKey::Name, true));
    }

    #[test]
    fn the_columns_are_coloured_like_eza() {
        use crate::app::testing::fixture;
        use std::os::unix::fs::PermissionsExt;
        let mut f = fixture();
        let path = f.home.join("report.pdf");
        fs::write(&path, vec![0u8; 2048]).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o754)).unwrap();
        let locked = f.home.join("locked.txt");
        fs::write(&locked, "x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        f.app.handle_key(key(KeyCode::F(5)));
        let report = entry(&f.home, "report.pdf");
        let user = model::owner(report.uid);
        let drawn = buffer(&mut f.app, 200, 30);
        let files = panes(&f.app, 200).files;
        let (_, top) = find(&drawn, "Permissions", files.clone()).expect("the header");
        let title = |text: &str| x_in(&drawn, top, text, files.clone()).expect(text);
        let ink = |x: u16, y: u16| drawn[(x, y)].fg;
        let (_, y) = find(&drawn, "report.pdf", files.clone()).expect("the row");
        let (red, yellow, green, blue) = (
            theme::TAGS[0],
            theme::TAGS[2],
            theme::TAGS[3],
            theme::TAGS[4],
        );
        // Permissions letter by letter: -rwxr-xr--
        let mode = title("Permissions");
        let expected = [
            theme::DIMMED,
            yellow,
            red,
            green,
            yellow,
            theme::DIMMED,
            green,
            yellow,
            theme::DIMMED,
            theme::DIMMED,
        ];
        let shown: Vec<_> = (0..10).map(|at| ink(mode + at, y)).collect();
        assert_eq!(shown, expected);
        // The size's number green, its unit muted.
        let size = x_in(&drawn, y, "2.0 KB", files.clone()).expect("the size");
        assert_eq!(ink(size, y), green);
        assert_eq!(ink(size + 4, y), theme::MUTED);
        // Its owner, who is the user running it, yellow.
        assert_eq!(x_in(&drawn, y, &user, files.clone()), Some(title("User")));
        assert_eq!(ink(title("User"), y), yellow);
        // The kind in its glyph's colour, the date blue.
        assert_eq!(ink(title("Kind"), y), icons::tint(&report));
        assert_eq!(ink(title("Modified"), y), blue);
        // A folder away from the cursor: `d` blue, its item count muted.
        let (_, folder) = find(&drawn, "Pictures", files.clone()).expect("a folder");
        assert_eq!(ink(mode, folder), blue);
        let (count, _) = (x_in(&drawn, folder, "items", files.clone()))
            .map(|x| (x, folder))
            .expect("its count");
        assert_eq!(ink(count, folder), theme::MUTED);
        // What can't be read stays muted throughout.
        let (_, locked) = find(&drawn, "locked.txt", files.clone()).expect("the locked row");
        for at in 0..10 {
            assert_eq!(ink(mode + at, locked), theme::MUTED, "{at}");
        }
        assert_eq!(ink(title("User"), locked), theme::MUTED);
    }

    #[test]
    fn the_list_shows_only_the_columns_the_settings_keep() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let entries = [entry(dir.path(), "notes.txt")];
        let now = std::time::SystemTime::now();
        let shown = |width, keep| -> Vec<Column> {
            let table = table(&entries, width, now, DateStyle::MonthFirst, keep);
            table.iter().map(|(column, _)| *column).collect()
        };
        let keep = [true, false, true, false, true];
        let all = shown(400, keep);
        assert_eq!(all, [Column::Size, Column::Permissions, Column::Modified]);
        assert_eq!(shown(400, [false; 5]), []);
        // Narrow, the last of those give way first, as ever.
        for width in 0..400 {
            let columns = shown(width, keep);
            assert_eq!(columns, all[..columns.len()], "{width}");
        }
        // In the list itself, the header and the rows leave them out.
        use crate::app::testing::fixture;
        let mut f = fixture();
        f.app.columns = keep;
        let drawn = buffer(&mut f.app, 200, 30);
        let files = panes(&f.app, 200).files;
        let (_, top) = find(&drawn, "Permissions", files.clone()).expect("the header");
        assert_eq!(x_in(&drawn, top, "User", files.clone()), None);
        assert_eq!(x_in(&drawn, top, "Kind", files.clone()), None);
        assert!(x_in(&drawn, top, "Modified", files).is_some());
    }

    #[test]
    fn a_narrow_list_drops_its_last_columns_first() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        let entries = [entry(dir.path(), "notes.txt")];
        let now = std::time::SystemTime::now();
        let shown = |width| -> Vec<Column> {
            let table = table(&entries, width, now, DateStyle::MonthFirst, [true; 5]);
            table.iter().map(|(column, _)| *column).collect()
        };
        assert_eq!(shown(20), []);
        assert_eq!(shown(400), COLUMNS);
        let mut last = 0;
        for width in 0..400 {
            let columns = shown(width);
            assert_eq!(columns, COLUMNS[..columns.len()], "{width}");
            assert!(columns.len() >= last, "{width}");
            last = columns.len();
        }
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
