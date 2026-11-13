//! What the sidebar lists.

use std::path::{Path, PathBuf};

use crate::icons::Icon;
use crate::model::{self, SortMode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub icon: Icon,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub places: Vec<Place>,
}

/// Every visible folder in the home folder.
pub fn home_section(home: &Path) -> Section {
    let places = model::list_dir(home, false, SortMode::Name)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| Place {
            icon: Icon::for_folder_name(&entry.name),
            label: entry.name,
            path: entry.path,
        })
        .collect();
    Section {
        title: "Home",
        places,
    }
}
