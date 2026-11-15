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
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
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
    } else {
        hear_late_answers(&mut picker, read_input);
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

/// How long a late answer to the picture question gets to come in, after
/// the last one.
const LATE: Duration = Duration::from_millis(150);

/// After the picture question, when it did not say kitty: herdr-app answers
/// the kitty graphics part only after the status report that ends the
/// question, once the picker has stopped reading. What comes in late is read
/// here, so it doesn't reach the app as keys; kitty saying OK turns kitty
/// graphics on. `read` waits up to its time for what comes in next.
fn hear_late_answers(picker: &mut Picker, mut read: impl FnMut(Duration) -> Option<Vec<u8>>) {
    if picker.protocol_type() != ProtocolType::Halfblocks {
        return;
    }
    let mut heard = Vec::new();
    // Bounded, should the terminal never stop talking.
    while heard.len() < 4096 {
        match read(LATE) {
            Some(bytes) => heard.extend(bytes),
            None => break,
        }
    }
    let ok = b"\x1b_Gi=31;OK";
    if heard.windows(ok.len()).any(|window| window == ok) {
        picker.set_protocol_type(ProtocolType::Kitty);
    }
}

/// What the terminal sends next on the standard input, waiting up to `wait`
/// for it; nothing when it sends nothing in that time.
fn read_input(wait: Duration) -> Option<Vec<u8>> {
    let mut ready = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = libc::c_int::try_from(wait.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: one valid pollfd, and the count says one.
    if unsafe { libc::poll(&mut ready, 1, millis) } <= 0 {
        return None;
    }
    let mut buffer = [0u8; 512];
    // SAFETY: reads at most the buffer's length into it.
    let read = unsafe { libc::read(libc::STDIN_FILENO, buffer.as_mut_ptr().cast(), buffer.len()) };
    let read = usize::try_from(read).ok().filter(|read| *read > 0)?;
    Some(buffer[..read].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A terminal that sends `chunks` late, one per read, then nothing.
    fn late(chunks: &[&[u8]]) -> impl FnMut(Duration) -> Option<Vec<u8>> {
        let mut chunks: VecDeque<Vec<u8>> = chunks.iter().map(|c| c.to_vec()).collect();
        move |_| chunks.pop_front()
    }

    #[test]
    fn a_late_kitty_ok_turns_kitty_graphics_on_and_everything_late_is_read() {
        let mut picker = Picker::halfblocks();
        let mut chunks = late(&[b"\x1b_Gi=31;OK", b"\x1b\\", b"\x1b[?62;22c"]);
        let mut reads = 0;
        hear_late_answers(&mut picker, |wait| {
            reads += 1;
            chunks(wait)
        });
        assert_eq!(picker.protocol_type(), ProtocolType::Kitty);
        // Every chunk was read, and the read that found nothing ended it.
        assert_eq!(reads, 4);
    }

    #[test]
    fn without_a_late_kitty_ok_the_picker_keeps_what_it_had() {
        for chunks in [&[][..], &[&b"\x1b_Gi=31;ENOTSUPPORTED\x1b\\"[..]][..]] {
            let mut picker = Picker::halfblocks();
            hear_late_answers(&mut picker, late(chunks));
            assert_eq!(picker.protocol_type(), ProtocolType::Halfblocks);
        }
        // A picker that already draws better isn't kept waiting.
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        hear_late_answers(&mut picker, |_| panic!("read while kitty was known"));
        assert_eq!(picker.protocol_type(), ProtocolType::Kitty);
    }
}
