//! FileFinder: a keyboard-first terminal file manager for macOS.

mod app;
mod format;
mod icons;
mod input;
mod keys;
mod matcher;
mod model;
mod ops;
mod places;
mod search;
mod settings;
mod state;
mod syntax;
mod system;
mod tags;
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
    self, DisableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Picker, ProtocolType};

use app::{App, Config};
use format::DateStyle;
use syntax::{Bat, Highlighter};
use system::{MacSystem, TrackMouse};
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
    // Tells Ctrl+Shift+N from Ctrl+N, where the terminal can. Asked before
    // the picture question: an answer to that one that comes late (herdr's
    // does) would otherwise land in this one and make it say no.
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    let picker = query_picker();
    let thumbnailer = MacThumbnailer::new(&home.join("Library/Caches/filefinder-tui"));
    let config = Config {
        state_dir: Some(home.join(".config/filefinder-tui")),
        thumbs: Some(Thumbs::new(picker, Arc::new(thumbnailer))),
        highlighter: Some(Highlighter::new(Arc::new(Bat))),
        trash_dir: home.join(".Trash"),
        volumes_dir: PathBuf::from("/Volumes"),
        applications_dir: PathBuf::from("/Applications"),
        home: home.clone(),
        start,
        date_style: DateStyle::from_env(),
        depth: Depth::detect(),
    };
    if enhanced {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(stdout(), PushKeyboardEnhancementFlags(flags))?;
    }
    let mut app = App::new(config, Box::new(MacSystem::new(enhanced)));
    // The caret of a prompt is the terminal's own bar cursor.
    execute!(stdout(), SetCursorStyle::SteadyBar)?;
    // Clicks and the wheel come to the app; selecting text then takes the
    // terminal's own modifier (Shift-drag in Ghostty, Option-drag in Terminal).
    execute!(stdout(), TrackMouse { motion: false })?;
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
    let options = picture_question(std::env::var_os("HERDR_ENV").as_deref());
    let picker =
        Picker::from_query_stdio_with_options(options).unwrap_or_else(|_| Picker::halfblocks());
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
    // Whether every move of the mouse is reported: while a menu or a card
    // is open, and while the view buttons show, for their
    // tooltips.
    let mut motion = false;
    while !app.should_quit {
        app.tick();
        if app.take_redraw() {
            // Another program had the screen: nothing on it is ours, and
            // the mouse is as it was given back.
            terminal.clear()?;
            execute!(stdout(), TrackMouse { motion })?;
        }
        if app.wants_motion() != motion {
            motion = app.wants_motion();
            execute!(stdout(), TrackMouse { motion })?;
        }
        terminal.draw(|frame| ui::draw(frame, app))?;
        // Short enough for thumbnails to show up as they are made.
        if event::poll(Duration::from_millis(100))? {
            take_in(app, event::read()?);
            // A moving mouse sends many moves a frame: everything waiting is
            // taken in before the next frame is drawn.
            while event::poll(Duration::ZERO)? {
                take_in(app, event::read()?);
            }
        }
    }
    Ok(())
}

/// Hands a key or the mouse to the app.
fn take_in(app: &mut App, event: Event) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => app.handle_key(key),
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        _ => {}
    }
}

/// What the picture question asks. herdr (`HERDR_ENV` is set in its panes)
/// answers the kitty graphics part only after the status report that ends
/// the question: the picker has stopped reading by then, and the late answer
/// would come in as keys. Pictures there are half blocks either way.
fn picture_question(herdr: Option<&std::ffi::OsStr>) -> QueryStdioOptions {
    let mut options = QueryStdioOptions::default();
    if herdr.is_some_and(|value| !value.is_empty()) {
        options.blacklist_protocols = vec![ProtocolType::Kitty];
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_image::picker::cap_parser::Parser;
    use std::ffi::OsStr;

    /// Whether the picture question asks about kitty graphics.
    fn asks_kitty(herdr: Option<&OsStr>) -> bool {
        Parser::query(false, picture_question(herdr)).contains("_Gi=")
    }

    #[test]
    fn inside_herdr_the_picture_question_leaves_kitty_graphics_out() {
        assert!(!asks_kitty(Some(OsStr::new("1"))));
        // Anywhere else it is asked, as before.
        assert!(asks_kitty(None));
        assert!(asks_kitty(Some(OsStr::new(""))));
    }
}
