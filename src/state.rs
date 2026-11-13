//! What is remembered between launches: the view, the size of the tiles and
//! the pinned folders.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::app::{TileSize, ViewMode};

const FILE: &str = "state";
const PINS: &str = "pinned";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    pub view: ViewMode,
    pub tile: TileSize,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            view: ViewMode::Grid,
            tile: TileSize::Normal,
        }
    }
}

/// Reads `key=value` lines from the state file in a folder; whatever is
/// missing or unreadable keeps its default.
pub fn load(dir: &Path) -> Saved {
    let mut saved = Saved::default();
    let text = fs::read_to_string(dir.join(FILE)).unwrap_or_default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("view", "grid") => saved.view = ViewMode::Grid,
            ("view", "list") => saved.view = ViewMode::List,
            ("view", "columns") => saved.view = ViewMode::Columns,
            ("tiles", "small") => saved.tile = TileSize::Small,
            ("tiles", "normal") => saved.tile = TileSize::Normal,
            ("tiles", "large") => saved.tile = TileSize::Large,
            _ => {}
        }
    }
    saved
}

/// Writes the state file, making its folder the first time.
pub fn save(dir: &Path, saved: Saved) -> io::Result<()> {
    let view = match saved.view {
        ViewMode::Grid => "grid",
        ViewMode::List => "list",
        ViewMode::Columns => "columns",
    };
    let tiles = match saved.tile {
        TileSize::Small => "small",
        TileSize::Normal => "normal",
        TileSize::Large => "large",
    };
    fs::create_dir_all(dir)?;
    fs::write(dir.join(FILE), format!("view={view}\ntiles={tiles}\n"))
}

/// Reads the pinned folders, one path per line; none when the file is
/// missing or unreadable.
pub fn load_pins(dir: &Path) -> Vec<PathBuf> {
    let text = fs::read_to_string(dir.join(PINS)).unwrap_or_default();
    text.lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Writes the pinned folders, making the folder of the file the first time.
pub fn save_pins(dir: &Path, pins: &[PathBuf]) -> io::Result<()> {
    let lines: String = pins
        .iter()
        .map(|path| format!("{}\n", path.display()))
        .collect();
    fs::create_dir_all(dir)?;
    fs::write(dir.join(PINS), lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_launch_starts_with_the_grid_and_normal_tiles() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("never-written")),
            Saved {
                view: ViewMode::Grid,
                tile: TileSize::Normal,
            }
        );
    }

    #[test]
    fn what_is_saved_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        // The folder is made on the first save.
        let folder = dir.path().join("Application Support").join("filefinder");
        let saved = Saved {
            view: ViewMode::Columns,
            tile: TileSize::Small,
        };
        save(&folder, saved).unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("state")).unwrap(),
            "view=columns\ntiles=small\n"
        );
        assert_eq!(load(&folder), saved);
    }

    #[test]
    fn lines_it_cannot_read_fall_back_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("state"),
            "nonsense\nview=sideways\ntiles = large \nlater=thing\n",
        )
        .unwrap();
        assert_eq!(
            load(dir.path()),
            Saved {
                view: ViewMode::Grid,
                tile: TileSize::Large,
            }
        );
    }

    #[test]
    fn pins_are_kept_one_path_per_line() {
        let dir = tempfile::tempdir().unwrap();
        // The folder is made on the first save.
        let folder = dir.path().join("filefinder-tui");
        assert!(load_pins(&folder).is_empty());
        let pins = [PathBuf::from("/a/Lisbon 2026"), PathBuf::from("/b")];
        save_pins(&folder, &pins).unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("pinned")).unwrap(),
            "/a/Lisbon 2026\n/b\n"
        );
        assert_eq!(load_pins(&folder), pins);

        // A blank line is no pin.
        fs::write(folder.join("pinned"), "\n/b\n\n").unwrap();
        assert_eq!(load_pins(&folder), [PathBuf::from("/b")]);
    }
}
