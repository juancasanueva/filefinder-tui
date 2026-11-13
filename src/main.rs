//! FileFinder: a keyboard-first terminal file manager for macOS, after Omarchy's folio.

mod app;
mod format;
mod icons;
mod input;
mod model;
mod ops;
mod places;
mod state;
mod system;
mod theme;
mod thumbs;
mod ui;

use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui_image::picker::Picker;

use app::{App, Config};
use format::DateStyle;
use system::MacSystem;
use thumbs::{MacThumbnailer, Thumbs};

fn main() -> io::Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let start = match std::env::args_os().nth(1) {
        Some(arg) => std::path::absolute(PathBuf::from(arg))?,
        None => std::env::current_dir().unwrap_or_else(|_| home.clone()),
    };
    icons::init_from_env();

    let mut terminal = ratatui::init();
    let picker = query_picker();
    let thumbnailer = MacThumbnailer::new(&home.join("Library/Caches/filefinder-tui"));
    let config = Config {
        state_dir: Some(home.join(".config/filefinder-tui")),
        thumbs: Some(Thumbs::new(picker, Arc::new(thumbnailer))),
        trash_dir: home.join(".Trash"),
        volumes_dir: PathBuf::from("/Volumes"),
        home: home.clone(),
        start,
        date_style: DateStyle::from_env(),
    };
    let mut app = App::new(config, Box::new(MacSystem::new(&home)));
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

/// Asks the terminal how it shows pictures. It has to be asked before any key
/// is read; one that doesn't say gets them as colored half blocks.
fn query_picker() -> Picker {
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    if picker.capabilities().is_empty() {
        // The terminal said nothing useful. When it said nothing at all (tmux
        // with nobody attached passes the question on to no one) the picker
        // gave up and left its reader waiting on the keyboard, where it would
        // take the next thing typed. A status report, which tmux and every
        // terminal answer themselves, is what that reader is waiting for; it
        // holds the lock on the standard input until it has read it.
        let mut out = stdout();
        if out.write_all(b"\x1b[5n").and_then(|()| out.flush()).is_ok() {
            drop(io::stdin().lock());
        }
    }
    picker
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        app.tick();
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Short enough for thumbnails to show up as they are made.
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            app.handle_key(key);
        }
    }
    Ok(())
}
