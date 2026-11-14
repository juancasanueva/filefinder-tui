//! The bottom row while it asks something: a prompt (Rename, Folder name,
//! Go to), the filter's or Find's field, or the paste question. Each takes
//! the footer's place.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::fit;
use crate::app::{App, Asking, Field, PASTE_KEYS, PasteQuestion, Prompt, RENAME_HINT};
use crate::input::Input;
use crate::theme;

/// Draws a prompt: one space, the muted label, a space, the field on its
/// fill, then why Enter was refused in the error colour, or Rename's keys
/// against the right edge. The terminal's cursor is the caret, while
/// nothing is selected.
pub fn draw(frame: &mut Frame, prompt: &Prompt, area: Rect) {
    let (after, style, right) = match (&prompt.error, &prompt.asking) {
        (Some(error), _) => (error.as_str(), theme::error(), false),
        (None, Asking::Rename(_)) => (RENAME_HINT, theme::muted(), true),
        _ => ("", theme::muted(), false),
    };
    let (label, input, ghost) = (prompt.label(), &prompt.input, prompt.ghost.as_str());
    let field = |room| field(input, ghost, room);
    row(frame, area, label, field, (after, style, right));
}

/// Draws the filter's or Find's field the way flea draws its prompts:
/// after the field, how many the filter shows, or where Find looks and
/// that Tab changes it, or why Ctrl+G was refused.
pub fn draw_field(frame: &mut Frame, app: &App, field: &Field, area: Rect) {
    let pane = app.pane();
    let (label, input, after, style) = match (field, pane.filter(), pane.search()) {
        (Field::Filter, Some(filter), _) => ("Filter", &filter.input, shown(app), theme::muted()),
        (Field::Search, _, Some(search)) => match &search.error {
            Some(error) => ("Find", &search.input, error.clone(), theme::error()),
            None => {
                let scope = app.scope_label(search);
                (
                    "Find",
                    &search.input,
                    format!("{scope} · tab changes scope"),
                    theme::muted(),
                )
            }
        },
        _ => return,
    };
    row(
        frame,
        area,
        label,
        |room| typed(input, room),
        (&after, style, false),
    );
}

/// What the filter shows, in flea's words: `N of M shown`, where it
/// looks when that is below the folder too, and how many are close matches.
pub fn shown(app: &App) -> String {
    let pane = app.pane();
    let Some(filter) = pane.filter() else {
        return String::new();
    };
    let mut said = format!("{} of {} shown", pane.entries.len(), filter.total());
    if filter.scope == crate::app::FilterScope::Subfolders {
        said.push_str(&format!(" in {} and subfolders", app.shown_path(&pane.cwd)));
    }
    match filter.close {
        0 => said,
        1 => format!("{said} · 1 close match"),
        close => format!("{said} · {close} close matches"),
    }
}

/// One row of the bottom row as a prompt: one space, the muted label, a
/// space, the field `field` draws in the room it is given, then `after`:
/// one space after the field, or against the right edge when `right`.
fn row(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    field: impl FnOnce(usize) -> (Vec<Span<'static>>, usize, Option<usize>),
    (after, style, right): (&str, Style, bool),
) {
    let columns = (area.width as usize).saturating_sub(2);
    let label = fit(label, columns);
    let mut used = label.width();
    let mut spans = vec![Span::raw(" "), Span::styled(label, theme::muted())];
    let mut caret = None;
    let room = columns.saturating_sub(used + 1);
    if room > 0 {
        spans.push(Span::raw(" "));
        used += 1;
        let (cells, width, at) = field(room);
        caret = at.map(|at| used + at);
        spans.extend(cells);
        used += width;
    }
    let left = columns.saturating_sub(used + 1);
    if !after.is_empty() && left > 0 && (!right || after.width() < left) {
        let width = after.width().min(left);
        let gap = if right { columns - used - width } else { 1 };
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(fit(after, width), style));
        used += gap + width;
    }
    spans.push(Span::raw(" ".repeat(columns.saturating_sub(used) + 1)));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme::base()), area);
    if let Some(at) = caret {
        frame.set_cursor_position(Position::new(area.x + 1 + at as u16, area.y));
    }
}

/// A field that is only typed into, never selected in or completed: the
/// text around the cursor and one cell for the caret, on the field's fill.
fn typed(input: &Input, columns: usize) -> (Vec<Span<'static>>, usize, Option<usize>) {
    let room = columns.saturating_sub(1);
    let (text, caret) = input.window(room);
    let width = text.width();
    let spans = vec![
        Span::styled(text, theme::field()),
        Span::styled(" ", theme::field()),
    ];
    (spans, width + 1, Some(caret as usize))
}

/// The field in at most `columns` cells: the text around the cursor, what
/// is selected on the selection's fill, then the ghost of a completion in
/// the extension ink and one cell for the caret, all on the field's fill.
/// Its spans, how many cells they take, and the cell the caret is on.
fn field(input: &Input, ghost: &str, columns: usize) -> (Vec<Span<'static>>, usize, Option<usize>) {
    let chars = input.chars();
    let selection = input.selection();
    let typed: usize = chars.iter().map(|c| c.width().unwrap_or(0)).sum();
    let at_end = input.cursor() == chars.len();
    let ghost = if selection.is_none() && at_end && typed + ghost.width() < columns {
        ghost
    } else {
        ""
    };
    let plain = theme::on(theme::FOREGROUND, theme::FIELD);
    let selected = if theme::depth().reverses() {
        plain.add_modifier(Modifier::REVERSED)
    } else {
        theme::on(theme::FOREGROUND, theme::FIELD_SELECTION)
    };
    let room = columns.saturating_sub(1);
    let (mut spans, mut used, mut caret) = (Vec::new(), 0, None);
    let mut run = (String::new(), plain);
    for (index, c) in chars.iter().enumerate().skip(input.first_shown(columns)) {
        if index == input.cursor() {
            caret = Some(used);
        }
        let cells = c.width().unwrap_or(0);
        if used + cells > room {
            break;
        }
        let style: Style = match selection {
            Some((start, end)) if index >= start && index < end => selected,
            _ => plain,
        };
        if style != run.1 && !run.0.is_empty() {
            let done = std::mem::take(&mut run.0);
            spans.push(Span::styled(done, run.1));
        }
        run.1 = style;
        run.0.push(*c);
        used += cells;
    }
    if !run.0.is_empty() {
        spans.push(Span::styled(run.0, run.1));
    }
    if at_end && used <= room {
        caret = Some(used);
    }
    spans.push(Span::styled(
        ghost.to_string(),
        theme::on(theme::EXTENSION, theme::FIELD),
    ));
    spans.push(Span::styled(" ", plain));
    used += ghost.width() + 1;
    (spans, used, caret.filter(|_| selection.is_none()))
}

/// Draws the paste question: one space, the question in the foreground,
/// two spaces, the answers it takes, muted; each cut to fit.
pub fn question(frame: &mut Frame, question: &PasteQuestion, area: Rect) {
    let columns = (area.width as usize).saturating_sub(2);
    let head = question.text();
    let head_width = head.width().min(columns);
    let keys_width = columns
        .saturating_sub(head_width + 2)
        .min(PASTE_KEYS.width());
    let gap = if keys_width > 0 { 2 } else { 0 };
    let used = head_width + gap + keys_width;
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(fit(&head, head_width), theme::base()),
        Span::raw(" ".repeat(gap)),
        Span::styled(fit(PASTE_KEYS, keys_width), theme::muted()),
        Span::raw(" ".repeat(columns.saturating_sub(used) + 1)),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::base()), area);
}
