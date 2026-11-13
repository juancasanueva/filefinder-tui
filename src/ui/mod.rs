//! Drawing: one function per region of the window.

mod details;
mod help;
mod list;
mod sidebar;
mod topbar;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus};
use crate::theme;

const SIDEBAR_WIDTH: u16 = 24;
const DETAILS_WIDTH: u16 = 34;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::new().style(theme::base()), area);

    let [top, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    topbar::draw(frame, app, top);

    // Narrow terminals give the room to the files: details go first, then the sidebar.
    let details_width = if app.details_visible && area.width >= 90 {
        DETAILS_WIDTH
    } else {
        0
    };
    let sidebar_width = if area.width >= 56 { SIDEBAR_WIDTH } else { 0 };
    let [side, files, details] = Layout::horizontal([
        Constraint::Length(sidebar_width),
        Constraint::Min(0),
        Constraint::Length(details_width),
    ])
    .areas(body);

    if side.width > 0 {
        let block = Block::new()
            .borders(Borders::RIGHT)
            .border_style(theme::rule());
        let inner = block.inner(side);
        frame.render_widget(block, side);
        sidebar::draw(frame, app, inner);
    }
    if details.width > 0 {
        let block = Block::new()
            .borders(Borders::LEFT)
            .border_style(theme::rule());
        let inner = block.inner(details);
        frame.render_widget(block, details);
        details::draw(frame, app, inner);
    }
    draw_files(frame, app, files);

    if app.help_visible {
        help::draw(frame, area);
    }
}

fn draw_files(frame: &mut Frame, app: &mut App, area: Rect) {
    let band = u16::from(app.message.is_some());
    let [view, message] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(band)]).areas(area);
    let focused = app.focus == Focus::Files;
    app.page_rows = view.height.saturating_sub(1).max(1) as usize;
    list::draw(frame, app, view, focused);

    if let Some(note) = &app.message {
        let style = if note.is_error {
            theme::error()
        } else {
            theme::notice()
        };
        let text = fit(&format!("  {}", note.text), message.width as usize);
        frame.render_widget(Paragraph::new(Line::from(text)).style(style), message);
    }
}

/// Shortens text to a width in cells, ending in an ellipsis when it had to cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    if width > 0 {
        out.push('…');
    }
    out
}

/// Fits text and pads it with spaces to exactly `width` cells.
pub fn pad(text: &str, width: usize) -> String {
    let fitted = fit(text, width);
    let gap = width.saturating_sub(fitted.width());
    format!("{fitted}{}", " ".repeat(gap))
}

/// Like [`pad`], with the text against the right edge.
pub fn pad_left(text: &str, width: usize) -> String {
    let fitted = fit(text, width);
    let gap = width.saturating_sub(fitted.width());
    format!("{}{fitted}", " ".repeat(gap))
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Draws the app at a size and returns the screen as lines of text.
    pub fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::screen;
    use super::*;
    use crate::app::testing::{fixture, press};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn fit_cuts_with_an_ellipsis_and_pad_fills_the_width() {
        assert_eq!(fit("harbour-dusk.jpg", 20), "harbour-dusk.jpg");
        assert_eq!(fit("harbour-dusk.jpg", 8), "harbour…");
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad_left("ab", 4), "  ab");
        assert_eq!(pad("abcdef", 4), "abc…");
    }

    #[test]
    fn the_window_shows_breadcrumbs_sidebar_files_and_details() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Down, KeyCode::Enter, KeyCode::Down]);
        let lines = screen(&mut f.app, 120, 24);
        let all = lines.join("\n");
        assert!(lines[0].contains("~ / Pictures"), "{}", lines[0]);
        assert!(lines[1].starts_with("────"), "{}", lines[1]);
        assert!(all.contains("HOME"));
        assert!(all.contains("Documents"));
        assert!(all.contains("Trip"));
        // The selected file: in the list and named in the details pane.
        assert!(all.matches("a.jpg").count() >= 2, "{all}");
        assert!(all.contains("JPEG image · 3 B"), "{all}");
        assert!(all.contains("Modified"));
        assert!(all.contains("Open"));
    }

    #[test]
    fn narrow_windows_drop_the_details_then_the_sidebar() {
        let mut f = fixture();
        let medium = screen(&mut f.app, 70, 20).join("\n");
        assert!(medium.contains("HOME") && !medium.contains("Modified"));
        let narrow = screen(&mut f.app, 40, 20).join("\n");
        assert!(!narrow.contains("HOME") && narrow.contains("notes.txt"));
    }

    #[test]
    fn an_empty_folder_says_so() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Enter]);
        let all = screen(&mut f.app, 100, 20).join("\n");
        assert!(all.contains("No items"), "{all}");
        assert!(all.contains("Nothing selected"), "{all}");
    }

    #[test]
    fn the_help_overlay_lists_the_shortcuts() {
        let mut f = fixture();
        press(&mut f.app, &[KeyCode::Char('?')]);
        let all = screen(&mut f.app, 120, 40).join("\n");
        assert!(all.contains("Keyboard shortcuts"));
        assert!(all.contains("Go to the parent folder"));
    }
}
