//! The top bar: where you are, and the view toggles.

use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, ViewMode};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let [row, rule] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let toggles = toggles(app);
    let toggles_width = toggles
        .iter()
        .map(|span| span.content.width())
        .sum::<usize>() as u16;
    let [path, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(toggles_width + 1)]).areas(row);

    let crumbs = breadcrumbs(
        &app.pane().cwd,
        &app.home,
        path.width.saturating_sub(4) as usize,
    );
    let mut spans = vec![Span::raw("  ")];
    let last = crumbs.len().saturating_sub(1);
    for (index, crumb) in crumbs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" / ", theme::faint()));
        }
        let style = if index == last {
            theme::title()
        } else {
            theme::dim()
        };
        spans.push(Span::styled(crumb.clone(), style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), path);
    frame.render_widget(
        Paragraph::new(Line::from(toggles)).alignment(Alignment::Left),
        right,
    );
    frame.render_widget(
        Paragraph::new("─".repeat(rule.width as usize)).style(theme::rule()),
        rule,
    );
}

/// View, sort, hidden files and details; the ones switched on are lit.
fn toggles(app: &App) -> Vec<Span<'static>> {
    let view = match app.view {
        ViewMode::Grid => "▦",
        ViewMode::List => "≡",
        ViewMode::Columns => "▥",
    };
    let lit = |on: bool| if on { theme::selected() } else { theme::base() };
    vec![
        Span::styled(format!(" {view} "), theme::base()),
        Span::styled(" ⇅ ", theme::base()),
        Span::styled(" ◉ ", lit(app.show_hidden)),
        Span::styled(" ◨ ", lit(app.details_visible)),
    ]
}

/// `~ / Pictures / Lisbon 2026`, dropping middle folders when the bar is narrow.
pub fn breadcrumbs(cwd: &Path, home: &Path, width: usize) -> Vec<String> {
    let (root, rest) = match cwd.strip_prefix(home) {
        Ok(rest) => ("~", rest),
        Err(_) => ("/", cwd.strip_prefix("/").unwrap_or(cwd)),
    };
    let mut crumbs = vec![root.to_string()];
    crumbs.extend(
        rest.components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );

    let total = |crumbs: &[String]| {
        crumbs.iter().map(|crumb| crumb.width()).sum::<usize>() + 3 * (crumbs.len() - 1)
    };
    while total(&crumbs) > width && crumbs.len() > 2 {
        if crumbs[1] == "…" {
            if crumbs.len() == 3 {
                break;
            }
            crumbs.remove(2);
        } else {
            crumbs[1] = "…".to_string();
        }
    }
    crumbs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breadcrumbs_start_at_home_or_the_root() {
        let home = Path::new("/Users/me");
        assert_eq!(breadcrumbs(home, home, 80), ["~"]);
        assert_eq!(
            breadcrumbs(Path::new("/Users/me/Pictures/Lisbon 2026"), home, 80),
            ["~", "Pictures", "Lisbon 2026"]
        );
        assert_eq!(
            breadcrumbs(Path::new("/Volumes/USB"), home, 80),
            ["/", "Volumes", "USB"]
        );
        assert_eq!(breadcrumbs(Path::new("/"), home, 80), ["/"]);
    }

    #[test]
    fn breadcrumbs_drop_the_middle_when_narrow() {
        let home = Path::new("/Users/me");
        let deep = Path::new("/Users/me/Pictures/Lisbon 2026/Sintra day trip");
        assert_eq!(breadcrumbs(deep, home, 30), ["~", "…", "Sintra day trip"]);
    }
}
