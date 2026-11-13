//! What the sidebar lists.

use std::fs;
use std::path::{Path, PathBuf};

use crate::icons::Icon;
use crate::model::{self, SortMode};

/// What a place is, which decides what Enter and Delete do on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    /// A folder in the home folder.
    Home,
    /// A folder the user pinned.
    Pinned,
    /// The files used lately. Not a folder: its `path` is empty.
    Recents,
    Trash,
    /// A mounted volume.
    Drive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub icon: Icon,
    pub kind: PlaceKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub places: Vec<Place>,
}

/// The whole sidebar: Home, Pinned, Views and Drives, in that order. Pinned
/// and Drives are left out while they have nothing to list.
pub fn sections(home: &Path, pins: &[PathBuf], trash: &Path, volumes: &Path) -> Vec<Section> {
    let pinned = Section {
        title: "Pinned",
        places: pinned_places(pins),
    };
    let drives = Section {
        title: "Drives",
        places: drives(volumes),
    };
    let optional = |section: Section| (!section.places.is_empty()).then_some(section);
    [
        Some(home_section(home)),
        optional(pinned),
        Some(views_section(trash)),
        optional(drives),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Every visible folder in the home folder.
pub fn home_section(home: &Path) -> Section {
    let places = folders(home)
        .map(|(name, path)| Place {
            icon: Icon::for_folder_name(&name),
            label: name,
            path,
            kind: PlaceKind::Home,
        })
        .collect();
    Section {
        title: "Home",
        places,
    }
}

/// The pinned folders that are there, in the order they were pinned.
fn pinned_places(pins: &[PathBuf]) -> Vec<Place> {
    pins.iter()
        .filter(|path| path.is_dir())
        .map(|path| Place {
            label: label(path),
            path: path.clone(),
            icon: Icon::Pin,
            kind: PlaceKind::Pinned,
        })
        .collect()
}

fn views_section(trash: &Path) -> Section {
    let recents = Place {
        label: "Recents".to_string(),
        path: PathBuf::new(),
        icon: Icon::Recents,
        kind: PlaceKind::Recents,
    };
    let trash = Place {
        label: "Trash".to_string(),
        path: trash.to_path_buf(),
        icon: Icon::Trash,
        kind: PlaceKind::Trash,
    };
    Section {
        title: "Views",
        places: vec![recents, trash],
    }
}

/// Every mounted volume but the system's own, which is listed among the
/// volumes as a link to `/`.
fn drives(volumes: &Path) -> Vec<Place> {
    folders(volumes)
        .filter(|(_, path)| !fs::canonicalize(path).is_ok_and(|real| real == Path::new("/")))
        .map(|(name, path)| Place {
            label: name,
            path,
            icon: Icon::Drive,
            kind: PlaceKind::Drive,
        })
        .collect()
}

/// The visible folders in a folder, by name; none when it can't be read.
fn folders(folder: &Path) -> impl Iterator<Item = (String, PathBuf)> {
    model::list_dir(folder, false, SortMode::Name)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| (entry.name, entry.path))
}

/// A folder's name as the sidebar shows it; `/` has none, so it shows as itself.
pub fn label(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}
