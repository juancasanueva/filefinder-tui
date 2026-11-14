//! FileFinder: a keyboard-first terminal file manager for macOS, after flea.

mod app;
mod format;
mod icons;
mod input;
mod matcher;
mod model;
mod ops;
mod search;
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
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui_image::picker::Picker;

use app::{App, Config};
use format::DateStyle;
use system::MacSystem;
use theme::Depth;
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
        home: home.clone(),
        start,
        date_style: DateStyle::from_env(),
        depth: Depth::detect(),
    };
    // Tells Ctrl+Shift+N from Ctrl+N, where the terminal can.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(stdout(), PushKeyboardEnhancementFlags(flags))?;
    }
    let mut app = App::new(config, Box::new(MacSystem::new(enhanced)));
    // The caret of a prompt is the terminal's own bar cursor.
    execute!(stdout(), SetCursorStyle::SteadyBar)?;
    // Clicks and the wheel come to the app; selecting text then takes the
    // terminal's own modifier (Shift-drag in Ghostty, Option-drag in Terminal).
    execute!(stdout(), EnableMouseCapture)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        give_back(enhanced);
        hook(info);
    }));
    let result = run(&mut terminal, &mut app);
    give_back(enhanced);
    ratatui::restore();
    result
}

/// Hands the mouse back to the terminal, and the keyboard as it was.
fn give_back(enhanced: bool) {
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape
    );
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
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
        if app.take_redraw() {
            // Another program had the screen: nothing on it is ours.
            terminal.clear()?;
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Short enough for thumbnails to show up as they are made.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                _ => {}
            }
        }
    }
    Ok(())
}
