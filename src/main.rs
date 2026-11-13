//! FileFinder: a keyboard-first terminal file manager for macOS, after Omarchy's folio.

mod app;
mod format;
mod icons;
mod model;
mod places;
mod state;
mod system;
mod theme;
mod ui;

use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;

use app::{App, Config};
use format::DateStyle;
use system::MacSystem;

fn main() -> io::Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let start = match std::env::args_os().nth(1) {
        Some(arg) => std::path::absolute(PathBuf::from(arg))?,
        None => std::env::current_dir().unwrap_or_else(|_| home.clone()),
    };
    icons::init_from_env();
    let config = Config {
        state_dir: Some(home.join(".config/filefinder-tui")),
        home,
        start,
        date_style: DateStyle::from_env(),
    };
    let mut app = App::new(config, Box::new(MacSystem));

    let mut terminal = ratatui::init();
    // Tells Ctrl+I from Tab and Ctrl+Shift+N from Ctrl+N, where the terminal can.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(stdout(), PushKeyboardEnhancementFlags(flags))?;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
            hook(info);
        }));
    }
    let result = run(&mut terminal, &mut app);
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            app.handle_key(key);
        }
    }
    Ok(())
}
