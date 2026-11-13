//! Folder listings: entries, kinds and sorting. No terminal types in here.

use std::cell::OnceCell;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::format::natural_cmp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Folder,
    Image,
    Video,
    Audio,
    Pdf,
    Text,
    Code,
    Archive,
    App,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Kind,
    Newest,
    Oldest,
    Size,
}

impl SortMode {
    pub const ALL: [SortMode; 5] = [
        SortMode::Name,
        SortMode::Kind,
        SortMode::Newest,
        SortMode::Oldest,
        SortMode::Size,
    ];

    /// What the status line says after switching to this order.
    pub fn tip(self) -> &'static str {
        match self {
            SortMode::Name => "Sorted by name",
            SortMode::Kind => "Sorted by kind",
            SortMode::Newest => "Newest first",
            SortMode::Oldest => "Oldest first",
            SortMode::Size => "Biggest first",
        }
    }

    pub fn step(self, delta: isize) -> Self {
        let count = Self::ALL.len() as isize;
        let index = Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0) as isize;
        Self::ALL[(index + delta).rem_euclid(count) as usize]
    }
}

#[derive(Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    items: OnceCell<Option<usize>>,
}

impl Entry {
    pub fn from_path(path: PathBuf) -> io::Result<Self> {
        let link = fs::symlink_metadata(&path)?;
        let is_symlink = link.file_type().is_symlink();
        // A symlink reads as what it points at; a broken one as itself.
        let meta = if is_symlink {
            fs::metadata(&path).unwrap_or(link)
        } else {
            link
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Ok(Self {
            // Application bundles are folders on disk but open like files.
            is_dir: meta.is_dir() && extension(&name).as_deref() != Some("app"),
            size: meta.len(),
            modified: meta.modified().ok(),
            items: OnceCell::new(),
            name,
            path,
        })
    }

    /// How many things a folder holds; read once, the first time it's asked.
    pub fn item_count(&self) -> Option<usize> {
        if !self.is_dir {
            return None;
        }
        *self
            .items
            .get_or_init(|| fs::read_dir(&self.path).ok().map(|dir| dir.count()))
    }

    pub fn category(&self) -> Category {
        category(&self.name, self.is_dir)
    }

    pub fn kind(&self) -> String {
        kind(&self.name, self.is_dir)
    }
}

pub fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_lowercase())
}

pub fn category(name: &str, is_dir: bool) -> Category {
    if is_dir {
        return Category::Folder;
    }
    match extension(name).as_deref().unwrap_or("") {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp"
        | "svg" | "ico" | "avif" => Category::Image,
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" => Category::Video,
        "mp3" | "m4a" | "wav" | "flac" | "aac" | "ogg" | "aiff" | "opus" => Category::Audio,
        "pdf" => Category::Pdf,
        "txt" | "md" | "markdown" | "rtf" | "log" | "csv" => Category::Text,
        "rs" | "swift" | "py" | "js" | "ts" | "tsx" | "jsx" | "json" | "toml" | "yaml" | "yml"
        | "html" | "css" | "sh" | "zsh" | "c" | "h" | "cpp" | "hpp" | "go" | "java" | "kt"
        | "rb" | "lua" | "xml" | "sql" => Category::Code,
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "dmg" | "iso" | "pkg" => {
            Category::Archive
        }
        "app" => Category::App,
        _ => Category::Other,
    }
}

/// The type name shown under a file's name, like `JPEG image`.
pub fn kind(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "Folder".to_string();
    }
    let Some(ext) = extension(name) else {
        return "Document".to_string();
    };
    let known = match ext.as_str() {
        "jpg" | "jpeg" => "JPEG image",
        "png" => "PNG image",
        "gif" => "GIF image",
        "webp" => "WebP image",
        "heic" | "heif" => "HEIC image",
        "tif" | "tiff" => "TIFF image",
        "svg" => "SVG image",
        "pdf" => "PDF document",
        "md" | "markdown" => "Markdown document",
        "txt" => "Plain text",
        "rtf" => "Rich text",
        "csv" => "CSV document",
        "mp4" | "m4v" => "MPEG-4 video",
        "mov" => "QuickTime movie",
        "mkv" => "Matroska video",
        "webm" => "WebM video",
        "mp3" => "MP3 audio",
        "m4a" => "MPEG-4 audio",
        "wav" => "WAV audio",
        "flac" => "FLAC audio",
        "zip" => "Zip archive",
        "dmg" => "Disk image",
        "app" => "Application",
        "rs" => "Rust source",
        "swift" => "Swift source",
        "py" => "Python script",
        "sh" | "zsh" => "Shell script",
        "json" => "JSON document",
        "toml" => "TOML document",
        "yaml" | "yml" => "YAML document",
        "html" => "HTML document",
        _ => "",
    };
    if known.is_empty() {
        let noun = match category(name, false) {
            Category::Image => "image",
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Archive => "archive",
            Category::Code => "source",
            _ => "file",
        };
        format!("{} {noun}", ext.to_uppercase())
    } else {
        known.to_string()
    }
}

/// Dotfiles, plus what macOS itself hides (like `~/Library`).
pub fn is_hidden(name: &str, meta: &Metadata) -> bool {
    name.starts_with('.') || has_hidden_flag(meta)
}

#[cfg(target_os = "macos")]
fn has_hidden_flag(meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const UF_HIDDEN: u32 = 0x8000;
    meta.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn has_hidden_flag(_meta: &Metadata) -> bool {
    false
}

/// Lists a folder: folders first, then by the chosen order.
pub fn list_dir(dir: &Path, show_hidden: bool, sort: SortMode) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if !show_hidden && item.metadata().is_ok_and(|meta| is_hidden(&name, &meta)) {
            continue;
        }
        // Something deleted while we were reading is simply not listed.
        if let Ok(entry) = Entry::from_path(item.path()) {
            entries.push(entry);
        }
    }
    sort_entries(&mut entries, sort);
    Ok(entries)
}

pub fn sort_entries(entries: &mut [Entry], sort: SortMode) {
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            let by_name = || natural_cmp(&a.name, &b.name);
            match sort {
                SortMode::Name => by_name(),
                SortMode::Kind => a.kind().cmp(&b.kind()).then_with(by_name),
                SortMode::Newest => b.modified.cmp(&a.modified).then_with(by_name),
                SortMode::Oldest => a.modified.cmp(&b.modified).then_with(by_name),
                // Folders have no size of their own, so they stay by name.
                SortMode::Size if a.is_dir => by_name(),
                SortMode::Size => b.size.cmp(&a.size).then_with(by_name),
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::time::Duration;

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    fn write(path: &Path, bytes: usize, age_secs: u64) {
        let mut file = File::create(path).unwrap();
        file.write_all(&vec![b'x'; bytes]).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(age_secs))
            .unwrap();
    }

    fn sample() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("zeta")).unwrap();
        fs::create_dir(dir.path().join("Alpha")).unwrap();
        write(&dir.path().join("b.txt"), 30, 300);
        write(&dir.path().join("a.jpg"), 10, 100);
        write(&dir.path().join("c.pdf"), 20, 200);
        write(&dir.path().join(".secret"), 1, 0);
        dir
    }

    #[test]
    fn lists_folders_first_and_leaves_hidden_files_out() {
        let dir = sample();
        let entries = list_dir(dir.path(), false, SortMode::Name).unwrap();
        assert_eq!(
            names(&entries),
            ["Alpha", "zeta", "a.jpg", "b.txt", "c.pdf"]
        );
    }

    #[test]
    fn shows_hidden_files_when_asked() {
        let dir = sample();
        let entries = list_dir(dir.path(), true, SortMode::Name).unwrap();
        assert!(names(&entries).contains(&".secret"));
    }

    #[test]
    fn sorts_by_each_order_keeping_folders_first() {
        let dir = sample();
        let list = |sort| {
            let entries = list_dir(dir.path(), false, sort).unwrap();
            names(&entries)[2..]
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(list(SortMode::Newest), ["a.jpg", "c.pdf", "b.txt"]);
        assert_eq!(list(SortMode::Oldest), ["b.txt", "c.pdf", "a.jpg"]);
        assert_eq!(list(SortMode::Size), ["b.txt", "c.pdf", "a.jpg"]);
        // JPEG image, PDF document, Plain text
        assert_eq!(list(SortMode::Kind), ["a.jpg", "c.pdf", "b.txt"]);
    }

    #[test]
    fn folders_count_their_items() {
        let dir = sample();
        write(&dir.path().join("Alpha").join("one"), 1, 0);
        let entries = list_dir(dir.path(), false, SortMode::Name).unwrap();
        assert_eq!(entries[0].item_count(), Some(1));
        assert_eq!(entries[1].item_count(), Some(0));
        assert_eq!(entries[2].item_count(), None);
    }

    #[test]
    fn names_the_kind_of_a_file() {
        assert_eq!(kind("harbour-dusk.JPG", false), "JPEG image");
        assert_eq!(kind("notes.md", false), "Markdown document");
        assert_eq!(kind("data.xyz", false), "XYZ file");
        assert_eq!(kind("README", false), "Document");
        assert_eq!(kind(".zshrc", false), "Document");
        assert_eq!(kind("Pictures", true), "Folder");
    }

    #[test]
    fn application_bundles_open_like_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("Safari.app")).unwrap();
        let entries = list_dir(dir.path(), false, SortMode::Name).unwrap();
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].kind(), "Application");
    }

    #[test]
    fn sort_modes_cycle_both_ways() {
        assert_eq!(SortMode::Name.step(1), SortMode::Kind);
        assert_eq!(SortMode::Name.step(-1), SortMode::Size);
        assert_eq!(SortMode::Size.step(1), SortMode::Name);
    }
}
