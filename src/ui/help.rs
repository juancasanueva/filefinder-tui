//! The `?` overlay: every shortcut that works.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use super::pad;
use crate::theme;

const SHORTCUTS: &[(&str, &str)] = &[
    ("← ↑ → ↓", "Move the selection"),
    ("→ / ←", "In list and columns: into a folder / back out"),
    (
        "Tab / Shift+Tab",
        "Move between the sidebar, the panes and the details",
    ),
    ("⏎", "Open"),
    ("Space", "Preview the selected file"),
    ("Backspace", "Go to the parent folder"),
    ("Alt+← / Alt+→", "Back / forward"),
    ("Alt+Home", "Go home"),
    ("Ctrl+1 / 2 / 3", "Switch the view: grid, list, columns"),
    (
        "Ctrl+= / Ctrl+- / Ctrl+0",
        "Bigger, smaller or normal tiles",
    ),
    ("Ctrl+I", "Show or hide the details pane"),
    ("S / Shift+S", "Sort: name, kind, newest, oldest, size"),
    ("Ctrl+H", "Show hidden files"),
    ("F5", "Reload"),
    ("?", "Show these shortcuts"),
    ("Q", "Close the window"),
];

const KEY_WIDTH: usize = 26;

pub fn draw(frame: &mut Frame, area: Rect) {
    let width = 86.min(area.width.saturating_sub(4));
    let height = (SHORTCUTS.len() as u16 + 6).min(area.height.saturating_sub(2));
    let overlay = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme::rule())
        .padding(Padding::new(3, 3, 1, 1))
        .style(theme::base().bg(theme::DARK_BACKGROUND));

    let mut lines = vec![
        Line::from(Span::styled("Keyboard shortcuts", theme::title())),
        Line::default(),
    ];
    for (keys, does) in SHORTCUTS {
        lines.push(Line::from(vec![
            Span::styled(pad(keys, KEY_WIDTH), theme::accent()),
            Span::styled(*does, theme::base().bg(theme::DARK_BACKGROUND)),
        ]));
    }
    frame.render_widget(Clear, overlay);
    frame.render_widget(Paragraph::new(lines).block(block), overlay);
}
