//! Looking for items at and below a folder, on a thread of its own: by
//! name for Find, by contents through `grep`, or every item below for the
//! filter's subfolders. No terminal types in here.

use std::collections::VecDeque;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::format::natural_cmp;
use crate::matcher::{self, Query, Score, Tier};
use crate::model::{self, Entry};
pub use crate::system::Grep;

/// How many matches a search hands over before it stops looking; as many
/// again of mere typos.
pub const LIMIT: usize = 500;
/// How many items the filter's subfolders take in at most.
pub const BELOW_LIMIT: usize = 20_000;

/// What a walk looks for.
pub enum Looking {
    /// Names that match, scored the way Find ranks them.
    Names(Query),
    /// Every item in the folders under the root, not the root's own.
    Below,
    /// Files whose contents hold the text, as the program that greps finds them.
    Contents(Grep),
}

/// Something a walk found: the item, how well it matched, and its path
/// below the folder looked in.
pub struct Hit {
    pub entry: Entry,
    pub score: Score,
    pub path: String,
}

/// Where a walk goes: the folder it starts in, the volumes it may enter
/// and the folders it leaves out.
#[derive(Clone, Debug)]
pub struct Reach {
    pub root: PathBuf,
    devices: Vec<u64>,
    skip: Vec<PathBuf>,
}

/// What a walk of everywhere leaves out: the system, the swap files and
/// the other volumes.
const NOWHERE: [&str; 5] = ["/System", "/private/var/vm", "/Volumes", "/dev", "/cores"];

impl Reach {
    /// A folder and the folders below it on its own volume.
    pub fn volume(root: PathBuf) -> Self {
        let devices = fs::metadata(&root)
            .map(|meta| meta.dev())
            .into_iter()
            .collect();
        Self {
            root,
            devices,
            skip: Vec::new(),
        }
    }

    /// Everything from `/` down on the system's volume and the home
    /// folder's, leaving out the system itself, the swap files and other
    /// volumes.
    pub fn everywhere(home: &Path) -> Self {
        let devices = [Path::new("/"), home]
            .iter()
            .filter_map(|path| fs::metadata(path).ok().map(|meta| meta.dev()))
            .collect();
        Self {
            root: PathBuf::from("/"),
            devices,
            skip: NOWHERE.iter().map(PathBuf::from).collect(),
        }
    }

    /// Whether a walk goes into an item: a real folder, not a link to one,
    /// on a volume it may enter and not left out.
    pub fn enters(&self, path: &Path, meta: &Metadata) -> bool {
        meta.is_dir()
            && self.devices.contains(&meta.dev())
            && !self.skip.iter().any(|skipped| skipped == path)
    }

    /// Where the program that greps starts, staying on one volume from
    /// each: the root, or for everywhere each folder in it the walk enters.
    pub fn roots(&self) -> Vec<PathBuf> {
        if self.skip.is_empty() {
            return vec![self.root.clone()];
        }
        list(&self.root)
            .into_iter()
            .filter(|(_, path, meta)| self.enters(path, meta))
            .map(|(_, path, _)| path)
            .collect()
    }
}

/// A search under way. Dropping it stops the walk.
pub struct Walk {
    cancel: Arc<AtomicBool>,
    scanned: Arc<AtomicUsize>,
    found: Receiver<Hit>,
}

impl Walk {
    /// Starts looking through `reach` for what `looking` asks.
    pub fn start(reach: Reach, looking: Looking, show_hidden: bool) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let scanned = Arc::new(AtomicUsize::new(0));
        let (sender, found) = mpsc::channel();
        let (flag, count) = (cancel.clone(), scanned.clone());
        // Without its thread the walk is over at once, having found nothing.
        let _ = thread::Builder::new()
            .name("search".to_string())
            .spawn(move || {
                // Nobody listening any more means the search was left.
                let mut hand_over = |hit| sender.send(hit).is_ok();
                match looking {
                    Looking::Contents(grep) => {
                        let root = reach.root.clone();
                        let mut found = |path: PathBuf| match Entry::from_path(path) {
                            // One that went since it was found is simply not found.
                            Err(_) => true,
                            Ok(entry) => {
                                count.fetch_add(1, Ordering::Relaxed);
                                let path = shown_below(&root, &entry.path);
                                let score = Score {
                                    tier: Tier::Exact,
                                    bonus: 0,
                                };
                                hand_over(Hit { entry, score, path })
                            }
                        };
                        // What failed found nothing more.
                        let _ = grep(&mut found, &flag);
                    }
                    looking => walk(&reach, &looking, show_hidden, &flag, &count, &mut hand_over),
                }
            });
        Self {
            cancel,
            scanned,
            found,
        }
    }

    /// What was found since the last time, nearest first, and whether the
    /// walk is over.
    pub fn collect(&mut self) -> (Vec<Hit>, bool) {
        let mut hits = Vec::new();
        loop {
            match self.found.try_recv() {
                Ok(hit) => hits.push(hit),
                Err(TryRecvError::Empty) => return (hits, false),
                Err(TryRecvError::Disconnected) => return (hits, true),
            }
        }
    }

    /// How many items the walk has looked at so far.
    pub fn scanned(&self) -> usize {
        self.scanned.load(Ordering::Relaxed)
    }
}

impl Drop for Walk {
    /// A walk nobody waits for any more stops at the next item.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A path below the root, the way a result names it.
fn shown_below(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Goes through the reach one level at a time, so that nearer items come
/// first, counting every item it looks at in `scanned`, and hands over
/// what `looking` asks for. It ends after its limit, when `cancel` is set
/// or when `found` says nobody is listening.
fn walk(
    reach: &Reach,
    looking: &Looking,
    show_hidden: bool,
    cancel: &AtomicBool,
    scanned: &AtomicUsize,
    found: &mut dyn FnMut(Hit) -> bool,
) {
    let mut folders = VecDeque::from([(reach.root.clone(), 0usize)]);
    let (mut matches, mut typos) = (0, 0);
    let anything = Score {
        tier: Tier::Exact,
        bonus: 0,
    };
    while let Some((folder, depth)) = folders.pop_front() {
        for (name, path, meta) in list(&folder) {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            // A hidden folder hides what is inside it as well.
            if !show_hidden && model::is_hidden(&name, &meta) {
                continue;
            }
            scanned.fetch_add(1, Ordering::Relaxed);
            let below = shown_below(&reach.root, &path);
            let score = match looking {
                Looking::Names(query) => matcher::score(query, &name, &below),
                _ if depth > 0 => Some(anything),
                _ => None,
            };
            // One that went since the folder was listed is simply not found.
            if let Some(score) = score
                && let Ok(entry) = Entry::from_path(path.clone())
            {
                let count = if score.tier == Tier::Typo {
                    &mut typos
                } else {
                    &mut matches
                };
                *count += 1;
                let limit = match looking {
                    Looking::Below => BELOW_LIMIT,
                    _ => LIMIT,
                };
                let full = *count > limit;
                if !full
                    && !found(Hit {
                        entry,
                        score,
                        path: below,
                    })
                {
                    return;
                }
                if matches >= limit {
                    return;
                }
            }
            if reach.enters(&path, &meta) {
                folders.push_back((path, depth + 1));
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

    fn names(text: &str) -> Looking {
        Looking::Names(Query::new(text).unwrap())
    }

    /// Everything a walk finds, in the order it finds it, as paths below `root`.
    fn find(root: &Path, text: &str, show_hidden: bool) -> Vec<String> {
        let mut found = Vec::new();
        let cancel = AtomicBool::new(false);
        let reach = Reach::volume(root.to_path_buf());
        walk(
            &reach,
            &names(text),
            show_hidden,
            &cancel,
            &AtomicUsize::new(0),
            &mut |hit| {
                found.push(below(root, &hit.entry));
                true
            },
        );
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
        let reach = Reach::volume(dir.path().to_path_buf());
        let scanned = AtomicUsize::new(0);
        walk(
            &reach,
            &names("photo"),
            false,
            &cancel,
            &scanned,
            &mut |hit| {
                folders.push(hit.entry.is_dir);
                true
            },
        );
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
            let reach = Reach::volume(dir.path().to_path_buf());
            let scanned = AtomicUsize::new(0);
            walk(
                &reach,
                &names("match"),
                false,
                &cancel,
                &scanned,
                &mut |_| {
                    found += 1;
                    cancel.store(stop_after_one, Ordering::Relaxed);
                    !refuse
                },
            );
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
        let reach = Reach::volume(dir.path().to_path_buf());
        let on_disk = |name: &str| fs::symlink_metadata(dir.path().join(name)).unwrap();
        let enters = |name: &str| reach.enters(&dir.path().join(name), &on_disk(name));
        assert!(enters("folder"));
        assert!(!enters("file.txt"));
        assert!(!enters("link"));
        // The devices' folder is a volume of its own, mounted on the system's.
        let other = fs::metadata("/dev").unwrap();
        assert_ne!(other.dev(), fs::metadata(dir.path()).unwrap().dev());
        assert!(other.is_dir() && !reach.enters(Path::new("/dev"), &other));
    }

    #[test]
    fn a_walk_hands_over_what_it_finds_until_it_says_it_is_done() {
        let dir = tree(&["a/report.txt", "report.txt", "b/c/report.txt"]);
        let reach = Reach::volume(dir.path().to_path_buf());
        let mut walk = Walk::start(reach, names("REPORT"), false);
        let mut found = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (entries, finished) = walk.collect();
            found.extend(entries.iter().map(|hit| below(dir.path(), &hit.entry)));
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
        let reach = Reach::volume(dir.path().to_path_buf());
        let walk = Walk::start(reach, names("report"), false);
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
    /// Lets a walk run to its end: everything it handed over, and how many
    /// items it scanned.
    fn settle(mut walk: Walk) -> (Vec<Hit>, usize) {
        let mut found = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let (hits, finished) = walk.collect();
            found.extend(hits);
            if finished {
                return (found, walk.scanned());
            }
            assert!(Instant::now() < deadline, "the walk never finished");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_walk_scores_what_it_finds_and_counts_what_it_scanned() {
        let dir = tree(&[
            "Trip/day-trip.jpg",
            "trip-plan.md",
            "daytrip.txt",
            "tirp.txt",
            "x.txt",
        ]);
        let reach = Reach::volume(dir.path().to_path_buf());
        let (hits, scanned) = settle(Walk::start(reach, names("trip"), false));
        let tiers: Vec<(String, Tier)> = hits
            .iter()
            .map(|hit| (hit.path.clone(), hit.score.tier))
            .collect();
        assert_eq!(
            tiers,
            [
                ("daytrip.txt".to_string(), Tier::Subsequence),
                ("tirp.txt".to_string(), Tier::Typo),
                ("Trip".to_string(), Tier::Exact),
                ("trip-plan.md".to_string(), Tier::Prefix),
                ("Trip/day-trip.jpg".to_string(), Tier::WordStart),
            ]
        );
        assert_eq!(scanned, 6, "every item it looked at");
    }

    #[test]
    fn below_hands_over_every_item_in_the_folders_under_the_root_but_not_its_own() {
        let dir = tree(&["a/one.txt", "a/b/two.txt", "top.txt", ".hid/three.txt"]);
        let reach = Reach::volume(dir.path().to_path_buf());
        let (hits, _) = settle(Walk::start(reach.clone(), Looking::Below, false));
        let found: Vec<&str> = hits.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(found, ["a/b", "a/one.txt", "a/b/two.txt"]);
        let (hits, _) = settle(Walk::start(reach, Looking::Below, true));
        assert_eq!(hits.len(), 4, "with hidden items shown");
    }

    #[test]
    fn contents_hands_over_what_grep_found_that_is_still_there() {
        let dir = tree(&["notes.txt", "deep/plan.md"]);
        let root = dir.path().to_path_buf();
        let paths = vec![
            root.join("deep/plan.md"),
            root.join("gone.txt"),
            root.join("notes.txt"),
        ];
        let grep: Grep = Box::new(move |found, _stop| {
            for path in paths {
                if !found(path) {
                    break;
                }
            }
            Ok(())
        });
        let (hits, _) = settle(Walk::start(
            Reach::volume(root),
            Looking::Contents(grep),
            false,
        ));
        let found: Vec<&str> = hits.iter().map(|hit| hit.path.as_str()).collect();
        assert_eq!(found, ["deep/plan.md", "notes.txt"]);
    }

    #[test]
    fn everywhere_starts_at_the_root_and_leaves_out_the_system_and_other_volumes() {
        let home = tempfile::tempdir().unwrap();
        let reach = Reach::everywhere(home.path());
        assert_eq!(reach.root, Path::new("/"));
        let enters = |path: &str| {
            let meta = fs::symlink_metadata(path).unwrap();
            reach.enters(Path::new(path), &meta)
        };
        assert!(!enters("/System"));
        assert!(!enters("/dev"));
        assert!(!enters("/Volumes"));
        assert!(enters("/usr"));
        // The home folder's volume is walked too, wherever it is mounted.
        assert!(reach.enters(home.path(), &fs::metadata(home.path()).unwrap()));
        let roots = reach.roots();
        assert!(roots.contains(&PathBuf::from("/usr")));
        for skipped in ["/System", "/dev", "/Volumes"] {
            assert!(!roots.contains(&PathBuf::from(skipped)), "{skipped}");
        }
        // One folder's reach is the folder alone.
        let folder = Reach::volume(home.path().to_path_buf());
        assert_eq!(folder.roots(), [home.path().to_path_buf()]);
    }
}
