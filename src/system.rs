//! The port to everything outside the folder tree: launching apps and previews.

use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub trait System {
    /// Opens a file with its default app.
    fn open(&self, path: &Path) -> io::Result<()>;

    /// Shows a file in a preview window of its own, without waiting for it.
    /// Asked again while that window is still open, closes it instead.
    fn preview(&self, path: &Path) -> io::Result<()>;
}

#[derive(Default)]
pub struct MacSystem {
    preview: Preview,
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
    fn a_preview_that_cannot_start_says_so() {
        let preview = Preview::default();
        let missing = || Command::new("/nonexistent/qlmanage").spawn();
        assert!(preview.toggle(missing).is_err());
        assert!(!preview.is_open());
    }
}
