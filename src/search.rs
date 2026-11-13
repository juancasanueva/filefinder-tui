//! Looking for items by name at and below a folder, on a thread of its own.
//! No terminal types in here.

use std::collections::VecDeque;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::format::natural_cmp;
use crate::model::{self, Entry};

/// How many matches a search lists before it stops looking.
pub const LIMIT: usize = 500;

/// A search under way. Dropping it stops the walk.
pub struct Walk {
    cancel: Arc<AtomicBool>,
    found: Receiver<Entry>,
}

impl Walk {
    /// Starts looking at and below `root` for names that contain `text`.
    pub fn start(root: PathBuf, text: &str, show_hidden: bool) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, found) = mpsc::channel();
        let flag = cancel.clone();
        let text = text.to_string();
        // Without its thread the walk is over at once, having found nothing.
        let _ = thread::Builder::new()
            .name("search".to_string())
            .spawn(move || {
                // Nobody listening any more means the search was left.
                let mut hand_over = |entry| sender.send(entry).is_ok();
                walk(&root, &text, show_hidden, &flag, &mut hand_over);
            });
        Self { cancel, found }
    }

    /// What was found since the last time, nearest first, and whether the
    /// walk is over.
    pub fn collect(&mut self) -> (Vec<Entry>, bool) {
        let mut entries = Vec::new();
        loop {
            match self.found.try_recv() {
                Ok(entry) => entries.push(entry),
                Err(TryRecvError::Empty) => return (entries, false),
                Err(TryRecvError::Disconnected) => return (entries, true),
            }
        }
    }
}

impl Drop for Walk {
    /// A walk nobody waits for any more stops at the next item.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Goes through `root` one level at a time, so that nearer items come
/// first, and hands over every item whose name contains `text`, whatever
/// the case. It ends after [`LIMIT`] matches, when `cancel` is set or when
/// `found` says nobody is listening.
fn walk(
    root: &Path,
    text: &str,
    show_hidden: bool,
    cancel: &AtomicBool,
    found: &mut dyn FnMut(Entry) -> bool,
) {
    let Ok(device) = fs::metadata(root).map(|meta| meta.dev()) else {
        return;
    };
    let text = text.to_lowercase();
    let mut folders = VecDeque::from([root.to_path_buf()]);
    let mut matches = 0;
    while let Some(folder) = folders.pop_front() {
        for (name, path, meta) in list(&folder) {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            // A hidden folder hides what is inside it as well.
            if !show_hidden && model::is_hidden(&name, &meta) {
                continue;
            }
            // One that went since the folder was listed is simply not found.
            if name.to_lowercase().contains(&text)
                && let Ok(entry) = Entry::from_path(path.clone())
            {
                matches += 1;
                if !found(entry) || matches == LIMIT {
                    return;
                }
            }
            if enters(&meta, device) {
                folders.push_back(path);
            }
        }
    }
}

/// A folder's items by name, each as it is on disk: a symlink is a symlink,
/// not what it points at. A folder that can't be read has none.
fn list(folder: &Path) -> Vec<(String, PathBuf, Metadata)> {
    let Ok(items) = fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut listed: Vec<_> = items
        .flatten()
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            Some((name, item.path(), item.metadata().ok()?))
        })
        .collect();
    listed.sort_by(|(a, ..), (b, ..)| natural_cmp(a, b));
    listed
}

/// Whether the walk goes into an item: a real folder, not a link to one,
/// on the volume the walk started on.
fn enters(meta: &Metadata, device: u64) -> bool {
    meta.is_dir() && meta.dev() == device
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::{Duration, Instant};

    /// A folder holding these; the ones ending in `/` are folders.
    fn tree(paths: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for path in paths {
            let full = dir.path().join(path);
            if path.ends_with('/') {
                fs::create_dir_all(full).unwrap();
            } else {
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(full, "").unwrap();
            }
        }
        dir
    }

    fn below(root: &Path, entry: &Entry) -> String {
        entry.path.strip_prefix(root).unwrap().display().to_string()
    }

    /// Everything a walk finds, in the order it finds it, as paths below `root`.
    fn find(root: &Path, text: &str, show_hidden: bool) -> Vec<String> {
        let mut found = Vec::new();
        let cancel = AtomicBool::new(false);
        walk(root, text, show_hidden, &cancel, &mut |entry| {
            found.push(below(root, &entry));
            true
        });
        found
    }

    #[test]
    fn nearer_items_come_first_and_each_folder_goes_by_name() {
        let dir = tree(&[
            "zeta/report 2.txt",
            "deep/er/report.txt",
            "deep/report.txt",
            "alpha/report 10.txt",
            "alpha/report 9.txt",
            "report.txt",
            "other.txt",
        ]);
        assert_eq!(
            find(dir.path(), "report", false),
            [
                "report.txt",
                "alpha/report 9.txt",
                "alpha/report 10.txt",
                "deep/report.txt",
                "zeta/report 2.txt",
                "deep/er/report.txt",
            ]
        );
    }

    #[test]
    fn files_and_folders_match_by_any_part_of_the_name_whatever_the_case() {
        let dir = tree(&["Photos/PHOTO-1.JPG", "my photo album/", "notes.txt"]);
        let expected = ["my photo album", "Photos", "Photos/PHOTO-1.JPG"];
        assert_eq!(find(dir.path(), "photo", false), expected);
        assert_eq!(find(dir.path(), "PhOtO", false), expected);

        // What is found is an entry like a folder's own: a folder is a folder.
        let mut folders = Vec::new();
        let cancel = AtomicBool::new(false);
        walk(dir.path(), "photo", false, &cancel, &mut |entry| {
            folders.push(entry.is_dir);
            true
        });
        assert_eq!(folders, [true, true, false]);
    }

    #[test]
    fn symlinked_folders_are_not_entered_but_can_match() {
        let dir = tree(&["real/inside.txt"]);
        symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        // One that leads back up would never end.
        symlink(dir.path(), dir.path().join("real").join("loop")).unwrap();
        assert_eq!(find(dir.path(), "inside", false), ["real/inside.txt"]);
        assert_eq!(find(dir.path(), "link", false), ["link"]);
        assert_eq!(find(dir.path(), "loop", false), ["real/loop"]);
    }

    #[test]
    fn hidden_items_and_what_is_inside_them_are_skipped_unless_they_are_shown() {
        let dir = tree(&[
            ".git/config.txt",
            ".config.txt",
            "src/config.txt",
            "src/.config.bak",
        ]);
        assert_eq!(find(dir.path(), "config", false), ["src/config.txt"]);
        assert_eq!(
            find(dir.path(), "config", true),
            [
                ".config.txt",
                ".git/config.txt",
                "src/.config.bak",
                "src/config.txt",
            ]
        );
    }

    #[test]
    fn a_folder_that_cannot_be_read_is_passed_over() {
        let dir = tree(&["locked/secret plan.txt", "open/plan.txt"]);
        let locked = dir.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let plans = find(dir.path(), "plan", false);
        let locks = find(dir.path(), "lock", false);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(plans, ["open/plan.txt"]);
        assert_eq!(locks, ["locked"], "it is still found itself");
    }

    #[test]
    fn the_walk_stops_at_its_limit_of_matches() {
        let names: Vec<String> = (0..LIMIT + 20).map(|n| format!("item {n}.txt")).collect();
        let paths: Vec<&str> = names.iter().map(String::as_str).collect();
        let dir = tree(&paths);
        let found = find(dir.path(), "item", false);
        assert_eq!(found.len(), LIMIT);
        assert_eq!(found[0], "item 0.txt");
        assert_eq!(found[LIMIT - 1], format!("item {}.txt", LIMIT - 1));
    }

    #[test]
    fn a_cancelled_walk_finds_nothing_more() {
        let dir = tree(&["a/match 1", "b/match 2", "c/match 3"]);
        let count = |cancelled: bool, stop_after_one: bool, refuse: bool| {
            let cancel = AtomicBool::new(cancelled);
            let mut found = 0;
            walk(dir.path(), "match", false, &cancel, &mut |_| {
                found += 1;
                cancel.store(stop_after_one, Ordering::Relaxed);
                !refuse
            });
            found
        };
        assert_eq!(count(false, false, false), 3);
        assert_eq!(count(true, false, false), 0, "cancelled before it began");
        assert_eq!(count(false, true, false), 1, "cancelled on the way");
        assert_eq!(count(false, false, true), 1, "nobody is listening");
    }

    #[test]
    fn the_walk_only_goes_into_real_folders_on_the_volume_it_started_on() {
        let dir = tree(&["folder/", "file.txt"]);
        symlink(dir.path().join("folder"), dir.path().join("link")).unwrap();
        let device = fs::metadata(dir.path()).unwrap().dev();
        let on_disk = |name: &str| fs::symlink_metadata(dir.path().join(name)).unwrap();
        assert!(enters(&on_disk("folder"), device));
        assert!(!enters(&on_disk("file.txt"), device));
        assert!(!enters(&on_disk("link"), device));
        // The devices' folder is a volume of its own, mounted on the system's.
        let other = fs::metadata("/dev").unwrap();
        assert_ne!(other.dev(), device);
        assert!(other.is_dir() && !enters(&other, device));
    }

    #[test]
    fn a_walk_hands_over_what_it_finds_until_it_says_it_is_done() {
        let dir = tree(&["a/report.txt", "report.txt", "b/c/report.txt"]);
        let mut walk = Walk::start(dir.path().to_path_buf(), "REPORT", false);
        let mut found = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (entries, finished) = walk.collect();
            found.extend(entries.iter().map(|entry| below(dir.path(), entry)));
            if finished {
                break;
            }
            assert!(Instant::now() < deadline, "the walk never finished");
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(found, ["report.txt", "a/report.txt", "b/c/report.txt"]);
        // Done stays done.
        let (entries, finished) = walk.collect();
        assert!(entries.is_empty() && finished);
    }

    #[test]
    fn dropping_a_walk_ends_its_thread() {
        let dir = tree(&["a/b/c/d/e/report.txt"]);
        let walk = Walk::start(dir.path().to_path_buf(), "report", false);
        // The thread holds the other end of this flag for as long as it runs.
        let flag = walk.cancel.clone();
        drop(walk);
        assert!(flag.load(Ordering::Relaxed), "it was told to stop");
        let deadline = Instant::now() + Duration::from_secs(20);
        while Arc::strong_count(&flag) > 1 {
            assert!(Instant::now() < deadline, "the thread is still running");
            thread::sleep(Duration::from_millis(2));
        }
    }
}
