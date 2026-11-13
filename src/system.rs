//! The port to everything outside the folder tree: launching apps, previews
//! and what Spotlight knows.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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
}
