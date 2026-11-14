//! The footer: how many items there are or are selected, what was last said,
//! and on the right the volume and where the keys are; each run on the right
//! steps aside when it doesn't fit.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::fit;
use crate::app::App;
use crate::format;
use crate::theme;

/// A run of text with the cells before it.
struct Run {
    text: String,
    style: Style,
    gap: usize,
}

fn run(text: String, style: Style, gap: usize) -> Run {
    Run { text, style, gap }
}

/// Draws the footer row: one space, the runs on the left, the runs on the
/// right, one space.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let columns = (area.width as usize).saturating_sub(2);
    let left = left_runs(app);
    let left_width: usize = left.iter().map(|run| run.text.width() + run.gap).sum();

    let disk = app
        .volume()
        .map(|volume| format!("{} · {} free", volume.kind, format::size(volume.free)))
        .unwrap_or_default();
    let mut right: Vec<Span> = Vec::new();
    let mut right_width = 0;
    for (text, style) in [
        (disk, theme::base()),
        ("? keys".to_string(), theme::muted()),
    ] {
        let gap = if right_width == 0 { 0 } else { 3 };
        if text.is_empty()
            || text.width() + right_width + gap + left_width.min(columns) + 2 > columns
        {
            continue;
        }
        if gap > 0 {
            right.push(Span::styled(" · ", theme::muted()));
        }
        right_width += text.width() + gap;
        right.push(Span::styled(text, style));
    }

    let available = columns.saturating_sub(right_width + usize::from(right_width > 0) * 2);
    let mut spans = vec![Span::raw(" ")];
    let mut used = 0;
    for run in left {
        let gap = if used == 0 {
            0
        } else {
            run.gap.min(available.saturating_sub(used))
        };
        spans.push(Span::raw(" ".repeat(gap)));
        used += gap;
        let text = fit(&run.text, available.saturating_sub(used));
        used += text.width();
        spans.push(Span::styled(text, run.style));
    }
    spans.push(Span::raw(
        " ".repeat(columns.saturating_sub(used + right_width)),
    ));
    spans.extend(right);
    spans.push(Span::raw(" "));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme::base()), area);
}

/// How many items, or how many are selected and their size; then the file
/// work under way or what was last said.
fn left_runs(app: &App) -> Vec<Run> {
    let pane = app.pane();
    let total = pane.entries.len();
    let marked: Vec<_> = (0..total).filter(|index| pane.is_marked(*index)).collect();
    let count = if marked.is_empty() {
        format::items(total)
    } else {
        let bytes: u64 = marked
            .iter()
            .map(|index| &pane.entries[*index])
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.size)
            .sum();
        format!(
            "{} of {total} selected · {}",
            marked.len(),
            format::size(bytes)
        )
    };
    let mut runs = vec![run(count, theme::base(), 0)];
    if let Some(label) = app.working() {
        runs.push(run(label.to_string(), theme::base(), 2));
    } else if let Some(message) = &app.message {
        let style = if message.is_error {
            theme::error()
        } else {
            theme::base()
        };
        runs.push(run(message.text.clone(), style, 2));
        if message.undoable {
            runs.push(run("· z undoes".to_string(), theme::muted(), 1));
        }
    }
    runs
}
