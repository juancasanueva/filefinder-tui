//! What the places panel lists, flea's way: PLACES (Home, the usual
//! folders in it that exist, then the Trash) and DEVICES (the mounted
//! volumes but the system's own). Read on start, on F5 and when the
//! keyboard enters the panel; never while drawing.

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
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Movies,
    Projects,
    Trash,
    /// A mounted volume.
    Drive,
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
pub fn read(home: &Path, trash: &Path, volumes: &Path) -> Vec<Section> {
    let mut places = vec![Place {
        name: "Home".to_string(),
        path: home.to_path_buf(),
        kind: Kind::Home,
    }];
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
            heading: "DEVICES",
            places: drives(volumes),
        },
    ];
    sections
        .into_iter()
        .filter(|section| !section.places.is_empty())
        .collect()
}

/// Every mounted volume but the system's own, which is listed among the
/// volumes as a link to `/`; by name. None when the folder can't be read.
fn drives(volumes: &Path) -> Vec<Place> {
    let Ok(children) = fs::read_dir(volumes) else {
        return Vec::new();
    };
    let mut drives: Vec<Place> = children
        .flatten()
        .filter_map(|child| {
            let name = child.file_name().to_string_lossy().into_owned();
            let path = child.path();
            let system = fs::canonicalize(&path).is_ok_and(|real| real == Path::new("/"));
            let shown = !name.starts_with('.') && path.is_dir() && !system;
            shown.then_some(Place {
                name,
                path,
                kind: Kind::Drive,
            })
        })
        .collect();
    drives.sort_by_key(|place| place.name.to_lowercase());
    drives
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
    fn places_list_home_the_usual_folders_that_exist_and_the_trash_then_the_devices() {
        let dir = tempfile::tempdir().unwrap();
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
        let sections = read(&home, &trash, &volumes);
        assert_eq!(
            names(&sections),
            [
                (
                    "PLACES",
                    vec![
                        "Home",
                        "Desktop",
                        "Downloads",
                        "Movies",
                        "Projects",
                        "Trash"
                    ]
                ),
                ("DEVICES", vec!["Archive", "backup"]),
            ]
        );
        assert_eq!(sections[0].places[5].path, trash);
        assert_eq!(sections[0].places[5].kind, Kind::Trash);
        // No volumes: no DEVICES heading.
        let sections = read(&home, &trash, &dir.path().join("none"));
        assert_eq!(names(&sections).len(), 1);
    }

    #[test]
    fn the_current_place_is_the_deepest_one_holding_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        fs::create_dir_all(home.join("Pictures/Trip")).unwrap();
        let sections = read(&home, &home.join(".Trash"), &home.join("none"));
        assert_eq!(current(&sections, &home.join("Pictures/Trip")), Some(1));
        assert_eq!(current(&sections, &home), Some(0));
        assert_eq!(current(&sections, &home.join(".Trash")), Some(2));
        assert_eq!(current(&sections, Path::new("/")), None);
    }
}
