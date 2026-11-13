//! The port to everything outside the folder tree: launching apps, previews,
//! terminals, the program that zips, and what Spotlight knows.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// What writes one zip archive. It is handed to the thread that does file
/// work, and run there with the new file the archive goes in and the flag
/// that stops it: once that is set it gives up, with an error.
pub type Archiver = Box<dyn FnOnce(&Path, &AtomicBool) -> io::Result<()> + Send>;

pub trait System {
    /// Opens a file with its default app.
    fn open(&self, path: &Path) -> io::Result<()>;

    /// Shows a file in a preview window of its own, without waiting for it.
    /// Asked again while that window is still open, closes it instead.
    fn preview(&self, path: &Path) -> io::Result<()>;

    /// Puts text on the system clipboard, for other apps to paste.
    fn copy_text(&self, text: &str) -> io::Result<()>;

    /// The files used lately, the newest first and at most `limit` of them.
    fn recents(&self, limit: usize) -> Vec<PathBuf>;

    /// Opens a terminal window in a folder.
    fn open_terminal(&self, folder: &Path) -> io::Result<()>;

    /// What zips these items, which are all in one folder, into one archive.
    fn archiver(&self, items: &[PathBuf]) -> Archiver;
}

/// What Spotlight is asked for: everything opened in the last thirty days.
const USED_LATELY: &str = "kMDItemLastUsedDate >= $time.today(-30)";

pub struct MacSystem {
    /// Where Spotlight looks for the files used lately.
    home: PathBuf,
    preview: Preview,
}

impl MacSystem {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            preview: Preview::default(),
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

    fn recents(&self, limit: usize) -> Vec<PathBuf> {
        let mut spotlight = Command::new("mdfind");
        spotlight.arg("-onlyin").arg(&self.home).arg(USED_LATELY);
        found(&mut spotlight, limit)
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

    fn archiver(&self, items: &[PathBuf]) -> Archiver {
        let items = items.to_vec();
        Box::new(move |archive, stop| run_until(&mut zip_command(&items, archive)?, stop))
    }
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

/// Runs a command to its end, looking now and then whether it should stop:
/// then it is killed, and that is an error like its failing is.
fn run_until(command: &mut Command, stop: &AtomicBool) -> io::Result<()> {
    let mut child = quiet(command).spawn()?;
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

/// Runs a command that prints one path per line and returns the files among
/// them that are there, the last modified first and at most `limit` of them.
/// A command that fails found nothing.
fn found(command: &mut Command, limit: usize) -> Vec<PathBuf> {
    let printed = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
        .unwrap_or_default();
    let mut files: Vec<_> = printed
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(OsStr::from_bytes(line)))
        .filter_map(|path| {
            let meta = path.metadata().ok().filter(|meta| meta.is_file())?;
            Some((meta.modified().ok(), path))
        })
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.truncate(limit);
    files.into_iter().map(|(_, path)| path).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn what_spotlight_finds_comes_back_newest_first_without_what_is_gone() {
        use std::fs::{self, File};
        use std::time::{Duration, SystemTime};
        let dir = tempfile::tempdir().unwrap();
        let aged = |name: &str, secs: u64| {
            let path = dir.path().join(name);
            let file = File::create(&path).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(secs))
                .unwrap();
            path
        };
        let (old, new, mid) = (
            aged("old.txt", 300),
            aged("new.txt", 10),
            aged("mid.txt", 100),
        );
        let folder = dir.path().join("Folder");
        fs::create_dir(&folder).unwrap();
        let gone = dir.path().join("gone.txt");

        // Stands in for `mdfind`: prints the paths it is given, one per line.
        let mut find = Command::new("printf");
        find.arg("%s\n").args([&old, &gone, &folder, &new, &mid]);
        assert_eq!(found(&mut find, 200), [new.clone(), mid.clone(), old]);
        assert_eq!(found(&mut find, 2), [new, mid]);

        // A Spotlight that fails or isn't there finds nothing.
        assert!(found(&mut Command::new("false"), 200).is_empty());
        assert!(found(&mut Command::new("/nonexistent/mdfind"), 200).is_empty());
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
}
