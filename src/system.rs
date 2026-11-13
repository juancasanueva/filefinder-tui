//! The port to everything outside the folder tree: launching apps and previews.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

pub trait System {
    /// Opens a file with its default app.
    fn open(&self, path: &Path) -> io::Result<()>;
}

pub struct MacSystem;

impl System for MacSystem {
    fn open(&self, path: &Path) -> io::Result<()> {
        let status = quiet(Command::new("open").arg(path)).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("no app is set up to open it"))
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
