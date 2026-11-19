//! The port to everything outside the folder tree: launching apps, previews,
//! terminals, the editor, the program that zips, and the volume a folder is on.

use std::cell::RefCell;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::ops::Format;

/// What writes one archive. It is handed to the thread that does file
/// work, and run there with the new file the archive goes in and the flag
/// that stops it: once that is set it gives up, with an error.
pub type Archiver = Box<dyn FnOnce(&Path, &AtomicBool) -> io::Result<()> + Send>;

/// What lists the names inside an archive, on the thread that does file
/// work, until the flag stops it.
pub type Lister = Box<dyn FnOnce(&AtomicBool) -> io::Result<Vec<String>> + Send>;

/// What writes what an archive holds into the folder it is given, on the
/// thread that does file work, until the flag stops it.
pub type Unpacker = Box<dyn FnOnce(&Path, &AtomicBool) -> io::Result<()> + Send>;

/// What finds the files whose contents hold some text. It is handed to the
/// thread that searches, and run there with what takes each file it finds
/// (and says whether to go on) and the flag that stops it.
pub type Grep =
    Box<dyn FnOnce(&mut dyn FnMut(PathBuf) -> bool, &AtomicBool) -> io::Result<()> + Send>;

pub trait System {
    /// Opens a file with its default app.
    fn open(&self, path: &Path) -> io::Result<()>;

    /// Shows a file in a preview window of its own, without waiting for it.
    /// Asked again while that window is still open, closes it instead.
    fn preview(&self, path: &Path) -> io::Result<()>;

    /// Puts text on the system clipboard, for other apps to paste.
    fn copy_text(&self, text: &str) -> io::Result<()>;

    /// Opens a terminal window in a folder.
    fn open_terminal(&self, folder: &Path) -> io::Result<()>;

    /// What writes these items, which are all in one folder, into one
    /// archive of a format.
    fn archiver(&self, items: &[PathBuf], format: Format) -> Archiver;

    /// The formats Compress offers: the written ones, and 7z when 7-Zip is installed.
    fn formats(&self) -> Vec<Format>;

    /// What lists and unpacks an archive of a format; `None` when nothing
    /// installed reads it.
    fn extractor(&self, archive: &Path, format: Format) -> Option<(Lister, Unpacker)>;

    /// The volume a folder is on; `None` when that can't be told.
    fn volume(&self, folder: &Path) -> Option<Volume>;

    /// Ejects the mounted volume at `volume`, as Finder does; fails while
    /// something is using it.
    fn eject(&self, volume: &Path) -> io::Result<()>;

    /// Whether the mounted volume at `volume` is on the Mac's own disk,
    /// like Recovery, so it can't be ejected; false when that can't be told.
    fn internal(&self, volume: &Path) -> bool;

    /// Edits a file in `$EDITOR` (`vi` without one) in this terminal, and
    /// waits for it: the window is handed over meanwhile and taken back after.
    fn edit(&self, path: &Path) -> io::Result<()>;

    /// What finds the files below `roots` whose contents hold `text`,
    /// whatever its case; `None` when ripgrep isn't installed.
    fn grep(&self, roots: &[PathBuf], text: &str, show_hidden: bool) -> Option<Grep>;

    /// Whether ripgrep is installed, so that `grep` gives something.
    fn can_grep(&self) -> bool;

    /// What Spotlight finds carrying every one of `tags`, below `root`, or
    /// anywhere on the Mac without one; `None` without Spotlight, or for a
    /// tag it can't be asked about.
    fn spotlight(&self, tags: &[String], root: Option<&Path>) -> Option<Grep>;

    /// The colour tags Spotlight knows some file carries, Red's bit first;
    /// `None` without Spotlight.
    fn tags_in_use(&self) -> Option<u8>;
}

/// A volume as the footer names it: its file-system type and the space left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Like `apfs`.
    pub kind: String,
    /// Bytes free for this user.
    pub free: u64,
}

#[derive(Default)]
pub struct MacSystem {
    preview: Preview,
    /// Whether the keyboard was set up to tell more keys apart, which the
    /// editor has to get back as it was.
    enhanced: bool,
}

impl MacSystem {
    pub fn new(enhanced: bool) -> Self {
        Self {
            enhanced,
            ..Self::default()
        }
    }
}

impl System for MacSystem {
    fn open(&self, path: &Path) -> io::Result<()> {
        let status = quiet(Command::new("open").arg(path)).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("no app is set up to open it"))
        }
    }

    fn preview(&self, path: &Path) -> io::Result<()> {
        // Quick Look, in a panel that lives as long as this process does.
        self.preview
            .toggle(|| quiet(Command::new("qlmanage").arg("-p").arg(path)).spawn())
    }

    fn copy_text(&self, text: &str) -> io::Result<()> {
        pipe_into(&mut Command::new("pbcopy"), text)
    }

    fn open_terminal(&self, folder: &Path) -> io::Result<()> {
        let program = std::env::var("TERM_PROGRAM").ok();
        let app = terminal_app(program.as_deref());
        let status = quiet(&mut terminal_command(app, folder)).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("no terminal started"))
        }
    }

    fn archiver(&self, items: &[PathBuf], format: Format) -> Archiver {
        let items = items.to_vec();
        Box::new(move |archive, stop| {
            let mut command = match format {
                Format::Zip => zip_command(&items, archive)?,
                Format::SevenZ => seven_zip_command(&items, archive)?,
                _ => tar_command(&items, archive)?,
            };
            run_until(&mut command, stop)
        })
    }

    fn formats(&self) -> Vec<Format> {
        let mut formats = Format::WRITTEN.to_vec();
        if seven_zip().is_some() {
            formats.push(Format::SevenZ);
        }
        formats
    }

    fn extractor(&self, archive: &Path, format: Format) -> Option<(Lister, Unpacker)> {
        let archive = archive.to_path_buf();
        if format == Format::SevenZ {
            let seven = seven_zip()?;
            let mut list = Command::new(&seven);
            list.args(["l", "-slt", "-ba", "--"]).arg(&archive);
            let lister: Lister = Box::new(move |stop| {
                let listed = output_of(&mut list, stop)?;
                let paths = listed
                    .lines()
                    .filter_map(|line| line.strip_prefix("Path = "));
                Ok(paths.map(str::to_string).collect())
            });
            let unpacker: Unpacker = Box::new(move |into, stop| {
                let mut unpack = Command::new(&seven);
                let out = format!("-o{}", into.display());
                unpack.args(["x", "-bd", "-y", &out, "--"]).arg(&archive);
                run_until(&mut unpack, stop)
            });
            return Some((lister, unpacker));
        }
        // bsdtar lists every kind; a zip is unpacked by `ditto`, which
        // keeps what macOS knows about each file and never asks for a
        // password.
        let mut list = Command::new("/usr/bin/tar");
        list.args(["-t", "-f"]).arg(&archive);
        let lister: Lister = Box::new(move |stop| {
            let listed = output_of(&mut list, stop)?;
            Ok(listed.lines().map(str::to_string).collect())
        });
        let unpacker: Unpacker = Box::new(move |into, stop| {
            let mut unpack = if format == Format::Zip {
                let mut ditto = Command::new("/usr/bin/ditto");
                ditto.args(["-x", "-k"]).arg(&archive).arg(into);
                ditto
            } else {
                let mut tar = Command::new("/usr/bin/tar");
                tar.args(["-x", "-f"]).arg(&archive).arg("-C").arg(into);
                tar
            };
            run_until(&mut unpack, stop)
        });
        Some((lister, unpacker))
    }

    fn eject(&self, volume: &Path) -> io::Result<()> {
        // The whole disk, like Finder; what has no disk of its own, like a
        // network share, is unmounted instead.
        for verb in ["eject", "unmount"] {
            let mut command = Command::new("diskutil");
            if quiet(command.arg(verb).arg(volume)).status()?.success() {
                return Ok(());
            }
        }
        Err(io::Error::other("it is in use"))
    }

    fn internal(&self, volume: &Path) -> bool {
        let mut command = Command::new("diskutil");
        let output = command
            .args(["info", "-plist"])
            .arg(volume)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        output.is_ok_and(|output| {
            output.status.success() && says_internal(&String::from_utf8_lossy(&output.stdout))
        })
    }

    fn volume(&self, folder: &Path) -> Option<Volume> {
        volume_of(folder)
    }

    fn edit(&self, path: &Path) -> io::Result<()> {
        let editor = editor_of(std::env::var("EDITOR").ok());
        let mut command = Command::new("/bin/sh");
        // The editor is written the way a shell reads it, like `code -w`;
        // the file goes after it as it is, whatever its name holds.
        command
            .arg("-c")
            .arg(format!("{editor} \"$1\""))
            .arg("sh")
            .arg(path);
        hand_over(self.enhanced);
        let status = command.status();
        let taken_back = take_back(self.enhanced);
        let status = status?;
        taken_back?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("the editor failed"))
        }
    }

    fn grep(&self, roots: &[PathBuf], text: &str, show_hidden: bool) -> Option<Grep> {
        let rg = ripgrep()?;
        let mut command = grep_command(&rg, roots, text, show_hidden);
        Some(Box::new(move |found, stop| {
            stream_lines(&mut command, found, stop)
        }))
    }

    fn can_grep(&self) -> bool {
        ripgrep().is_some()
    }

    fn spotlight(&self, tags: &[String], root: Option<&Path>) -> Option<Grep> {
        let query = spotlight_query(tags)?;
        if !Path::new(MDFIND).is_file() {
            return None;
        }
        let mut command = Command::new(MDFIND);
        if let Some(root) = root {
            command.arg("-onlyin").arg(root);
        }
        command.arg(query);
        Some(Box::new(move |found, stop| {
            stream_lines(&mut command, found, stop)
        }))
    }

    fn tags_in_use(&self) -> Option<u8> {
        if !Path::new(MDFIND).is_file() {
            return None;
        }
        // The seven asked at once: about a tenth of a second in all.
        let asked: Vec<Option<Child>> = crate::tags::NAMES
            .iter()
            .map(|name| {
                spotlight_query(&[name.to_string()]).and_then(|query| spotlight_count(&query))
            })
            .collect();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut mask = 0;
        for (index, child) in asked.into_iter().enumerate() {
            let Some(mut child) = child else { continue };
            let done = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status.success(),
                    Ok(None) if std::time::Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break false;
                    }
                }
            };
            let mut said = String::new();
            if done && let Some(mut output) = child.stdout.take() {
                let _ = io::Read::read_to_string(&mut output, &mut said);
            }
            if said.trim().parse::<usize>().is_ok_and(|count| count > 0) {
                mask |= 1 << index;
            }
        }
        Some(mask)
    }
}

/// Where ripgrep is: on the path, or in Homebrew's folders for a terminal
/// started without them on its path.
fn ripgrep() -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    program_on(
        "rg",
        Some(&format!("{path}:/opt/homebrew/bin:/usr/local/bin")),
    )
}

/// Where a program is on a `PATH`-like list of folders, when it is.
fn program_on(name: &str, path: Option<&str>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path?
        .split(':')
        .filter(|folder| !folder.is_empty())
        .map(|folder| Path::new(folder).join(name))
        .find(|program| {
            fs::metadata(program)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// ripgrep asked for the files below `roots` holding `text` as it is,
/// whatever its case, each root's search staying on its volume, by path.
fn grep_command(rg: &Path, roots: &[PathBuf], text: &str, show_hidden: bool) -> Command {
    let mut command = Command::new(rg);
    command.args([
        "--files-with-matches",
        "--fixed-strings",
        "--ignore-case",
        "--no-messages",
        "--one-file-system",
        "--sort",
        "path",
    ]);
    if show_hidden {
        command.arg("--hidden");
    }
    // After `-e` the text is the text, whatever it starts with; after `--`
    // a folder is a folder.
    command.arg("-e").arg(text).arg("--").args(roots);
    command
}

/// Runs a command and hands each line it writes over as a path, until it
/// ends, `found` says enough or `stop` is set: then it is killed.
fn stream_lines(
    command: &mut Command,
    found: &mut dyn FnMut(PathBuf) -> bool,
    stop: &AtomicBool,
) -> io::Result<()> {
    use std::io::BufRead;
    use std::os::unix::ffi::OsStringExt;
    let mut child = quiet(command).stdout(Stdio::piped()).spawn()?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no output"))?;
    let (sender, lines) = std::sync::mpsc::channel();
    // Reading waits for the command; the lines come over to be looked at
    // between waits for whether to stop. The pipe's end ends the reader.
    thread::spawn(move || {
        for line in io::BufReader::new(output)
            .split(b'\n')
            .map_while(Result::ok)
        {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    loop {
        match lines.recv_timeout(Duration::from_millis(20)) {
            Ok(line) => {
                let path = PathBuf::from(std::ffi::OsString::from_vec(line));
                if !found(path) {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if stop.load(Ordering::Relaxed) => {
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// The editor to run: `$EDITOR`, or `vi` when it says nothing.
fn editor_of(editor: Option<String>) -> String {
    editor
        .filter(|editor| !editor.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// The mouse as the app asks for it: presses, releases and drags
/// (button-event tracking, 1002) in SGR form (1006), and every move as well
/// (any-motion tracking, 1003) only while `motion` is asked for, for a menu
/// or a card whose lines follow the pointer, or the view buttons' tooltips. crossterm's
/// `DisableMouseCapture` turns all of them off again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrackMouse {
    pub motion: bool,
}

impl ratatui::crossterm::Command for TrackMouse {
    fn write_ansi(&self, out: &mut impl std::fmt::Write) -> std::fmt::Result {
        // Terminals keep one tracking mode: turning 1003 off after 1002 is
        // on would turn the mouse off altogether, so it goes first.
        out.write_str(if self.motion {
            "\x1b[?1002h\x1b[?1006h\x1b[?1003h"
        } else {
            "\x1b[?1003l\x1b[?1002h\x1b[?1006h"
        })
    }
}

/// Gives the terminal to another program: the screen and keyboard as they
/// were before this app, the mouse its own.
fn hand_over(enhanced: bool) {
    use ratatui::crossterm::event::{DisableMouseCapture, PopKeyboardEnhancementFlags};
    use ratatui::crossterm::{cursor, execute, terminal};
    let mut out = io::stdout();
    let _ = execute!(
        out,
        DisableMouseCapture,
        cursor::SetCursorStyle::DefaultUserShape
    );
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
}

/// Takes the terminal back for this app, the way `main` first set it up.
fn take_back(enhanced: bool) -> io::Result<()> {
    use ratatui::crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    use ratatui::crossterm::{cursor, execute, terminal};
    let mut out = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(
        out,
        terminal::EnterAlternateScreen,
        TrackMouse { motion: false },
        cursor::SetCursorStyle::SteadyBar
    )?;
    if enhanced {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        execute!(out, PushKeyboardEnhancementFlags(flags))?;
    }
    Ok(())
}

/// Asks the system about the volume a folder is on.
#[cfg(target_os = "macos")]
fn volume_of(folder: &Path) -> Option<Volume> {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(folder.as_os_str().as_bytes()).ok()?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `path` ends in a NUL and `stats` is room for one `statfs`,
    // which the call fills in whenever it returns 0.
    let stats = unsafe {
        if libc::statfs(path.as_ptr(), stats.as_mut_ptr()) != 0 {
            return None;
        }
        stats.assume_init()
    };
    // SAFETY: the type's name is NUL-terminated within its array.
    let kind = unsafe { CStr::from_ptr(stats.f_fstypename.as_ptr()) };
    Some(Volume {
        kind: kind.to_string_lossy().into_owned(),
        free: stats.f_bavail.saturating_mul(u64::from(stats.f_bsize)),
    })
}

#[cfg(not(target_os = "macos"))]
fn volume_of(_folder: &Path) -> Option<Volume> {
    None
}

/// The app that opens a terminal window: the one this runs in, going by what
/// it says in `TERM_PROGRAM`, and the system's own when it is another.
fn terminal_app(term_program: Option<&str>) -> &'static str {
    match term_program {
        Some("ghostty") => "Ghostty",
        Some("iTerm.app") => "iTerm",
        Some("WezTerm") => "WezTerm",
        Some("kitty") => "kitty",
        _ => "Terminal",
    }
}

fn terminal_command(app: &str, folder: &Path) -> Command {
    let mut open = Command::new("open");
    open.arg("-a").arg(app).arg(folder);
    open
}

/// The command that zips items of one folder into `archive`, a file that
/// isn't there yet. One file or folder goes through `ditto`, which keeps
/// what macOS knows about it. Several can only go in by name, from their
/// folder, through `zip`; so does a link, which `ditto` would follow to what
/// it points at, and `zip` is told to keep as a link.
fn zip_command(items: &[PathBuf], archive: &Path) -> io::Result<Command> {
    let whole = |item: &PathBuf| {
        let meta = fs::symlink_metadata(item).ok()?;
        (meta.is_dir() || meta.is_file()).then_some(meta.is_dir())
    };
    if let [one] = items
        && let Some(is_folder) = whole(one)
    {
        let mut ditto = Command::new("/usr/bin/ditto");
        ditto.args(["-c", "-k"]);
        if is_folder {
            // The archive holds the folder itself, not just what is in it.
            ditto.arg("--keepParent");
        }
        ditto.arg(one).arg(archive);
        return Ok(ditto);
    }
    let nowhere = || io::Error::other("there is nothing to compress");
    let folder = items.first().and_then(|item| item.parent());
    let mut zip = Command::new("/usr/bin/zip");
    zip.current_dir(folder.ok_or_else(nowhere)?);
    // After `--` a name is a name, whatever it starts with.
    zip.args(["-r", "-y", "-q"]).arg(archive).arg("--");
    for item in items {
        zip.arg(item.file_name().ok_or_else(nowhere)?);
    }
    Ok(zip)
}

/// The command that writes items of one folder into a tar archive,
/// compressed as the archive's ending says (`-a`). The items go in by name,
/// from their folder, written `./name` so that none is taken for an option
/// or for another archive to copy in (`@name`); the `./` is taken off again
/// in the archive. Links stay links.
fn tar_command(items: &[PathBuf], archive: &Path) -> io::Result<Command> {
    let nowhere = || io::Error::other("there is nothing to compress");
    let folder = items.first().and_then(|item| item.parent());
    let mut tar = Command::new("/usr/bin/tar");
    tar.args(["-a", "-c", "-f"])
        .arg(archive)
        .arg("-C")
        .arg(folder.ok_or_else(nowhere)?)
        .args(["-s", ",^\\./,,S", "--"]);
    for item in items {
        let mut name = std::ffi::OsString::from("./");
        name.push(item.file_name().ok_or_else(nowhere)?);
        tar.arg(name);
    }
    Ok(tar)
}

/// The command that writes items into a 7z archive: each by its whole path,
/// which 7-Zip stores under its name, taken literally (`-spd`), links as
/// links (`-snl`).
fn seven_zip_command(items: &[PathBuf], archive: &Path) -> io::Result<Command> {
    let seven = seven_zip().ok_or_else(|| io::Error::other("7-Zip isn't installed"))?;
    let mut command = Command::new(seven);
    command
        .args(["a", "-bd", "-spd", "-snl", "--"])
        .arg(archive)
        .args(items);
    Ok(command)
}

/// Where 7-Zip is, as `7zz` or `7z`, when it is installed.
fn seven_zip() -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{path}:/opt/homebrew/bin:/usr/local/bin");
    program_on("7zz", Some(&path)).or_else(|| program_on("7z", Some(&path)))
}

/// What a command writes, once it ends well, unless it is stopped first.
fn output_of(command: &mut Command, stop: &AtomicBool) -> io::Result<String> {
    use std::io::Read;
    let mut child = detached(quiet(command).stdout(Stdio::piped())).spawn()?;
    let mut output = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no output"))?;
    // Reading waits for the command, so it goes on beside the waiting.
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        output.read_to_end(&mut bytes).map(|_| bytes)
    });
    let ended = wait_until(&mut child, stop);
    let bytes = reader
        .join()
        .map_err(|_| io::Error::other("the output couldn't be read"))??;
    ended?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Lets a command run without the terminal, in a session of its own, so
/// that it can never ask on it for anything, like a password.
fn detached(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: `setsid` is safe to call between `fork` and `exec`.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
    }
}

/// Runs a command to its end, looking now and then whether it should stop:
/// then it is killed, and that is an error like its failing is.
fn run_until(command: &mut Command, stop: &AtomicBool) -> io::Result<()> {
    let mut child = detached(quiet(command)).spawn()?;
    wait_until(&mut child, stop)
}

/// Waits for a command to end well, killing it once `stop` is set.
fn wait_until(child: &mut Child, stop: &AtomicBool) -> io::Result<()> {
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(io::Error::other("the archive couldn't be written")),
            Ok(None) if stop.load(Ordering::Relaxed) => break io::ErrorKind::Interrupted.into(),
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => break error,
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    Err(ended)
}

/// Runs a command with `text` as its input and waits for it to take it.
fn pipe_into(command: &mut Command, text: &str) -> io::Result<()> {
    let mut child = quiet(command).stdin(Stdio::piped()).spawn()?;
    // The pipe closes as this statement ends, which is what lets the command finish.
    let fed = match child.stdin.take() {
        Some(mut pipe) => pipe.write_all(text.as_bytes()),
        None => Err(io::Error::other("it takes no input")),
    };
    let status = child.wait()?;
    fed?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("it didn't take the text"))
    }
}

/// The process showing the preview window, while there is one.
#[derive(Default)]
struct Preview {
    child: RefCell<Option<Child>>,
}

impl Preview {
    /// Closes the window when it is open; otherwise opens one with `open`.
    fn toggle(&self, open: impl FnOnce() -> io::Result<Child>) -> io::Result<()> {
        let mut child = self.child.borrow_mut();
        if let Some(mut showing) = child.take() {
            // Still running: this closes it. Gone: the window was closed by hand.
            if matches!(showing.try_wait(), Ok(None)) {
                let _ = showing.kill();
                let _ = showing.wait();
                return Ok(());
            }
        }
        *child = Some(open()?);
        Ok(())
    }

    #[cfg(test)]
    fn is_open(&self) -> bool {
        let mut child = self.child.borrow_mut();
        child
            .as_mut()
            .is_some_and(|showing| matches!(showing.try_wait(), Ok(None)))
    }
}

impl Drop for Preview {
    /// The window doesn't outlive the app.
    fn drop(&mut self) {
        if let Some(mut showing) = self.child.borrow_mut().take() {
            let _ = showing.kill();
            let _ = showing.wait();
        }
    }
}

/// Keeps a child process from drawing over the terminal UI.
pub fn quiet(command: &mut Command) -> &mut Command {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
}

/// Whether what `diskutil info -plist` says of a volume has it on an
/// internal disk.
fn says_internal(plist: &str) -> bool {
    let Some((_, after)) = plist.split_once("<key>Internal</key>") else {
        return false;
    };
    after.trim_start().starts_with("<true/>")
}

/// Spotlight's own search command.
const MDFIND: &str = "/usr/bin/mdfind";

/// The Spotlight query for what carries every one of `tags`, any case;
/// `None` for no tags, or for one with a quote, a backslash or a wildcard,
/// which would say something else to Spotlight.
fn spotlight_query(tags: &[String]) -> Option<String> {
    let odd = |tag: &String| tag.is_empty() || tag.contains(['\'', '"', '\\', '*']);
    if tags.is_empty() || tags.iter().any(odd) {
        return None;
    }
    let asks: Vec<String> = tags
        .iter()
        .map(|tag| format!("kMDItemUserTags == '{tag}'c"))
        .collect();
    Some(asks.join(" && "))
}

/// `mdfind -count` for `query`, started: it prints how many files match.
fn spotlight_count(query: &str) -> Option<Child> {
    Command::new(MDFIND)
        .arg("-count")
        .arg(query)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_volume_is_internal_only_when_diskutil_says_so() {
        let plist = |internal: &str| {
            format!(
                "<dict>\n\t<key>Ejectable</key>\n\t<false/>\n\t<key>Internal</key>\n\t{internal}\n</dict>"
            )
        };
        assert!(super::says_internal(&plist("<true/>")));
        assert!(!super::says_internal(&plist("<false/>")));
        // Nothing said, or something else first: not internal.
        assert!(!super::says_internal(""));
        assert!(!super::says_internal("<key>InternalX</key><true/>"));
    }

    #[test]
    fn a_spotlight_query_asks_for_every_tag_in_any_case_and_refuses_odd_names() {
        let tags = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            super::spotlight_query(&tags(&["red", "Work"])).as_deref(),
            Some("kMDItemUserTags == 'red'c && kMDItemUserTags == 'Work'c")
        );
        for odd in ["it's", "a\\b", "all*", "say \"hi\""] {
            assert_eq!(super::spotlight_query(&tags(&[odd])), None, "{odd}");
        }
        assert_eq!(super::spotlight_query(&[]), None);
    }

    #[test]
    fn spotlight_says_which_tags_are_in_use_on_a_mac() {
        if !std::path::Path::new(super::MDFIND).is_file() {
            eprintln!("no mdfind: nothing to check");
            return;
        }
        let system = super::MacSystem::new(false);
        assert!(super::System::tags_in_use(&system).is_some());
        // Asked about a folder that isn't there: a search that finds nothing.
        let nowhere = std::path::Path::new("/nowhere-at-all");
        let grep = super::System::spotlight(&system, &["red".to_string()], Some(nowhere));
        let mut found = Vec::new();
        let stop = std::sync::atomic::AtomicBool::new(false);
        grep.expect("a search")(
            &mut |path| {
                found.push(path);
                true
            },
            &stop,
        )
        .unwrap();
        assert!(found.is_empty());
    }
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn the_mouse_is_tracked_by_button_events_and_every_move_only_when_asked() {
        use ratatui::crossterm::Command;
        use ratatui::crossterm::event::DisableMouseCapture;
        let ansi = |command: &dyn Fn(&mut String) -> std::fmt::Result| {
            let mut out = String::new();
            command(&mut out).unwrap();
            out
        };
        let normal = ansi(&|out| TrackMouse { motion: false }.write_ansi(out));
        assert_eq!(normal, "\x1b[?1003l\x1b[?1002h\x1b[?1006h");
        let menu = ansi(&|out| TrackMouse { motion: true }.write_ansi(out));
        assert_eq!(menu, "\x1b[?1002h\x1b[?1006h\x1b[?1003h");
        // What gives the mouse back turns every one of them off.
        let off = ansi(&|out| DisableMouseCapture.write_ansi(out));
        for mode in ["1002l", "1003l", "1006l"] {
            assert!(off.contains(mode), "{mode} in {off:?}");
        }
    }

    #[test]
    fn the_editor_is_editor_or_else_vi() {
        assert_eq!(editor_of(Some("nvim".to_string())), "nvim");
        assert_eq!(editor_of(Some("code -w".to_string())), "code -w");
        assert_eq!(editor_of(Some("  ".to_string())), "vi");
        assert_eq!(editor_of(None), "vi");
    }

    #[test]
    fn a_preview_opens_without_waiting_and_asking_again_closes_it() {
        let preview = Preview::default();
        let opened = std::cell::Cell::new(0);
        let open = || {
            opened.set(opened.get() + 1);
            // Stands in for the Quick Look window: stays until it is closed.
            quiet(Command::new("sleep").arg("30")).spawn()
        };
        preview.toggle(open).unwrap();
        assert!(preview.is_open());
        preview.toggle(open).unwrap();
        assert!(!preview.is_open(), "the second time closes it");
        assert_eq!(opened.get(), 1, "and opens nothing new");
        preview.toggle(open).unwrap();
        assert!(preview.is_open());
        assert_eq!(opened.get(), 2);

        // A window closed by hand is simply opened again.
        let gone = Preview::default();
        gone.toggle(|| quiet(&mut Command::new("true")).spawn())
            .unwrap();
        while gone.is_open() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        gone.toggle(open).unwrap();
        assert!(gone.is_open());
        assert_eq!(opened.get(), 3);
    }

    #[test]
    fn text_for_the_clipboard_is_piped_into_the_command_that_takes_it() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("taken.txt");
        // Stands in for `pbcopy`: keeps what it is given, here in a file.
        let mut keep = Command::new("sh");
        keep.arg("-c").arg("cat > \"$0\"").arg(&taken);
        pipe_into(&mut keep, "/Users/añb/notes.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(&taken).unwrap(),
            "/Users/añb/notes.txt"
        );

        assert!(pipe_into(&mut Command::new("false"), "text").is_err());
        assert!(pipe_into(&mut Command::new("/nonexistent/pbcopy"), "text").is_err());
    }

    #[test]
    fn a_preview_that_cannot_start_says_so() {
        let preview = Preview::default();
        let missing = || Command::new("/nonexistent/qlmanage").spawn();
        assert!(preview.toggle(missing).is_err());
        assert!(!preview.is_open());
    }

    #[test]
    fn a_folder_is_on_a_volume_with_a_type_and_free_space() {
        let dir = tempfile::tempdir().unwrap();
        let volume = volume_of(dir.path()).expect("the volume");
        assert!(!volume.kind.is_empty(), "{volume:?}");
        assert!(volume.free > 0, "{volume:?}");
        assert_eq!(volume_of(&dir.path().join("missing")), None);
    }

    fn args(command: &Command) -> Vec<&OsStr> {
        command.get_args().collect()
    }

    #[test]
    fn the_terminal_to_start_is_the_one_the_app_runs_in() {
        for (program, app) in [
            ("ghostty", "Ghostty"),
            ("Apple_Terminal", "Terminal"),
            ("iTerm.app", "iTerm"),
            ("WezTerm", "WezTerm"),
            ("kitty", "kitty"),
            // One it doesn't know, like tmux, gets the system's own.
            ("tmux", "Terminal"),
            ("", "Terminal"),
        ] {
            assert_eq!(terminal_app(Some(program)), app, "{program}");
        }
        assert_eq!(terminal_app(None), "Terminal");

        let command = terminal_command("Ghostty", Path::new("/h/Lisbon 2026"));
        assert_eq!(command.get_program(), "open");
        assert_eq!(args(&command), ["-a", "Ghostty", "/h/Lisbon 2026"]);
    }

    #[test]
    fn one_file_or_folder_is_zipped_whole_and_the_rest_by_name_from_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        let folder = dir.path().join("Trip");
        let link = dir.path().join("link");
        std::fs::write(&file, "hello").unwrap();
        std::fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink("/", &link).unwrap();
        let archive = dir.path().join(".part").join("out.zip");
        let (file_arg, folder_arg, out) =
            (file.as_os_str(), folder.as_os_str(), archive.as_os_str());

        let one = zip_command(std::slice::from_ref(&file), &archive).unwrap();
        assert_eq!(one.get_program(), "/usr/bin/ditto");
        assert_eq!(
            args(&one),
            [OsStr::new("-c"), OsStr::new("-k"), file_arg, out]
        );

        // A folder's archive holds the folder, not just what is in it.
        let whole = zip_command(std::slice::from_ref(&folder), &archive).unwrap();
        assert_eq!(whole.get_program(), "/usr/bin/ditto");
        let keep = ["-c", "-k", "--keepParent"].map(OsStr::new);
        assert_eq!(args(&whole), [keep[0], keep[1], keep[2], folder_arg, out]);

        // Several go in by name, from the folder they are in; links as links.
        let several = zip_command(&[file, folder], &archive).unwrap();
        assert_eq!(several.get_program(), "/usr/bin/zip");
        assert_eq!(several.get_current_dir(), Some(dir.path()));
        let named = ["-r", "-y", "-q", "--", "notes.txt", "Trip", "link"].map(OsStr::new);
        assert_eq!(
            args(&several),
            [
                named[0], named[1], named[2], out, named[3], named[4], named[5]
            ]
        );

        // A link alone is never followed to what it points at.
        let linked = zip_command(std::slice::from_ref(&link), &archive).unwrap();
        assert_eq!(linked.get_program(), "/usr/bin/zip");
        assert_eq!(linked.get_current_dir(), Some(dir.path()));
        assert_eq!(
            args(&linked),
            [named[0], named[1], named[2], out, named[3], named[6]]
        );
    }

    #[test]
    fn a_command_runs_to_its_end_unless_it_is_stopped_first() {
        let go = AtomicBool::new(false);
        // Stand-ins for the archiver: one that works, one that fails, none.
        assert!(run_until(&mut Command::new("true"), &go).is_ok());
        let failed = run_until(&mut Command::new("false"), &go).unwrap_err();
        assert_eq!(failed.to_string(), "the archive couldn't be written");
        assert!(run_until(&mut Command::new("/nonexistent/zip"), &go).is_err());

        // Stopped, it is killed instead of waited for.
        let stop = AtomicBool::new(true);
        let started = std::time::Instant::now();
        let stopped = run_until(Command::new("sleep").arg("30"), &stop).unwrap_err();
        assert_eq!(stopped.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
    #[test]
    fn the_program_that_greps_is_found_on_the_path_or_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let rg = dir.path().join("rg");
        fs::write(&rg, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&rg, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let path = format!("/nowhere:{}", dir.path().display());
        assert_eq!(program_on("rg", Some(&path)), Some(rg));
        assert_eq!(program_on("rg", Some("/nowhere")), None);
        assert_eq!(program_on("rg", None), None);
    }

    #[test]
    fn grep_asks_for_the_names_of_files_holding_the_text_whatever_the_case() {
        let roots = [PathBuf::from("/a"), PathBuf::from("/b c")];
        let command = grep_command(Path::new("/bin/rg"), &roots, "-x y", false);
        assert_eq!(command.get_program(), "/bin/rg");
        assert_eq!(
            args(&command),
            [
                "--files-with-matches",
                "--fixed-strings",
                "--ignore-case",
                "--no-messages",
                "--one-file-system",
                "--sort",
                "path",
                "-e",
                "-x y",
                "--",
                "/a",
                "/b c"
            ]
        );
        let hidden = grep_command(Path::new("/bin/rg"), &roots, "x", true);
        assert!(args(&hidden).contains(&OsStr::new("--hidden")));
    }

    #[test]
    fn what_a_command_writes_is_handed_over_line_by_line_until_it_is_stopped() {
        let mut lines = Vec::new();
        let mut shell = Command::new("/bin/sh");
        shell.args(["-c", "printf 'one\\ntwo two\\nthree\\n'"]);
        let stop = AtomicBool::new(false);
        stream_lines(
            &mut shell,
            &mut |line| {
                lines.push(line);
                true
            },
            &stop,
        )
        .unwrap();
        assert_eq!(lines, ["one", "two two", "three"].map(PathBuf::from));

        // Told to stop, it kills the command rather than wait for its end.
        let mut slow = Command::new("/bin/sh");
        slow.args(["-c", "echo one; sleep 30; echo two"]);
        let started = std::time::Instant::now();
        let mut seen = 0;
        let stopped = stream_lines(
            &mut slow,
            &mut |_| {
                seen += 1;
                false
            },
            &stop,
        );
        assert!(stopped.is_ok());
        assert_eq!(seen, 1);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// A folder `pack` holding `a.txt` and a link to it, and two files whose
    /// names a program could take for something else, in a folder of their own.
    fn packable() -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("pack").join("sub")).unwrap();
        fs::write(root.join("pack").join("a.txt"), "alpha").unwrap();
        std::os::unix::fs::symlink("a.txt", root.join("pack").join("link")).unwrap();
        fs::write(root.join("@weird"), "at").unwrap();
        fs::write(root.join("-dash"), "dash").unwrap();
        let items = ["pack", "@weird", "-dash"].map(|name| root.join(name));
        (dir, items.to_vec())
    }

    #[test]
    fn compress_offers_its_formats_in_order_and_seven_zip_only_when_installed() {
        let formats = MacSystem::default().formats();
        let written = [
            Format::Zip,
            Format::Tar,
            Format::TarGz,
            Format::TarBz2,
            Format::TarXz,
        ];
        assert_eq!(formats[..5], written);
        assert_eq!(formats.contains(&Format::SevenZ), seven_zip().is_some());
    }

    #[test]
    fn every_format_is_written_and_read_back_by_the_system_tools_names_taken_literally() {
        let (dir, items) = packable();
        let system = MacSystem::default();
        let go = AtomicBool::new(false);
        // How each kind of file starts.
        let starts: [(Format, &[u8], usize); 5] = [
            (Format::Zip, b"PK", 0),
            (Format::Tar, b"ustar", 257),
            (Format::TarGz, b"\x1f\x8b", 0),
            (Format::TarBz2, b"BZh", 0),
            (Format::TarXz, b"\xfd7zXZ", 0),
        ];
        for (format, magic, at) in starts {
            let archive = dir.path().join(format!("out.{}", format.extension()));
            system.archiver(&items, format)(&archive, &go).unwrap();
            let bytes = fs::read(&archive).unwrap();
            assert_eq!(&bytes[at..at + magic.len()], magic, "{format:?}");

            let (list, unpack) = system.extractor(&archive, format).expect("a reader");
            let mut names = list(&go).unwrap();
            names.sort();
            let names: Vec<&str> = names
                .iter()
                .map(|name| name.trim_end_matches('/'))
                .collect();
            assert_eq!(
                names,
                [
                    "-dash",
                    "@weird",
                    "pack",
                    "pack/a.txt",
                    "pack/link",
                    "pack/sub"
                ],
                "{format:?}"
            );
            let into = dir.path().join(format!("into-{}", format.extension()));
            fs::create_dir(&into).unwrap();
            unpack(&into, &go).unwrap();
            assert_eq!(
                fs::read_to_string(into.join("pack/a.txt")).unwrap(),
                "alpha"
            );
            assert_eq!(fs::read_to_string(into.join("@weird")).unwrap(), "at");
            assert_eq!(fs::read_to_string(into.join("-dash")).unwrap(), "dash");
            // The link is still a link, to the same name.
            assert_eq!(
                fs::read_link(into.join("pack/link")).unwrap(),
                Path::new("a.txt"),
                "{format:?}"
            );
            assert!(into.join("pack/sub").is_dir());
        }
    }

    #[test]
    fn an_archive_with_a_password_fails_to_unpack_without_asking_for_it() {
        let (dir, items) = packable();
        let zip = Command::new("/usr/bin/zip")
            .current_dir(dir.path())
            .args(["-q", "-r", "-P", "secret", "enc.zip", "pack"])
            .status()
            .unwrap();
        assert!(zip.success());
        let _ = items;
        let archive = dir.path().join("enc.zip");
        let go = AtomicBool::new(false);
        let (list, unpack) = MacSystem::default()
            .extractor(&archive, Format::Zip)
            .unwrap();
        assert!(list(&go).unwrap().contains(&"pack/a.txt".to_string()));
        let into = dir.path().join("into");
        fs::create_dir(&into).unwrap();
        let started = std::time::Instant::now();
        assert!(unpack(&into, &go).is_err());
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}
