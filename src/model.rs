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

/// What a folder is sorted by: four keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

impl SortKey {
    /// The key's name, as the header and the notices say it.
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Modified => "Modified",
            SortKey::Kind => "Kind",
        }
    }
}

/// How a folder is sorted: a key, ascending (A to Z, smallest or oldest
/// first) or reversed. Folders always come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortMode {
    pub key: SortKey,
    pub reversed: bool,
}

impl SortMode {
    /// The order a folder starts in: by name, A to Z.
    #[allow(non_upper_case_globals)]
    pub const Name: SortMode = SortMode {
        key: SortKey::Name,
        reversed: false,
    };

    /// `s`: the next key, Name → Size → Modified → Kind → Name, ascending.
    pub fn next(self) -> Self {
        let key = match self.key {
            SortKey::Name => SortKey::Size,
            SortKey::Size => SortKey::Modified,
            SortKey::Modified => SortKey::Kind,
            SortKey::Kind => SortKey::Name,
        };
        SortMode {
            key,
            reversed: false,
        }
    }

    /// `S`: the same key the other way round.
    pub fn reverse(self) -> Self {
        SortMode {
            reversed: !self.reversed,
            ..self
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The item's own type and permission bits (`st_mode`); for a link,
    /// the link's, the way `ls -l` shows it.
    pub mode: u32,
    /// Who owns it: the user's number.
    pub uid: u32,
    /// Where a symbolic link points, as it is written; `None` for anything else.
    pub link: Option<PathBuf>,
    /// Which of the seven Finder tags it carries, one bit each, bit 0 for Red.
    pub tags: u8,
    items: OnceCell<Option<usize>>,
}

impl Entry {
    pub fn from_path(path: PathBuf) -> io::Result<Self> {
        let link = fs::symlink_metadata(&path)?;
        let mode = std::os::unix::fs::MetadataExt::mode(&link);
        let uid = std::os::unix::fs::MetadataExt::uid(&link);
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
            mode,
            uid,
            link: is_symlink.then(|| fs::read_link(&path).ok()).flatten(),
            tags: crate::tags::mask(&crate::tags::labels(&path)),
            items: OnceCell::new(),
            name,
            path,
        })
    }

    /// Whether it can't be read: a folder that can't be listed, or a file
    /// with no read permission for anyone.
    pub fn unreadable(&self) -> bool {
        if self.is_dir {
            self.item_count().is_none()
        } else {
            self.mode & 0o444 == 0
        }
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
            let ascending = match sort.key {
                SortKey::Name => by_name(),
                SortKey::Kind => a.kind().cmp(&b.kind()).then_with(by_name),
                SortKey::Modified => a.modified.cmp(&b.modified).then_with(by_name),
                // Folders have no size of their own, so they go by name.
                SortKey::Size if a.is_dir => by_name(),
                SortKey::Size => a.size.cmp(&b.size).then_with(by_name),
            };
            if sort.reversed {
                ascending.reverse()
            } else {
                ascending
            }
        })
    });
}

/// The name of the user with this number, or the number when the system
/// has no name for it. Asked once per user.
pub fn owner(uid: u32) -> String {
    thread_local! {
        static NAMES: std::cell::RefCell<std::collections::HashMap<u32, String>> =
            Default::default();
    }
    NAMES.with(|names| {
        names
            .borrow_mut()
            .entry(uid)
            .or_insert_with(|| user_name(uid).unwrap_or_else(|| uid.to_string()))
            .clone()
    })
}

fn user_name(uid: u32) -> Option<String> {
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to memory that lives until the call returns,
    // and `buffer` is as long as the length given.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() {
        return None;
    }
    // SAFETY: the call filled the entry in, and its name points into `buffer`.
    let name = unsafe { std::ffi::CStr::from_ptr((*found).pw_name) };
    Some(name.to_string_lossy().into_owned()).filter(|name| !name.is_empty())
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
    fn sorts_by_each_key_both_ways_keeping_folders_first() {
        let dir = sample();
        let list = |key, reversed| {
            let entries = list_dir(dir.path(), false, SortMode { key, reversed }).unwrap();
            names(&entries)
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        use SortKey::*;
        // Ascending: A to Z, smallest first, oldest first; folders lead by name.
        assert_eq!(
            list(Name, false),
            ["Alpha", "zeta", "a.jpg", "b.txt", "c.pdf"]
        );
        assert_eq!(
            list(Size, false),
            ["Alpha", "zeta", "a.jpg", "c.pdf", "b.txt"]
        );
        // The folders were made a moment apart: only the files are compared.
        assert_eq!(list(Modified, false)[2..], ["b.txt", "c.pdf", "a.jpg"]);
        // JPEG image, PDF document, Plain text
        assert_eq!(
            list(Kind, false),
            ["Alpha", "zeta", "a.jpg", "c.pdf", "b.txt"]
        );
        // Reversed: the exact reverse inside each block, folders still first.
        assert_eq!(
            list(Name, true),
            ["zeta", "Alpha", "c.pdf", "b.txt", "a.jpg"]
        );
        assert_eq!(
            list(Size, true),
            ["zeta", "Alpha", "b.txt", "c.pdf", "a.jpg"]
        );
        assert_eq!(list(Modified, true)[2..], ["a.jpg", "c.pdf", "b.txt"]);
        assert_eq!(
            list(Kind, true),
            ["zeta", "Alpha", "b.txt", "c.pdf", "a.jpg"]
        );
    }

    #[test]
    fn entries_know_their_owner() {
        let dir = sample();
        let entry = Entry::from_path(dir.path().join("a.jpg")).unwrap();
        let uid = std::os::unix::fs::MetadataExt::uid(&fs::metadata(dir.path()).unwrap());
        assert_eq!(entry.uid, uid);
        // The name the system has for the user, or the number without one.
        assert!(!owner(uid).is_empty());
        assert_eq!(owner(4_000_000_000), "4000000000");
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
    fn entries_keep_their_own_permissions_and_type() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("private");
        let file = dir.path().join("notes.txt");
        fs::create_dir(&folder).unwrap();
        fs::write(&file, "hi").unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&folder, dir.path().join("shortcut")).unwrap();

        let entry = |name: &str| Entry::from_path(dir.path().join(name)).unwrap();
        assert_eq!(entry("private").mode & 0o170777, 0o040700);
        assert_eq!(entry("notes.txt").mode & 0o170777, 0o100640);
        // A link is shown as the link itself, the way `ls -l` shows it,
        // while it still opens like the folder it points at.
        let shortcut = entry("shortcut");
        assert_eq!(shortcut.mode & 0o170000, 0o120000);
        assert!(shortcut.is_dir);
    }

    #[test]
    fn s_goes_round_the_keys_ascending_and_s_reverses() {
        let name = SortMode::Name;
        assert_eq!(
            name,
            SortMode {
                key: SortKey::Name,
                reversed: false
            }
        );
        let size = name.next();
        assert_eq!(
            size,
            SortMode {
                key: SortKey::Size,
                reversed: false
            }
        );
        assert_eq!(size.next().key, SortKey::Modified);
        assert_eq!(size.next().next().key, SortKey::Kind);
        // Reversed or not, the next key starts ascending.
        let reversed = size.next().next().reverse();
        assert!(reversed.reversed);
        assert_eq!(reversed.next(), SortMode::Name);
        assert!(!reversed.reverse().reversed);
        assert_eq!(SortKey::Modified.label(), "Modified");
    }

    #[test]
    fn a_link_knows_where_it_points_and_an_unreadable_item_says_so() {
        use std::os::unix::fs::PermissionsExt;
        let dir = sample();
        let link = dir.path().join("to-alpha");
        std::os::unix::fs::symlink("Alpha", &link).unwrap();
        let entry = Entry::from_path(link).unwrap();
        assert_eq!(entry.link, Some(PathBuf::from("Alpha")));
        assert!(entry.is_dir);
        let file = Entry::from_path(dir.path().join("b.txt")).unwrap();
        assert_eq!(file.link, None);
        assert!(!file.unreadable());

        let locked = dir.path().join("locked.txt");
        write(&locked, 1, 0);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o200)).unwrap();
        assert!(Entry::from_path(locked).unwrap().unreadable());
        let shut = dir.path().join("zeta");
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();
        let folder = Entry::from_path(shut.clone()).unwrap();
        let unreadable = folder.unreadable();
        fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(unreadable);
    }
}
