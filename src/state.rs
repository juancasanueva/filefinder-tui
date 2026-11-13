//! What is remembered between launches: the view and the size of the tiles.

use std::fs;
use std::io;
use std::path::Path;

use crate::app::{TileSize, ViewMode};

const FILE: &str = "state";

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
}
