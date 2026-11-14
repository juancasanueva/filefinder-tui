//! What the places panel lists, flea's way: PLACES (Applications when it
//! exists, Home, the usual folders in it that exist, then the Trash),
//! TAGS (the colour tags some file carries, in Finder's order) and
//! DEVICES (the system's own volume, iCloud Drive when it is on, then the
//! other mounted volumes). Read on start, on F5 and when the keyboard
//! enters the panel; never while drawing.

use std::fs;
use std::path::{Path, PathBuf};

/// The folders in the home folder PLACES lists when they exist, in order.
pub const KNOWN: [(&str, Kind); 7] = [
    ("Desktop", Kind::Desktop),
    ("Documents", Kind::Documents),
    ("Downloads", Kind::Downloads),
    ("Music", Kind::Music),
    ("Pictures", Kind::Pictures),
    ("Movies", Kind::Movies),
    ("Projects", Kind::Projects),
];

/// What a place is, which decides its glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Applications,
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Movies,
    Projects,
    Trash,
    /// A mounted volume, the system's own among them.
    Drive,
    /// iCloud Drive.
    Cloud,
    /// A colour tag, by its place in `tags::NAMES`: opening it finds what
    /// carries it.
    Tag(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

/// A heading and the places under it; never empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub heading: &'static str,
    pub places: Vec<Place>,
}

/// Every section that has something to list, in order.
pub fn read(
    applications: &Path,
    home: &Path,
    trash: &Path,
    volumes: &Path,
    tags: u8,
) -> Vec<Section> {
    let mut places = Vec::new();
    if applications.is_dir() {
        places.push(Place {
            name: "Applications".to_string(),
            path: applications.to_path_buf(),
            kind: Kind::Applications,
        });
    }
    places.push(Place {
        name: "Home".to_string(),
        path: home.to_path_buf(),
        kind: Kind::Home,
    });
    for (name, kind) in KNOWN {
        let path = home.join(name);
        if path.is_dir() {
            places.push(Place {
                name: name.to_string(),
                path,
                kind,
            });
        }
    }
    places.push(Place {
        name: "Trash".to_string(),
        path: trash.to_path_buf(),
        kind: Kind::Trash,
    });
    let sections = [
        Section {
            heading: "PLACES",
            places,
        },
        Section {
            heading: "TAGS",
            places: tag_places(tags),
        },
        Section {
            heading: "DEVICES",
            places: devices(home, volumes),
        },
    ];
    sections
        .into_iter()
        .filter(|section| !section.places.is_empty())
        .collect()
}

/// The colour tags in `tags`, a mask with Red's bit first, in Finder's
/// order. They hold no folder: their path is empty.
fn tag_places(tags: u8) -> Vec<Place> {
    crate::tags::NAMES
        .iter()
        .enumerate()
        .filter(|(index, _)| tags & (1 << index) != 0)
        .map(|(index, name)| Place {
            name: (*name).to_string(),
            path: PathBuf::new(),
            kind: Kind::Tag(index),
        })
        .collect()
}

/// Where iCloud Drive keeps its files in the home folder.
const ICLOUD_DRIVE: &str = "Library/Mobile Documents/com~apple~CloudDocs";

/// The system's own volume, which opens `/`; iCloud Drive when its folder
/// is there; then the other mounted volumes by name.
fn devices(home: &Path, volumes: &Path) -> Vec<Place> {
    let (mut system, mut drives) = drives(volumes);
    let icloud = home.join(ICLOUD_DRIVE);
    if icloud.is_dir() {
        system.push(Place {
            name: "iCloud Drive".to_string(),
            path: icloud,
            kind: Kind::Cloud,
        });
    }
    system.append(&mut drives);
    system
}

/// The mounted volumes: the system's own, listed among them as a link to
/// `/` and given `/` as its path, then the others by name. None when the
/// folder can't be read.
fn drives(volumes: &Path) -> (Vec<Place>, Vec<Place>) {
    let Ok(children) = fs::read_dir(volumes) else {
        return (Vec::new(), Vec::new());
    };
    let root = Path::new("/");
    let (system, mut drives): (Vec<Place>, Vec<Place>) = children
        .flatten()
        .filter_map(|child| {
            let name = child.file_name().to_string_lossy().into_owned();
            let path = child.path();
            let shown = !name.starts_with('.') && path.is_dir();
            let system = fs::canonicalize(&path).is_ok_and(|real| real == root);
            shown.then(|| Place {
                name,
                path: if system { root.to_path_buf() } else { path },
                kind: Kind::Drive,
            })
        })
        .partition(|place| place.path == root);
    drives.sort_by_key(|place| place.name.to_lowercase());
    (system, drives)
}

/// The places of every section, in the order they are drawn.
pub fn flat(sections: &[Section]) -> impl Iterator<Item = &Place> {
    sections.iter().flat_map(|section| &section.places)
}

/// The place holding `folder`, by its place among them all: the deepest
/// one it is in.
pub fn current(sections: &[Section], folder: &Path) -> Option<usize> {
    flat(sections)
        .enumerate()
        .filter(|(_, place)| !matches!(place.kind, Kind::Tag(_)))
        .filter(|(_, place)| folder.starts_with(&place.path))
        .max_by_key(|(_, place)| place.path.components().count())
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(sections: &[Section]) -> Vec<(&str, Vec<&str>)> {
        sections
            .iter()
            .map(|section| {
                let names = section.places.iter().map(|place| place.name.as_str());
                (section.heading, names.collect())
            })
            .collect()
    }

    #[test]
    fn places_list_applications_home_the_usual_folders_that_exist_and_the_trash_then_the_devices() {
        let dir = tempfile::tempdir().unwrap();
        let applications = dir.path().join("Applications");
        fs::create_dir_all(&applications).unwrap();
        let home = dir.path().join("home");
        for name in ["Projects", "Downloads", "Desktop", "Library", "Movies"] {
            fs::create_dir_all(home.join(name)).unwrap();
        }
        // A file named like one of them is no place.
        fs::write(home.join("Music"), "").unwrap();
        let volumes = dir.path().join("Volumes");
        fs::create_dir_all(volumes.join("backup")).unwrap();
        fs::create_dir_all(volumes.join("Archive")).unwrap();
        fs::create_dir_all(volumes.join(".hidden")).unwrap();
        // The system's own volume is a link to `/`.
        std::os::unix::fs::symlink("/", volumes.join("Macintosh HD")).unwrap();
        let trash = home.join(".Trash");
        let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        fs::create_dir_all(&icloud).unwrap();
        let sections = read(&applications, &home, &trash, &volumes, 0);
        assert_eq!(
            names(&sections),
            [
                (
                    "PLACES",
                    vec![
                        "Applications",
                        "Home",
                        "Desktop",
                        "Downloads",
                        "Movies",
                        "Projects",
                        "Trash"
                    ]
                ),
                (
                    "DEVICES",
                    vec!["Macintosh HD", "iCloud Drive", "Archive", "backup"]
                ),
            ]
        );
        assert_eq!(sections[0].places[0].path, applications);
        assert_eq!(sections[0].places[0].kind, Kind::Applications);
        assert_eq!(sections[0].places[6].path, trash);
        assert_eq!(sections[0].places[6].kind, Kind::Trash);
        // The system's own volume opens the root, iCloud Drive its folder.
        assert_eq!(sections[1].places[0].path, Path::new("/"));
        assert_eq!(sections[1].places[0].kind, Kind::Drive);
        assert_eq!(sections[1].places[1].path, icloud);
        assert_eq!(sections[1].places[1].kind, Kind::Cloud);
        // No volumes: DEVICES lists iCloud Drive alone.
        let sections = read(&applications, &home, &trash, &dir.path().join("none"), 0);
        assert_eq!(names(&sections)[1].1, ["iCloud Drive"]);
        // No volumes and no iCloud Drive: no DEVICES heading.
        fs::remove_dir(&icloud).unwrap();
        let sections = read(&applications, &home, &trash, &dir.path().join("none"), 0);
        assert_eq!(names(&sections).len(), 1);
        // No applications folder: PLACES starts at Home.
        let sections = read(&dir.path().join("none"), &home, &trash, &volumes, 0);
        assert_eq!(names(&sections)[0].1[0], "Home");
    }

    #[test]
    fn the_current_place_is_the_deepest_one_holding_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        fs::create_dir_all(home.join("Pictures/Trip")).unwrap();
        let sections = read(
            &home.join("none"),
            &home,
            &home.join(".Trash"),
            &home.join("none"),
            0,
        );
        assert_eq!(current(&sections, &home.join("Pictures/Trip")), Some(1));
        assert_eq!(current(&sections, &home), Some(0));
        assert_eq!(current(&sections, &home.join(".Trash")), Some(2));
        assert_eq!(current(&sections, Path::new("/")), None);
        // Tag rows hold no folder: never the current place.
        let sections = read(
            &home.join("none"),
            &home,
            &home.join(".Trash"),
            &home.join("none"),
            1,
        );
        assert_eq!(current(&sections, Path::new("/")), None);
        assert_eq!(current(&sections, &home), Some(0));
    }

    #[test]
    fn tags_in_use_list_between_places_and_devices_in_finder_s_order() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let volumes = dir.path().join("Volumes");
        fs::create_dir_all(volumes.join("backup")).unwrap();
        let none = dir.path().join("none");
        // Blue, Red and Gray in use: listed Red, Blue, Gray.
        let mask = crate::tags::mask(&["Blue".into(), "Red".into(), "Gray".into()]);
        let sections = read(&none, &home, &home.join(".Trash"), &volumes, mask);
        assert_eq!(
            names(&sections),
            [
                ("PLACES", vec!["Home", "Trash"]),
                ("TAGS", vec!["Red", "Blue", "Gray"]),
                ("DEVICES", vec!["backup"]),
            ]
        );
        let kinds: Vec<Kind> = sections[1].places.iter().map(|place| place.kind).collect();
        assert_eq!(kinds, [Kind::Tag(0), Kind::Tag(4), Kind::Tag(6)]);
        // No tag in use: no TAGS heading.
        let sections = read(&none, &home, &home.join(".Trash"), &volumes, 0);
        assert_eq!(names(&sections).len(), 2);
    }
}
