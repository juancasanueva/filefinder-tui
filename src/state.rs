//! What is remembered between launches: the view, the size of the tiles,
//! whether the preview pane shows, and where what is in the trash came from.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app::{TileSize, ViewMode};

const FILE: &str = "state";
const TRASHED: &str = "trashed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    pub view: ViewMode,
    pub tile: TileSize,
    /// Whether the preview pane shows beside the list.
    pub preview: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            view: ViewMode::List,
            tile: TileSize::Normal,
            preview: true,
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
            ("preview", "on") => saved.preview = true,
            ("preview", "off") => saved.preview = false,
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
    let preview = if saved.preview { "on" } else { "off" };
    fs::create_dir_all(dir)?;
    fs::write(
        dir.join(FILE),
        format!("view={view}\ntiles={tiles}\npreview={preview}\n"),
    )
}

/// Something this app moved to the trash: what Restore goes by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// Where it is in the trash.
    pub at: PathBuf,
    /// Where it was before.
    pub from: PathBuf,
    /// When it was moved there, to the second.
    pub when: SystemTime,
}

/// Reads the records of what was trashed, one per line: the second it
/// happened, where the item is and where it was, with tabs between them.
/// None when the file is missing or unreadable; a line that can't be read is
/// no record.
pub fn load_trashed(dir: &Path) -> Vec<Trashed> {
    let text = fs::read_to_string(dir.join(TRASHED)).unwrap_or_default();
    let record = |line: &str| {
        let mut fields = line.split('\t');
        let (when, at, from) = (fields.next()?, fields.next()?, fields.next()?);
        Some(Trashed {
            at: PathBuf::from(unescaped(at)?),
            from: PathBuf::from(unescaped(from)?),
            when: UNIX_EPOCH + Duration::from_secs(when.parse().ok()?),
        })
    };
    text.lines().filter_map(record).collect()
}

/// Writes the records of what was trashed, making the folder of the file
/// the first time. An item whose name can't be written as text has none.
pub fn save_trashed(dir: &Path, records: &[Trashed]) -> io::Result<()> {
    let line = |record: &Trashed| {
        let secs = record.when.duration_since(UNIX_EPOCH).ok()?.as_secs();
        let (at, from) = (record.at.to_str()?, record.from.to_str()?);
        Some(format!("{secs}\t{}\t{}\n", escaped(at), escaped(from)))
    };
    let lines: String = records.iter().filter_map(line).collect();
    fs::create_dir_all(dir)?;
    fs::write(dir.join(TRASHED), lines)
}

/// A path as one field of a line: the tab and the line breaks a name may
/// hold are spelled out, and so is the backslash that spells them.
fn escaped(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The path a field spells; none when it is spelled wrong.
fn unescaped(field: &str) -> Option<String> {
    let mut text = String::new();
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        text.push(match c {
            '\\' => match chars.next()? {
                '\\' => '\\',
                't' => '\t',
                'n' => '\n',
                'r' => '\r',
                _ => return None,
            },
            c => c,
        });
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_launch_starts_with_the_list_normal_tiles_and_the_preview() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load(&dir.path().join("never-written")),
            Saved {
                view: ViewMode::List,
                tile: TileSize::Normal,
                preview: true,
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
            preview: false,
        };
        save(&folder, saved).unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("state")).unwrap(),
            "view=columns\ntiles=small\npreview=off\n"
        );
        assert_eq!(load(&folder), saved);
        let shown = Saved {
            preview: true,
            ..saved
        };
        save(&folder, shown).unwrap();
        assert_eq!(load(&folder), shown);
    }

    #[test]
    fn lines_it_cannot_read_fall_back_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("state"),
            "nonsense\nview=sideways\ntiles = large \npreview=maybe\nlater=thing\n",
        )
        .unwrap();
        assert_eq!(
            load(dir.path()),
            Saved {
                view: ViewMode::List,
                tile: TileSize::Large,
                preview: true,
            }
        );
    }

    #[test]
    fn what_was_trashed_is_kept_one_record_per_line() {
        let dir = tempfile::tempdir().unwrap();
        // The folder is made on the first save.
        let folder = dir.path().join("filefinder-tui");
        assert!(load_trashed(&folder).is_empty());
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        let records = [
            Trashed {
                at: PathBuf::from("/h/.Trash/notes (copy).txt"),
                from: PathBuf::from("/h/Lisbon 2026/notes.txt"),
                when: at(1_794_000_000),
            },
            // A name may hold what separates the fields and the records.
            Trashed {
                at: PathBuf::from("/h/.Trash/a\tb\\c\nd"),
                from: PathBuf::from("/h/a\tb\\c\nd"),
                when: at(1_794_000_001),
            },
        ];
        save_trashed(&folder, &records).unwrap();
        assert_eq!(
            fs::read_to_string(folder.join("trashed")).unwrap(),
            "1794000000\t/h/.Trash/notes (copy).txt\t/h/Lisbon 2026/notes.txt\n\
             1794000001\t/h/.Trash/a\\tb\\\\c\\nd\t/h/a\\tb\\\\c\\nd\n"
        );
        assert_eq!(load_trashed(&folder), records);

        // A line it cannot read is no record.
        fs::write(
            folder.join("trashed"),
            "nonsense\nsoon\t/t/a\t/o/a\n5\t/t/b\n\n7\t/t/c\t/o/c\n",
        )
        .unwrap();
        let read = [Trashed {
            at: PathBuf::from("/t/c"),
            from: PathBuf::from("/o/c"),
            when: at(7),
        }];
        assert_eq!(load_trashed(&folder), read);
    }
}
