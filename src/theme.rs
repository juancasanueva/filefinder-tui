//! Folio's fallback palette: near-black panels with one yellow accent.

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Rgb(0xFF, 0xD6, 0x0A);
pub const BACKGROUND: Color = Color::Rgb(0x16, 0x16, 0x1a);
pub const DARK_BACKGROUND: Color = Color::Rgb(0x11, 0x11, 0x14);
pub const LIGHTER_BACKGROUND: Color = Color::Rgb(0x23, 0x23, 0x29);
pub const FOREGROUND: Color = Color::Rgb(0xc8, 0xc8, 0xd0);
pub const BRIGHT_FOREGROUND: Color = Color::Rgb(0xec, 0xec, 0xf1);
pub const RED: Color = Color::Rgb(0xf7, 0x76, 0x8e);

/// Secondary text: the foreground mixed halfway into the background.
pub const DIM: Color = Color::Rgb(0x7c, 0x7c, 0x83);
/// Separators and section labels.
pub const FAINT: Color = Color::Rgb(0x4a, 0x4a, 0x52);
/// Hairlines between panels.
pub const RULE: Color = Color::Rgb(0x2c, 0x2c, 0x33);
/// The accent mixed into the background, behind the selected item.
pub const SELECTION: Color = Color::Rgb(0x3e, 0x37, 0x17);
/// Text on top of the accent.
pub const ON_ACCENT: Color = Color::Rgb(0x11, 0x11, 0x14);

pub fn base() -> Style {
    Style::new().fg(FOREGROUND).bg(BACKGROUND)
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn faint() -> Style {
    Style::new().fg(FAINT)
}

pub fn bright() -> Style {
    Style::new().fg(BRIGHT_FOREGROUND)
}

pub fn title() -> Style {
    bright().add_modifier(Modifier::BOLD)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn rule() -> Style {
    Style::new().fg(RULE)
}

/// The selected item where the keyboard is.
pub fn selected() -> Style {
    Style::new().fg(ACCENT).bg(SELECTION)
}

/// The selected item in a pane the keyboard has left.
pub fn selected_inactive() -> Style {
    Style::new().fg(BRIGHT_FOREGROUND).bg(LIGHTER_BACKGROUND)
}

pub fn button() -> Style {
    Style::new()
        .fg(ON_ACCENT)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD)
}

pub fn error() -> Style {
    Style::new().fg(RED).bg(LIGHTER_BACKGROUND)
}

pub fn notice() -> Style {
    Style::new().fg(FOREGROUND).bg(LIGHTER_BACKGROUND)
}
