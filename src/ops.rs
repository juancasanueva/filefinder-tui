//! Changes to the user's files: copy, move, rename, trash and delete, and the
//! log that undoes them. Plain `std::fs`, and nothing here ever overwrites.
//!
//! Every new name is taken with a call that fails when something is already
//! there, so a file that appears in the meantime is never written over.
//! Trashing and undoing only ever move things; `delete` alone removes.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// How many "keep both" names are tried before giving up.
const TRIES: usize = 10_000;

/// What went wrong, in words for the person at the keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Asked between files whether the work should stop.
pub trait Cancel {
    fn cancelled(&self) -> bool;
}

impl Cancel for AtomicBool {
    fn cancelled(&self) -> bool {
        self.load(Ordering::Relaxed)
    }
}

/// For work too short to be stopped.
struct Never;

impl Cancel for Never {
    fn cancelled(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
    Duplicate,
    Rename,
    NewFolder,
    Trash,
    Compress,
    Restore,
    Tag,
    /// Several items renamed at once, in `$EDITOR`.
    BulkRename,
    Extract,
}

/// The kinds of archive Compress writes and Extract reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    SevenZ,
}

impl Format {
    /// What Compress always offers, in flea's order; 7z follows when
    /// 7-Zip is installed.
    pub const WRITTEN: [Format; 5] = [
        Format::Zip,
        Format::Tar,
        Format::TarGz,
        Format::TarBz2,
        Format::TarXz,
    ];

    /// The ending its archives get, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::Tar => "tar",
            Format::TarGz => "tar.gz",
            Format::TarBz2 => "tar.bz2",
            Format::TarXz => "tar.xz",
            Format::SevenZ => "7z",
        }
    }

    /// The kind of archive a name ends like, whatever its case, and the
    /// name without that ending: the folder Extract makes for it.
    pub fn of_archive(name: &str) -> Option<(Format, &str)> {
        // The longest endings first, so `.tar.gz` comes before `.tar`.
        const ENDINGS: [(&str, Format); 7] = [
            (".tar.bz2", Format::TarBz2),
            (".tar.gz", Format::TarGz),
            (".tar.xz", Format::TarXz),
            (".tgz", Format::TarGz),
            (".tar", Format::Tar),
            (".zip", Format::Zip),
            (".7z", Format::SevenZ),
        ];
        ENDINGS.iter().find_map(|(ending, format)| {
            let cut = name
                .len()
                .checked_sub(ending.len())
                .filter(|cut| *cut > 0)?;
            let tail = name.get(cut..)?;
            tail.eq_ignore_ascii_case(ending)
                .then(|| (*format, &name[..cut]))
        })
    }
}

/// One thing that was done to one item, with what it takes to take it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Several items renamed together, each from its first path to its
    /// second, through temporary names so that names can swap.
    Renamed { moves: Vec<(PathBuf, PathBuf)> },
    /// Its Finder tags changed from `before` to `after`: the bytes of both
    /// attributes they are kept in, the tags and Finder's colour label.
    Tagged {
        at: PathBuf,
        before: crate::tags::Stored,
        after: crate::tags::Stored,
    },
    /// Moved or renamed.
    Moved { from: PathBuf, to: PathBuf },
    /// Made new: a copy, a duplicate or a folder. Undoing it moves it to the
    /// trash, where it waits at `parked` to be brought back.
    Created {
        at: PathBuf,
        parked: Option<PathBuf>,
    },
    /// Moved to the trash, where it is `to`.
    Trashed { from: PathBuf, to: PathBuf },
}

/// What one command did, as the undo log keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: Kind,
    pub steps: Vec<Step>,
}

impl Change {
    /// Where the things this change made or moved are now.
    pub fn landed(&self) -> Vec<PathBuf> {
        let landed = self.steps.iter().map(|step| match step {
            Step::Moved { to, .. } => vec![to.clone()],
            Step::Created { at, parked: None } | Step::Tagged { at, .. } => vec![at.clone()],
            Step::Renamed { moves } => moves.iter().map(|(_, to)| to.clone()).collect(),
            _ => Vec::new(),
        });
        landed.flatten().collect()
    }

    /// Where the things this change moved or trashed came from.
    pub fn origins(&self) -> Vec<PathBuf> {
        let origins = self.steps.iter().map(|step| match step {
            Step::Moved { from, .. } | Step::Trashed { from, .. } => vec![from.clone()],
            Step::Tagged { at, .. } => vec![at.clone()],
            Step::Renamed { moves } => moves.iter().map(|(from, _)| from.clone()).collect(),
            Step::Created { .. } => Vec::new(),
        });
        origins.flatten().collect()
    }

    /// The name of the first item it touched, as it was before a rename.
    pub fn name(&self) -> String {
        match self.steps.first() {
            Some(Step::Moved { from: path, .. })
            | Some(Step::Trashed { from: path, .. })
            | Some(Step::Created { at: path, .. })
            | Some(Step::Tagged { at: path, .. }) => shown(path),
            Some(Step::Renamed { moves }) => moves
                .first()
                .map(|(from, _)| shown(from))
                .unwrap_or_default(),
            None => String::new(),
        }
    }
}

/// The longest name a file can have on a Mac, in bytes.
const NAME_MAX: usize = 255;

/// Bulk rename: the items' new names, a line each, as they came back from
/// the editor, made into renames. Every line is checked first, in flea's
/// words, and with one wrong nothing is renamed. Unchanged names are left
/// out; a name may be one another item of the batch is giving up.
pub fn plan_renames(items: &[PathBuf], names: &[String]) -> Result<Vec<(PathBuf, PathBuf)>, Error> {
    if names.len() != items.len() {
        return Err(Error(
            "Bulk rename refused: the number of names changed".into(),
        ));
    }
    let nested = items.iter().any(|item| {
        let holds = |other: &&PathBuf| *other != item && item.starts_with(other);
        items.iter().any(|other| holds(&other))
    });
    if nested {
        return Err(Error(
            "Rename selected folders separately from their selected contents".into(),
        ));
    }
    for (index, name) in names.iter().enumerate() {
        let reason = if name.trim().is_empty() {
            "empty".to_string()
        } else if name.contains('/') {
            "contains /".to_string()
        } else if matches!(name.as_str(), "." | "..") || name.contains('\0') {
            "invalid".to_string()
        } else if name.len() > NAME_MAX {
            "too long".to_string()
        } else {
            let folder = items[index].parent();
            let alike = |(earlier, other): &(usize, &String)| {
                items[*earlier].parent() == folder && other.to_lowercase() == name.to_lowercase()
            };
            match names.iter().enumerate().take(index).find(alike) {
                Some((earlier, _)) => format!(
                    "same as line {}. Give each item a different name.",
                    earlier + 1
                ),
                None => continue,
            }
        };
        return Err(Error(format!("Line {}: {reason}", index + 1)));
    }
    let moves: Vec<(PathBuf, PathBuf)> = items
        .iter()
        .zip(names)
        .filter(|(item, name)| item.file_name() != Some(OsStr::new(name.as_str())))
        .map(|(item, name)| (item.clone(), item.with_file_name(name)))
        .collect();
    let ours = |place: &Path| moves.iter().any(|(from, _)| same_entry(from, place));
    if moves.iter().any(|(_, to)| exists(to) && !ours(to)) {
        return Err(Error(
            "Bulk rename refused: a destination already exists".into(),
        ));
    }
    Ok(moves)
}

/// Renames the items as planned, as one change for the undo log.
pub fn bulk_rename(moves: &[(PathBuf, PathBuf)]) -> Result<Change, Error> {
    run_batch(moves)?;
    Ok(Change {
        kind: Kind::BulkRename,
        steps: vec![Step::Renamed {
            moves: moves.to_vec(),
        }],
    })
}

/// The same renames the other way round.
fn reversed(moves: &[(PathBuf, PathBuf)]) -> Vec<(PathBuf, PathBuf)> {
    let back = moves.iter().map(|(from, to)| (to.clone(), from.clone()));
    back.collect()
}

/// Renames each item to its new name: first every one to a temporary name
/// of its own in its folder, then each to its new name, so that names can
/// swap or go round. Nothing is ever renamed onto a name that is taken.
/// When one can't be renamed, every item is put back under its old name.
fn run_batch(moves: &[(PathBuf, PathBuf)]) -> Result<(), Error> {
    let mut aside = Vec::with_capacity(moves.len());
    for (from, _) in moves {
        match set_aside(from) {
            Ok(temporary) => aside.push(temporary),
            Err(error) => {
                let left = put_back(&moves[..aside.len()], &aside);
                return Err(batch_error(from, &error, &left));
            }
        }
    }
    for (index, ((from, to), temporary)) in moves.iter().zip(&aside).enumerate() {
        if let Err(error) = rename_new(temporary, to) {
            // The ones already renamed step aside again, then all go back.
            let mut left = Vec::new();
            for ((_, to), temporary) in moves[..index].iter().zip(&aside) {
                if rename_new(to, temporary).is_err() {
                    left.push(to.clone());
                }
            }
            left.extend(put_back(moves, &aside));
            return Err(batch_error(from, &error, &left));
        }
    }
    Ok(())
}

/// Renames an item to a temporary name of its own in its folder.
fn set_aside(item: &Path) -> io::Result<PathBuf> {
    let folder = item
        .parent()
        .ok_or_else(|| io::Error::other("it has no folder"))?;
    for n in 0..TRIES {
        let temporary = folder.join(format!(".filefinder-rename-{}-{n}", std::process::id()));
        match rename_new(item, &temporary) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            done => return done.map(|()| temporary),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// Puts items set aside back under their old names. Where any had to be
/// left instead.
fn put_back(moves: &[(PathBuf, PathBuf)], aside: &[PathBuf]) -> Vec<PathBuf> {
    let back = moves.iter().zip(aside);
    back.filter(|((from, _), temporary)| rename_new(temporary, from).is_err())
        .map(|(_, temporary)| temporary.clone())
        .collect()
}

fn batch_error(item: &Path, error: &io::Error, left: &[PathBuf]) -> Error {
    let mut said = format!("Couldn't rename {}{}", shown(item), because(error));
    if !left.is_empty() {
        let names: Vec<String> = left.iter().map(|path| shown(path)).collect();
        said.push_str(&format!(
            " Some items could not be put back and are named {}.",
            names.join(", ")
        ));
    }
    Error(said)
}

/// Has the editor change the names, a line each, in a file of its own in a
/// private folder of the system's temporary folder, taken away again after.
/// `None` when the editor failed: nothing is to be renamed then.
pub fn edit_names(
    names: &[String],
    edit: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<Option<Vec<String>>, Error> {
    if names
        .iter()
        .any(|name| name.contains('\n') || name.contains('\r'))
    {
        return Err(Error(
            "Bulk rename cannot represent a filename containing a line break".into(),
        ));
    }
    let base = std::env::temp_dir();
    let folder = (0..TRIES)
        .map(|n| base.join(format!("filefinder-rename-{}-{n}", std::process::id())))
        .find(|folder| fs::DirBuilder::new().mode(0o700).create(folder).is_ok())
        .ok_or_else(|| Error("Bulk rename could not make its edit directory.".into()))?;
    let file = folder.join("names.txt");
    let mut text = names.join("\n");
    text.push('\n');
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .and_then(|mut out| io::Write::write_all(&mut out, text.as_bytes()));
    if written.is_err() {
        let _ = fs::remove_dir_all(&folder);
        return Err(Error("Bulk rename could not make its edit file.".into()));
    }
    let edited = edit(&file);
    let read = fs::read(&file);
    let _ = fs::remove_dir_all(&folder);
    if edited.is_err() {
        return Ok(None);
    }
    let text = read
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| Error("Bulk rename cannot handle a name that is not valid text".into()))?;
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    // The line break that ends the last line ends no line of its own.
    if text.ends_with('\n') {
        lines.pop();
    }
    Ok(Some(lines))
}

/// What a tag key asks for: one of the seven on or off, or none of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tagging {
    /// Taken off where every item has it, added where it is missing.
    Toggle(usize),
    /// Every one of the seven taken off; tags of the user's own stay.
    RemoveAll,
}

/// What a tag change turned out to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retagged {
    Added(usize),
    Removed(usize),
    Cleared,
}

/// Changes the Finder tags of the items, as `tagging` asks. Every item's
/// attribute is read first: one that can't be read is never written over.
/// When one can't be written, the ones already written are put back, so
/// either every item changed or none did.
pub fn retag(items: &[PathBuf], tagging: Tagging) -> Result<(Retagged, Change), Error> {
    use crate::tags;
    let mut read = Vec::with_capacity(items.len());
    for item in items {
        let refused = |why: String| Error(format!("Couldn't tag {}: {why}", shown(item)));
        let stored = match tags::read_stored(item) {
            Ok(stored) => stored,
            Err(error) if tags::is_unsupported(&error) => {
                return Err(Error(tags::UNSUPPORTED.to_string()));
            }
            Err(error) => return Err(refused(why_failed(&error))),
        };
        let labels = match &stored.tags {
            None => Vec::new(),
            Some(bytes) => {
                tags::decode(bytes).ok_or_else(|| refused("its tags can't be read.".into()))?
            }
        };
        read.push((item, stored, labels));
    }
    let has = |labels: &[String], index: usize| tags::mask(labels) & 1 << index != 0;
    let what = match tagging {
        Tagging::Toggle(index) if read.iter().all(|(_, _, labels)| has(labels, index)) => {
            Retagged::Removed(index)
        }
        Tagging::Toggle(index) => Retagged::Added(index),
        Tagging::RemoveAll => Retagged::Cleared,
    };
    let mut steps = Vec::new();
    for (item, before, labels) in read {
        let wanted = match what {
            Retagged::Added(index) => tags::with(&labels, index),
            Retagged::Removed(index) => tags::without(&labels, index),
            Retagged::Cleared => tags::without_known(&labels),
        };
        if wanted == labels {
            continue;
        }
        // Finder's colour label follows the last colour tag, as Foundation
        // keeps it; with none left it goes, or macOS would still show it.
        let after = tags::Stored {
            tags: (!wanted.is_empty()).then(|| tags::encode(&wanted)),
            info: tags::with_colour(before.info.as_deref(), tags::colour(&wanted)),
        };
        if let Err(error) = write_tags(item, &after) {
            // This one and the ones already done go back to what they were.
            let _ = tags::store(item, &before);
            for step in steps.iter().rev() {
                if let Step::Tagged { at, before, .. } = step {
                    let _ = tags::store(at, before);
                }
            }
            return Err(error);
        }
        steps.push(Step::Tagged {
            at: item.clone(),
            before,
            after,
        });
    }
    let change = Change {
        kind: Kind::Tag,
        steps,
    };
    Ok((what, change))
}

/// Writes an item's tags, unless the system would write a file of the
/// user's over to keep them: on a disk without attributes of its own, they
/// go in a `._name` file next to it.
fn write_tags(item: &Path, wanted: &crate::tags::Stored) -> Result<(), Error> {
    let refused = |why: String| Error(format!("Couldn't tag {}: {why}", shown(item)));
    if let (Some(folder), Some(name)) = (item.parent(), item.file_name()) {
        let mut companion = OsString::from("._");
        companion.push(name);
        let companion = folder.join(companion);
        if exists(&companion) && !is_apple_double(&companion) && keeps_companions(folder) {
            let error = io::Error::other(SideBySide(name.to_string_lossy().into_owned()));
            return Err(refused(why_failed(&error)));
        }
    }
    match crate::tags::store(item, wanted) {
        Ok(()) => Ok(()),
        Err(error) if crate::tags::is_unsupported(&error) => {
            Err(Error(crate::tags::UNSUPPORTED.to_string()))
        }
        Err(error) => Err(refused(why_failed(&error))),
    }
}

/// How a command over several items went.
#[derive(Debug)]
pub struct Report {
    /// What was done, for the undo log: everything finished before a stop or
    /// a failure, and the part of a folder that got across before one.
    pub change: Change,
    /// How many items were asked for.
    pub asked: usize,
    /// How many were done whole.
    pub done: usize,
    /// How many were left alone because they already were where they were going.
    pub skipped: usize,
    /// How many were left out because their name was taken and the paste
    /// was told to skip those.
    pub declined: usize,
    /// Whether it was cancelled before the end.
    pub stopped: bool,
    pub error: Option<Error>,
}

/// How a permanent delete went. There is nothing to undo, so no change.
#[derive(Debug)]
pub struct Deleted {
    pub done: usize,
    pub error: Option<Error>,
}

/// `1 item`, `3 items`.
pub fn count(items: usize) -> String {
    if items == 1 {
        "1 item".to_string()
    } else {
        format!("{items} items")
    }
}

/// A name split where a "keep both" number goes: before a file's extension.
/// Folders, dotfiles and names without one have nothing after the split.
pub fn split_name(name: &str, is_dir: bool) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if !is_dir && dot > 0 && dot + 1 < name.len() => name.split_at(dot),
        _ => (name, ""),
    }
}

/// What a paste does with an item whose name is already taken in the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collide {
    /// The item goes in under the next free " copy" name.
    KeepBoth,
    /// The item already there is moved to the trash first, so that undo can
    /// bring it back, and the new one takes its name.
    Replace,
    /// The item stays where it is.
    Skip,
}

/// The names a paste of these items into `folder` would meet there, in the
/// order of the items. An item a move would leave where it is meets none.
pub fn clashes(items: &[PathBuf], folder: &Path, moving: bool) -> Vec<String> {
    let stays = |item: &&PathBuf| moving && in_folder(item, folder);
    let taken = |item: &PathBuf| Some(shown(&taken_place(folder, item.file_name()?)?));
    items
        .iter()
        .filter(|item| !stays(item))
        .filter_map(taken)
        .collect()
}

/// The item in `folder` that has `name`, as the volume decides: the same
/// name, or on a volume that ignores case one spelled otherwise. Its path is
/// spelled the way the folder lists it, so that what is said about it, and
/// where it goes and comes back to, keep its own spelling. `None` when the
/// name is free.
fn taken_place(folder: &Path, name: &OsStr) -> Option<PathBuf> {
    let asked = folder.join(name);
    let meta = fs::symlink_metadata(&asked).ok()?;
    let listed = fs::read_dir(folder).ok().into_iter().flatten().flatten();
    let folded = name.to_string_lossy().to_lowercase();
    let mut same_item = None;
    for entry in listed {
        let listed_name = entry.file_name();
        if listed_name == name {
            return Some(asked);
        }
        // Another spelling of the same name, not another hard link to the
        // same file under an unrelated name.
        if listed_name.to_string_lossy().to_lowercase() != folded {
            continue;
        }
        let path = folder.join(&listed_name);
        let is_it = fs::symlink_metadata(&path)
            .is_ok_and(|other| other.dev() == meta.dev() && other.ino() == meta.ino());
        if is_it && same_item.is_none() {
            same_item = Some(path);
        }
    }
    Some(same_item.unwrap_or(asked))
}

/// Copies or moves files and folders into a folder, treating the names
/// already taken there as `collide` says. Nothing is written over.
pub fn transfer(
    items: &[PathBuf],
    folder: &Path,
    moving: bool,
    collide: Collide,
    trash_dir: &Path,
    cancel: &dyn Cancel,
) -> Report {
    transfer_with(
        items,
        folder,
        moving,
        collide,
        trash_dir,
        cancel,
        &rename_new,
    )
}

fn transfer_with(
    items: &[PathBuf],
    folder: &Path,
    moving: bool,
    collide: Collide,
    trash_dir: &Path,
    cancel: &dyn Cancel,
    rename: Rename,
) -> Report {
    let kind = if moving { Kind::Move } else { Kind::Copy };
    each(kind, items, cancel, |item| {
        let taken = item.file_name().and_then(|name| taken_place(folder, name));
        let keeps_both = |item: &Path| {
            if moving {
                move_one(item, folder, cancel, rename)
            } else {
                copy_one(item, folder, 0, cancel)
            }
        };
        match (collide, taken) {
            // A move leaves what is already in the folder where it is.
            _ if moving && in_folder(item, folder) => keeps_both(item),
            (Collide::KeepBoth, _) | (_, None) => keeps_both(item),
            (Collide::Skip, Some(_)) => Turn {
                steps: Vec::new(),
                end: End::Declined,
            },
            (Collide::Replace, Some(place)) => {
                replace_one(item, &place, moving, trash_dir, cancel, rename)
            }
        }
    })
}

/// Puts `item` at `place`, whose name is taken: the item there goes to the
/// trash first, whole, by a move; then `item` takes the name, only if it is
/// free. When `item` can't be put there the one that was is put back; when
/// that can't be done either it stays in the trash, on record for undo.
fn replace_one(
    item: &Path,
    place: &Path,
    moving: bool,
    trash_dir: &Path,
    cancel: &dyn Cancel,
    rename: Rename,
) -> Turn {
    let name = shown(item);
    let folder = place.parent().unwrap_or(place);
    if inside(folder, item) {
        return Turn::failed(format!("Can't put {name} inside itself."));
    }
    // Replacing an item with itself would change nothing.
    let same = match (fs::symlink_metadata(item), fs::symlink_metadata(place)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    };
    if same {
        return Turn {
            steps: Vec::new(),
            end: End::Declined,
        };
    }
    // The one there can't go while it holds the item that replaces it.
    if inside(item, place) {
        return Turn::failed(format!("Can't replace {}: it holds {name}.", shown(place)));
    }
    // The new item comes in under its own name, whatever the spelling of
    // the one it replaces, which keeps its own in the trash and back.
    let arriving = item
        .file_name()
        .map_or_else(|| place.to_path_buf(), |own| folder.join(own));
    // A name this disk can't give the item stops it before anything moves.
    if let Some(error) = clash(&arriving, Some(item)) {
        let verb = if moving { "move" } else { "copy" };
        return Turn::failed(format!("Couldn't {verb} {name}{}", because(&error)));
    }
    let parked = match trash_item(place, trash_dir, rename, &volume_trash) {
        Ok(parked) => parked,
        Err(error) => {
            return Turn {
                steps: Vec::new(),
                end: End::Failed(error),
            };
        }
    };
    let away = Step::Trashed {
        from: place.to_path_buf(),
        to: parked.clone(),
    };
    let placed = if moving {
        move_new(item, &arriving, cancel, rename)
    } else {
        copy_new(item, &arriving, cancel).map(|()| Landed::Moved)
    };
    let arrived = match (moving, &placed) {
        (true, Ok(Landed::Moved)) => Step::Moved {
            from: item.to_path_buf(),
            to: arriving.clone(),
        },
        _ => created(arriving.clone()),
    };
    let unfinished = match placed {
        Ok(Landed::Moved) => {
            return Turn {
                steps: vec![away, arrived],
                end: End::Done,
            };
        }
        // There are two of it now, so what undo takes away is the copy.
        Ok(Landed::Copied) => {
            let why = format!("Copied {name}, but couldn't remove the original.");
            return Turn {
                steps: vec![away, arrived],
                end: End::Failed(Error(why)),
            };
        }
        Err(unfinished) => unfinished,
    };
    let end = match unfinished.why {
        Halt::Cancelled => End::Stopped,
        Halt::Taken => End::Failed(Error(format!(
            "{} already exists in {}.",
            shown(&arriving),
            folder_name(&arriving)
        ))),
        Halt::Failed(error) if moving => End::Failed(Error(format!(
            "Couldn't move {name}: {}",
            why_failed(&error)
        ))),
        Halt::Failed(error) => {
            End::Failed(Error(format!("Couldn't copy {name}{}", because(&error))))
        }
    };
    if unfinished.left {
        // Part of a folder got there: it is this paste's own. Undo takes it
        // away and brings the one that was there back.
        return Turn {
            steps: vec![away, arrived],
            end,
        };
    }
    match put_with(&parked, place, trash_dir, rename) {
        Ok(()) => Turn {
            steps: Vec::new(),
            end,
        },
        Err(_) => Turn {
            steps: vec![away],
            end,
        },
    }
}

/// Copies files and folders into a folder. A taken name keeps both.
#[cfg(test)]
pub fn copy_into(items: &[PathBuf], folder: &Path, cancel: &dyn Cancel) -> Report {
    each(Kind::Copy, items, cancel, |item| {
        copy_one(item, folder, 0, cancel)
    })
}

/// Copies each item next to itself, under the first free "keep both" name.
pub fn duplicate(items: &[PathBuf], cancel: &dyn Cancel) -> Report {
    each(Kind::Duplicate, items, cancel, |item| match item.parent() {
        Some(folder) => copy_one(item, folder, 1, cancel),
        None => Turn::failed(format!("Couldn't copy {}.", shown(item))),
    })
}

/// Moves files and folders into a folder. A taken name keeps both.
#[cfg(test)]
pub fn move_into(items: &[PathBuf], folder: &Path, cancel: &dyn Cancel) -> Report {
    move_with(items, folder, cancel, &rename_new)
}

/// Moves items to the trash folder, or to the trash of the volume they are on
/// when that is another one. Nothing is ever deleted: an item that can't be
/// moved stays where it is.
pub fn trash(items: &[PathBuf], trash_dir: &Path) -> Report {
    trash_with(items, trash_dir, &rename_new, &volume_trash)
}

/// Deletes for good: files, folders with all they hold, and links themselves,
/// never what a link points at.
pub fn delete(items: &[PathBuf]) -> Deleted {
    let done = items.iter().filter(|item| remove(item).is_ok()).count();
    let failed = items.len() - done;
    let error = (failed > 0).then(|| Error(format!("Couldn't delete {}.", count(failed))));
    Deleted { done, error }
}

/// The name the archive of these items would get right now, flea's way:
/// one item's name without its extension, or the name of the folder
/// several are in, then the format's ending; or the first of flea's copy
/// names after that which is free in their folder (`pack copy.tar.gz`).
pub fn archive_name(items: &[PathBuf], format: Format) -> Result<String, Error> {
    let named = archive_place(items).and_then(|(folder, stem)| {
        let mut free = archive_candidates(&stem, format).filter(|name| !exists(&folder.join(name)));
        free.next()
            .ok_or_else(|| io::ErrorKind::AlreadyExists.into())
    });
    match named {
        Ok(name) => Ok(name.to_string_lossy().into_owned()),
        Err(error) => Err(not_compressed(&error)),
    }
}

/// An archive's name, then flea's copy names: `pack.tar.gz`,
/// `pack copy.tar.gz`, `pack copy 2.tar.gz` and so on.
fn archive_candidates(stem: &str, format: Format) -> impl Iterator<Item = OsString> {
    let (stem, extension) = (stem.to_string(), format.extension());
    (0..TRIES).map(move |n| match n {
        0 => format!("{stem}.{extension}").into(),
        1 => format!("{stem} copy.{extension}").into(),
        n => format!("{stem} copy {n}.{extension}").into(),
    })
}

/// Makes one archive of the items in a format, beside them. `write` is what
/// writes it, to the path it is given: a new file in a folder made for it
/// alone, which takes its free name next to the items only once it is
/// whole. Nothing is overwritten, and an archive that fails or is stopped
/// halfway is removed again.
pub fn compress(
    items: &[PathBuf],
    format: Format,
    cancel: &dyn Cancel,
    write: impl FnOnce(&Path) -> io::Result<()>,
) -> Report {
    let mut report = Report {
        change: Change {
            kind: Kind::Compress,
            steps: Vec::new(),
        },
        asked: items.len(),
        done: 0,
        skipped: 0,
        declined: 0,
        stopped: false,
        error: None,
    };
    if cancel.cancelled() {
        report.stopped = true;
        return report;
    }
    match archive(items, format, write) {
        Ok(at) => {
            report.done = items.len();
            report.change.steps.push(created(at));
        }
        Err(_) if cancel.cancelled() => report.stopped = true,
        Err(error) => report.error = Some(not_compressed(&error)),
    }
    report
}

fn not_compressed(error: &io::Error) -> Error {
    Error(format!("Couldn't compress: {}", why_failed(error)))
}

/// The folder an archive of these items goes in, and its name before the
/// format's ending.
fn archive_place(items: &[PathBuf]) -> io::Result<(PathBuf, String)> {
    let nowhere = || io::Error::other("there is nothing to compress");
    let first = items.first().ok_or_else(nowhere)?;
    let folder = first.parent().ok_or_else(nowhere)?;
    if items.iter().any(|item| item.parent() != Some(folder)) {
        return Err(io::Error::other("the items are in different folders"));
    }
    let stem = match items {
        [one] => {
            let name = shown(one);
            match name.rfind('.') {
                Some(dot) if dot > 0 => name[..dot].to_string(),
                _ => name,
            }
        }
        _ => match folder.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => "archive".to_string(),
        },
    };
    Ok((folder.to_path_buf(), stem))
}

/// Has the archive written and puts it in place. Where it ended up.
fn archive(
    items: &[PathBuf],
    format: Format,
    write: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let (folder, stem) = archive_place(items)?;
    let name = format!("{stem}.{}", format.extension());
    let scratch = scratch_folder(&folder, &name)?;
    let part = scratch.join(&name);
    let names = archive_candidates(&stem, format);
    let placed = write(&part).and_then(|()| park_as(&part, &folder, names, &rename_new));
    // The folder was made a moment ago for this archive alone: what is left
    // in it is the half of one that failed, or nothing.
    let _ = remove(&scratch);
    placed
}

/// Makes a hidden folder of our own in `folder`, for the archive `name` to
/// be written in. It is made only under a name that is free.
fn scratch_folder(folder: &Path, name: &str) -> io::Result<PathBuf> {
    for n in 0..TRIES {
        let scratch = folder.join(format!(".{name}.part-{n}"));
        match fs::create_dir(&scratch) {
            Ok(()) => return Ok(scratch),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// Extracts an archive into a new folder beside it, named after it without
/// its ending (`pack.tar.gz` into `pack`), or the next free copy name. `list`
/// gives the names inside it and `unpack` writes what it holds into the
/// folder it is given. Nothing is written when a name would land outside
/// that folder: one from the root, or one with `..`. The folder is filled
/// out of sight, then takes its name only once it is whole and holds no
/// link that leads outside it; otherwise it is removed, as it is when the
/// work fails or is stopped halfway.
pub fn extract(
    archive: &Path,
    cancel: &dyn Cancel,
    list: impl FnOnce() -> io::Result<Vec<String>>,
    unpack: impl FnOnce(&Path) -> io::Result<()>,
) -> Report {
    let mut report = Report {
        change: Change {
            kind: Kind::Extract,
            steps: Vec::new(),
        },
        asked: 1,
        done: 0,
        skipped: 0,
        declined: 0,
        stopped: false,
        error: None,
    };
    let name = shown(archive);
    let refused = |why: &str| Some(Error(format!("Couldn't extract {name}: {why}")));
    let damaged = "it is damaged or needs a password.";
    let (Some(folder), Some((_, stem))) = (archive.parent(), Format::of_archive(&name)) else {
        report.error = refused("it isn't an archive.");
        return report;
    };
    let names = match list() {
        _ if cancel.cancelled() => {
            report.stopped = true;
            return report;
        }
        Ok(names) => names,
        Err(_) => {
            report.error = refused(damaged);
            return report;
        }
    };
    if names.iter().any(|name| leaves_its_folder(name)) {
        report.error = refused("it holds items that would land outside its folder.");
        return report;
    }
    let scratch = match scratch_folder(folder, stem) {
        Ok(scratch) => scratch,
        Err(error) => {
            report.error = refused(&why_failed(&error));
            return report;
        }
    };
    let inside = scratch.join(stem);
    let unpacked = fs::create_dir(&inside).and_then(|()| unpack(&inside));
    let outcome = match unpacked {
        _ if cancel.cancelled() => Err(None),
        Err(_) => Err(refused(damaged)),
        Ok(()) if links_lead_outside(&inside, &inside) => {
            Err(refused("a link in it points outside its folder."))
        }
        Ok(()) => {
            let names = candidates(OsStr::new(stem), true);
            park_as(&inside, folder, names, &rename_new)
                .map_err(|error| refused(&why_failed(&error)))
        }
    };
    // The folder was made a moment ago for this alone: what is left in it
    // is the part of one that failed, or nothing.
    let _ = remove(&scratch);
    match outcome {
        Ok(at) => {
            report.done = 1;
            report.change.steps.push(created(at));
        }
        Err(None) => report.stopped = true,
        Err(error) => report.error = error,
    }
    report
}

/// Whether a name inside an archive would land outside the folder it is
/// extracted in: one from the root, or one that goes up with `..`.
fn leaves_its_folder(name: &str) -> bool {
    use std::path::Component;
    let path = Path::new(name);
    path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
}

/// Whether any link in `folder`, or below it, points outside `root`: to a
/// path from the root, or up past `root` with `..`.
fn links_lead_outside(root: &Path, folder: &Path) -> bool {
    use std::path::Component;
    let Ok(items) = fs::read_dir(folder) else {
        // What can't be looked through can't be vouched for.
        return true;
    };
    items.flatten().any(|item| {
        let path = item.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return true;
        };
        if meta.is_dir() {
            return links_lead_outside(root, &path);
        }
        if !meta.file_type().is_symlink() {
            return false;
        }
        let Ok(target) = fs::read_link(&path) else {
            return true;
        };
        // How deep below the root the link is, then where its target goes.
        let mut depth = path.parent().map_or(0, |parent| {
            parent
                .strip_prefix(root)
                .map_or(0, |below| below.components().count())
        }) as isize;
        target.components().any(|part| {
            match part {
                Component::Normal(_) => depth += 1,
                Component::ParentDir => depth -= 1,
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => return true,
            }
            depth < 0
        })
    })
}

/// Moves items out of the trash, each back to the exact place given for it:
/// where it was before. An item with no such place, one whose folder is
/// gone and one whose name was taken since stay in the trash.
pub fn restore(items: &[(PathBuf, Option<PathBuf>)], trash_dir: &Path) -> Report {
    let in_trash: Vec<PathBuf> = items.iter().map(|(item, _)| item.clone()).collect();
    let mut places = items.iter().map(|(_, place)| place.as_deref());
    each(Kind::Restore, &in_trash, &Never, |item| {
        restore_one(item, places.next().flatten(), trash_dir)
    })
}

fn restore_one(item: &Path, place: Option<&Path>, trash_dir: &Path) -> Turn {
    let name = shown(item);
    let Some(place) = place else {
        return Turn::failed(format!("{name} has no record of where it came from."));
    };
    let refused = |why: String| Turn::failed(format!("Couldn't restore {name}: {why}"));
    let folder = folder_name(place);
    if !place.parent().is_some_and(Path::is_dir) {
        return refused(format!("{folder} isn't there any more."));
    }
    if exists(place) {
        return refused(format!("{} already exists in {folder}.", shown(place)));
    }
    // The move itself takes the name only if it is still free.
    match put(item, place, trash_dir) {
        Ok(()) => Turn::done(Step::Moved {
            from: item.to_path_buf(),
            to: place.to_path_buf(),
        }),
        Err(error) => Turn {
            steps: Vec::new(),
            end: End::Failed(error),
        },
    }
}

/// What the trash folder directly holds, by name: what emptying it deletes.
/// Finder's own notes about the folder are left out, and a trash that was
/// never made holds nothing. A trash that can't be listed is an error, and so
/// is anything but a real folder in its place: a link is never followed.
pub fn trash_items(trash_dir: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::symlink_metadata(trash_dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
        Ok(meta) if !meta.is_dir() => return Err(io::ErrorKind::NotADirectory.into()),
        Ok(_) => {}
    }
    let mut items = Vec::new();
    for child in fs::read_dir(trash_dir)? {
        let name = child?.file_name();
        if name != ".DS_Store" {
            items.push(trash_dir.join(name));
        }
    }
    items.sort();
    Ok(items)
}

/// Gives an item a new name in its folder. An empty or unchanged name does nothing.
pub fn rename(item: &Path, name: &str) -> Result<Option<Change>, Error> {
    let name = name.trim();
    let old = shown(item);
    if name.is_empty() || name == old {
        return Ok(None);
    }
    check_name(name)?;
    let to = item.with_file_name(name);
    match rename_new(item, &to) {
        Ok(()) => {
            let from = item.to_path_buf();
            Ok(Some(Change {
                kind: Kind::Rename,
                steps: vec![Step::Moved { from, to }],
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(Error(format!("{name} already exists here.")))
        }
        Err(error) => Err(Error(format!("Couldn't rename {old}{}", because(&error)))),
    }
}

/// Makes a folder. An empty name does nothing.
pub fn new_folder(folder: &Path, name: &str) -> Result<Option<Change>, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    check_name(name)?;
    let at = folder.join(name);
    let made = match clash(&at, None) {
        Some(error) => Err(error),
        None => fs::create_dir(&at),
    };
    match made {
        Ok(()) => Ok(Some(Change {
            kind: Kind::NewFolder,
            steps: vec![Step::Created { at, parked: None }],
        })),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(Error(format!("{name} already exists here.")))
        }
        Err(error) => Err(Error(format!("Couldn't create {name}{}", because(&error)))),
    }
}

fn check_name(name: &str) -> Result<(), Error> {
    if matches!(name, "." | "..") {
        Err(Error(
            "A name cannot be a single dot or a double dot.".to_string(),
        ))
    } else if name.contains('/') {
        Err(Error("A name cannot contain /.".to_string()))
    } else {
        Ok(())
    }
}

/// The changes made so far, newest last, and the ones taken back since.
#[derive(Default)]
pub struct History {
    done: Vec<Change>,
    undone: Vec<Change>,
}

impl History {
    /// Remembers a change. What was undone before it can no longer be redone.
    pub fn record(&mut self, change: Change) {
        if !change.steps.is_empty() {
            self.done.push(change);
            self.undone.clear();
        }
    }

    /// Takes back the newest change, if the world still is as it left it.
    pub fn undo(&mut self, trash_dir: &Path) -> Result<Option<Change>, Error> {
        let Some(mut change) = self.done.pop() else {
            return Ok(None);
        };
        if let Some(why) = blocked(&change.steps, Way::Undo) {
            self.done.push(change);
            return Err(Error(format!("Can't undo that: {why}")));
        }
        for index in (0..change.steps.len()).rev() {
            if let Err(error) = change.steps[index].undo(trash_dir) {
                // What was already taken back can be redone; the rest is still done.
                let taken_back = change.steps.split_off(index + 1);
                self.done.push(change.clone());
                if !taken_back.is_empty() {
                    change.steps = taken_back;
                    self.undone.push(change);
                }
                return Err(error);
            }
        }
        self.undone.push(change.clone());
        Ok(Some(change))
    }

    /// Does again the change that was undone last.
    pub fn redo(&mut self, trash_dir: &Path) -> Result<Option<Change>, Error> {
        let Some(mut change) = self.undone.pop() else {
            return Ok(None);
        };
        if let Some(why) = blocked(&change.steps, Way::Redo) {
            self.undone.push(change);
            return Err(Error(format!("Can't redo that: {why}")));
        }
        for index in 0..change.steps.len() {
            if let Err(error) = change.steps[index].redo(trash_dir) {
                // What was already done again can be undone; the rest still waits.
                let waiting = change.steps.split_off(index);
                if !change.steps.is_empty() {
                    self.done.push(change.clone());
                }
                change.steps = waiting;
                self.undone.push(change);
                return Err(error);
            }
        }
        self.done.push(change.clone());
        Ok(Some(change))
    }
}

#[derive(Clone, Copy)]
enum Way {
    Undo,
    Redo,
}

/// Why a change can't be taken back, or done again, as the world is now.
/// Undo takes the steps last to first and redo first to last, so a place a
/// step needs free may be taken now by what a step before it in that order
/// moves away: a Replace, whose new item has the name of the one it sent to
/// the trash.
fn blocked(steps: &[Step], way: Way) -> Option<String> {
    steps.iter().enumerate().find_map(|(index, step)| {
        let before = match way {
            Way::Undo => &steps[index + 1..],
            Way::Redo => &steps[..index],
        };
        let freed: Vec<&Path> = before.iter().filter_map(|step| step.frees(way)).collect();
        step.blocked(way, &freed)
    })
}

impl Step {
    /// The place this step leaves free when it is taken back, or done again.
    fn frees(&self, way: Way) -> Option<&Path> {
        match (self, way) {
            (Step::Moved { to, .. }, Way::Undo) => Some(to),
            (Step::Created { at, .. }, Way::Undo) => Some(at),
            (Step::Moved { from, .. } | Step::Trashed { from, .. }, Way::Redo) => Some(from),
            (Step::Trashed { .. }, Way::Undo)
            | (Step::Created { .. }, Way::Redo)
            | (Step::Tagged { .. } | Step::Renamed { .. }, _) => None,
        }
    }

    /// Why this step can't be taken back, or done again, as the world is
    /// now, once the places in `freed` are free.
    fn blocked(&self, way: Way, freed: &[&Path]) -> Option<String> {
        if let Step::Renamed { moves } = self {
            // Each item is where the batch left it, and each old name free
            // but for the items of the batch itself.
            let turned: Vec<(PathBuf, PathBuf)>;
            let moves = match way {
                Way::Redo => moves,
                Way::Undo => {
                    turned = reversed(moves);
                    &turned
                }
            };
            for (from, _) in moves {
                if !exists(from) {
                    return Some(format!("{} isn't where it was left.", shown(from)));
                }
            }
            let ours = |place: &Path| moves.iter().any(|(from, _)| same_entry(from, place));
            let taken = moves
                .iter()
                .map(|(_, to)| to)
                .find(|to| exists(to) && !ours(to))?;
            return Some(format!(
                "{} already exists in {}.",
                shown(taken),
                folder_name(taken)
            ));
        }
        if let Step::Tagged { at, before, after } = self {
            // The item is still there, tagged as this change left it.
            let left = match way {
                Way::Undo => after,
                Way::Redo => before,
            };
            if !exists(at) {
                return Some(format!("{} isn't where it was left.", shown(at)));
            }
            return match crate::tags::read_stored(at) {
                Ok(now) if now == *left => None,
                _ => Some(format!("the tags of {} changed since.", shown(at))),
            };
        }
        // What has to be there, the place that has to be free, and the item's name.
        let (source, free, item) = match (self, way) {
            (Step::Moved { from, to }, Way::Undo) => (Some(to), Some(from), to),
            (Step::Moved { from, to }, Way::Redo) => (Some(from), Some(to), from),
            (Step::Trashed { from, to }, Way::Undo) => (Some(to), Some(from), from),
            (Step::Trashed { from, .. }, Way::Redo) => (Some(from), None, from),
            (Step::Created { at, .. }, Way::Undo) => (Some(at), None, at),
            (Step::Created { at, parked }, Way::Redo) => (parked.as_ref(), Some(at), at),
            (Step::Tagged { .. } | Step::Renamed { .. }, _) => return None,
        };
        let Some(source) = source.filter(|path| exists(path)) else {
            return Some(format!("{} isn't where it was left.", shown(item)));
        };
        let taken = free.filter(|place| {
            exists(place)
                && !only_case_differs(source, place)
                && !freed.iter().any(|freed| same_entry(freed, place))
        })?;
        Some(format!(
            "{} already exists in {}.",
            shown(taken),
            folder_name(taken)
        ))
    }

    fn undo(&mut self, trash_dir: &Path) -> Result<(), Error> {
        match self {
            Step::Moved { from, to } | Step::Trashed { from, to } => put(to, from, trash_dir),
            Step::Created { at, parked } => {
                *parked = Some(trash_item(at, trash_dir, &rename_new, &volume_trash)?);
                Ok(())
            }
            Step::Tagged { at, before, .. } => write_tags(at, before),
            Step::Renamed { moves } => run_batch(&reversed(moves)),
        }
    }

    fn redo(&mut self, trash_dir: &Path) -> Result<(), Error> {
        match self {
            Step::Moved { from, to } => put(from, to, trash_dir),
            Step::Trashed { from, to } => {
                *to = trash_item(from, trash_dir, &rename_new, &volume_trash)?;
                Ok(())
            }
            Step::Created { at, parked } => {
                if let Some(waiting) = parked.as_deref() {
                    put(waiting, at, trash_dir)?;
                }
                *parked = None;
                Ok(())
            }
            Step::Tagged { at, after, .. } => write_tags(at, after),
            Step::Renamed { moves } => run_batch(moves),
        }
    }
}

/// Moves something to an exact place, which has to be free.
fn put(from: &Path, to: &Path, trash_dir: &Path) -> Result<(), Error> {
    put_with(from, to, trash_dir, &rename_new)
}

/// When it fails across volumes with part of the item already copied, that
/// part is taken away again, so that nothing is left behind that no undo
/// knows about. The item itself is whole where it was: it is only removed
/// there once all of it has arrived.
fn put_with(from: &Path, to: &Path, trash_dir: &Path, rename: Rename) -> Result<(), Error> {
    let name = shown(to);
    match move_new(from, to, &Never, rename) {
        Ok(Landed::Moved) => Ok(()),
        Ok(Landed::Copied) => Err(Error(format!(
            "Copied {name}, but couldn't remove the original."
        ))),
        Err(Unfinished { why, left }) => Err(Error(match why {
            Halt::Taken => format!("{name} already exists in {}.", folder_name(to)),
            // The part that arrived is this move's own: to the trash with it,
            // or removed when no trash takes it.
            Halt::Failed(error) if left && !clear_away(to, trash_dir, rename) => {
                format!("Moved only part of {name}: {}", why_failed(&error))
            }
            Halt::Failed(error) => format!("Couldn't move {name}: {}", why_failed(&error)),
            Halt::Cancelled => format!("Couldn't move {name}."),
        })),
    }
}

/// Takes away the part of an item that a failed move left at `part`: into the
/// trash, or, when no trash takes it, removed. It is a copy this move made
/// itself a moment ago, of an item that is still whole. True once it is gone.
fn clear_away(part: &Path, trash_dir: &Path, rename: Rename) -> bool {
    trash_item(part, trash_dir, rename, &volume_trash).is_ok() || remove(part).is_ok()
}

/// What one item's turn left behind and how it ended.
struct Turn {
    steps: Vec<Step>,
    end: End,
}

enum End {
    Done,
    /// It already was where it was going.
    Skipped,
    /// Its name was taken, and the paste skips those.
    Declined,
    Stopped,
    Failed(Error),
}

impl Turn {
    fn done(step: Step) -> Self {
        Self {
            steps: vec![step],
            end: End::Done,
        }
    }

    fn failed(message: String) -> Self {
        Self {
            steps: Vec::new(),
            end: End::Failed(Error(message)),
        }
    }
}

/// Gives each item its turn, until the end, a cancel or the first failure.
fn each(
    kind: Kind,
    items: &[PathBuf],
    cancel: &dyn Cancel,
    mut turn: impl FnMut(&Path) -> Turn,
) -> Report {
    let mut report = Report {
        change: Change {
            kind,
            steps: Vec::new(),
        },
        asked: items.len(),
        done: 0,
        skipped: 0,
        declined: 0,
        stopped: false,
        error: None,
    };
    for item in items {
        if cancel.cancelled() {
            report.stopped = true;
            break;
        }
        let Turn { steps, end } = turn(item);
        report.change.steps.extend(steps);
        match end {
            End::Done => report.done += 1,
            End::Skipped => report.skipped += 1,
            End::Declined => report.declined += 1,
            End::Stopped => {
                report.stopped = true;
                break;
            }
            End::Failed(error) => {
                report.error = Some(error);
                break;
            }
        }
    }
    report
}

/// Why making or moving one thing ended early.
enum Halt {
    /// The name was taken. Nothing was touched.
    Taken,
    Cancelled,
    Failed(io::Error),
}

/// Work on one item that ended early, and whether it left something behind.
struct Unfinished {
    why: Halt,
    /// Whether the new name had been taken by this work, so that what is there
    /// now is its own: a folder that wasn't filled to the end. When false,
    /// nothing at that name was made by it, whatever is there.
    left: bool,
}

impl Unfinished {
    /// Ended before anything was made.
    fn untouched(why: Halt) -> Self {
        Self { why, left: false }
    }

    /// The name couldn't be taken: something has it, or it can't be made.
    fn refused(error: io::Error) -> Self {
        Self::untouched(if error.kind() == io::ErrorKind::AlreadyExists {
            Halt::Taken
        } else {
            Halt::Failed(error)
        })
    }
}

/// Copies one item into a folder under the first free "keep both" name,
/// starting at the `first` of them (0 is the name itself).
fn copy_one(item: &Path, folder: &Path, first: usize, cancel: &dyn Cancel) -> Turn {
    let name = shown(item);
    let is_link = fs::symlink_metadata(item).is_ok_and(|meta| meta.is_symlink());
    let failed = |because: &str| {
        let what = if is_link { "the link " } else { "" };
        Turn::failed(format!("Couldn't copy {what}{name}{because}"))
    };
    let Some(file_name) = item.file_name() else {
        return failed(".");
    };
    if inside(folder, item) {
        return Turn::failed(format!("Can't put {name} inside itself."));
    }
    for candidate in candidates(file_name, names_like_folder(item)).skip(first) {
        let dest = folder.join(candidate);
        match copy_new(item, &dest, cancel) {
            Ok(()) => return Turn::done(created(dest)),
            Err(Unfinished {
                why: Halt::Taken, ..
            }) => continue,
            Err(Unfinished { why, left }) => {
                // Only what this copy made itself goes on record for undo: the
                // part of a folder that got across, which stays where it is.
                let step = left.then(|| created(dest));
                let end = match why {
                    Halt::Failed(error) => failed(&because(&error)).end,
                    _ => End::Stopped,
                };
                let steps = step.into_iter().collect();
                return Turn { steps, end };
            }
        }
    }
    failed(".")
}

fn created(at: PathBuf) -> Step {
    Step::Created { at, parked: None }
}

/// Makes `dest` a copy of `src`, taking the name only if it is free. Links are
/// copied as links. A file that fails halfway is removed again, so no half
/// file is left; a folder keeps the whole files it already got.
fn copy_new(src: &Path, dest: &Path, cancel: &dyn Cancel) -> Result<(), Unfinished> {
    if cancel.cancelled() {
        return Err(Unfinished::untouched(Halt::Cancelled));
    }
    let failed = |error| Unfinished::untouched(Halt::Failed(error));
    let meta = fs::symlink_metadata(src).map_err(failed)?;
    if let Some(error) = clash(dest, Some(src)) {
        return Err(failed(error));
    }
    let kind = meta.file_type();
    if kind.is_symlink() {
        let target = fs::read_link(src).map_err(failed)?;
        symlink(target, dest).map_err(Unfinished::refused)
    } else if kind.is_dir() {
        fs::create_dir(dest).map_err(Unfinished::refused)?;
        // From here on the folder at `dest` is this copy's own.
        fill(src, dest, &meta, cancel).map_err(|why| Unfinished { why, left: true })
    } else if kind.is_file() {
        // An empty file of our own takes the name; only that is written over.
        // The contents then go in with the dates, flags and attributes.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .map_err(Unfinished::refused)?;
        fs::copy(src, dest).map(|_| ()).map_err(|error| {
            // Half a file is worse than none, and this one is ours to remove.
            let left = fs::remove_file(dest).is_err();
            let why = Halt::Failed(error);
            Unfinished { why, left }
        })
    } else {
        // Pipes, sockets and devices have no contents to copy.
        Err(failed(io::Error::other("it isn't a file or a folder")))
    }
}

/// Copies what a folder holds into the copy of it that was just made.
fn fill(src: &Path, dest: &Path, meta: &fs::Metadata, cancel: &dyn Cancel) -> Result<(), Halt> {
    let mut names = Vec::new();
    for child in fs::read_dir(src).map_err(Halt::Failed)? {
        names.push(child.map_err(Halt::Failed)?.file_name());
    }
    copy_children(src, dest, &names, cancel)?;
    // Last, so that a read-only folder could still be filled.
    let _ = fs::set_permissions(dest, meta.permissions());
    if let (Ok(folder), Ok(modified)) = (fs::File::open(dest), meta.modified()) {
        let _ = folder.set_modified(modified);
    }
    Ok(())
}

/// Copies the named items of `src` into `dest`, a folder this copy made
/// itself, in the order they are given.
fn copy_children(
    src: &Path,
    dest: &Path,
    names: &[OsString],
    cancel: &dyn Cancel,
) -> Result<(), Halt> {
    let mut copied: Vec<&OsStr> = Vec::new();
    for name in names {
        match copy_new(&src.join(name), &dest.join(name), cancel) {
            Ok(()) => copied.push(name),
            Err(Unfinished {
                why: Halt::Taken, ..
            }) => {
                // The folder was made a moment ago for this, so the name can
                // only have gone to the system's own companion of an item
                // that was just copied. Anything else is a failure.
                if !is_companion_made_here(src, dest, name, &copied) {
                    // The user's own `._name`, where the system has put its.
                    let clash = SideBySide::of(&dest.join(name)).map(io::Error::other);
                    return Err(Halt::Failed(
                        clash.unwrap_or_else(|| io::ErrorKind::AlreadyExists.into()),
                    ));
                }
            }
            Err(unfinished) => return Err(unfinished.why),
        }
    }
    Ok(())
}

/// How an AppleDouble file starts: the `._name` file in which macOS keeps the
/// extended attributes of `name` on volumes that have none, like FAT disks.
const APPLE_DOUBLE_MAGIC: [u8; 4] = [0x00, 0x05, 0x16, 0x07];

/// Whether `._name` is already in `dest` because the system put it there, as
/// the companion of an item this copy has just made, so that the one in `src`
/// has nothing left to add. On a FAT disk, copying `one.txt` makes macOS
/// write `._one.txt` next to the copy by itself.
///
/// Only that is let through, and only when both files are such companions.
/// Where the system adds none, a `._name` file is an ordinary file and is
/// copied like any other; a file of the user's own that happens to have such
/// a name is never left out, the copy fails instead.
fn is_companion_made_here(src: &Path, dest: &Path, name: &OsStr, copied: &[&OsStr]) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Some(item) = name.as_bytes().strip_prefix(b"._") else {
        return false;
    };
    copied.contains(&OsStr::from_bytes(item))
        && is_apple_double(&src.join(name))
        && is_apple_double(&dest.join(name))
}

fn is_apple_double(path: &Path) -> bool {
    use std::io::Read;
    let mut start = [0; 4];
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
        && fs::File::open(path).is_ok_and(|mut file| file.read_exact(&mut start).is_ok())
        && start == APPLE_DOUBLE_MAGIC
}

/// Why a name can't be taken on a disk that keeps companion files: the item
/// that can't be next to a file of the user's named `._` and its name.
#[derive(Debug)]
struct SideBySide(String);

impl SideBySide {
    /// The clash a `._name` file is part of; none for any other name.
    fn of(companion: &Path) -> Option<Self> {
        use std::os::unix::ffi::OsStrExt;
        let item = companion.file_name()?.as_bytes().strip_prefix(b"._")?;
        (!item.is_empty()).then(|| Self(String::from_utf8_lossy(item).into_owned()))
    }
}

impl fmt::Display for SideBySide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let item = &self.0;
        write!(f, "{item} and ._{item} can't be side by side on this disk")
    }
}

impl std::error::Error for SideBySide {}

/// Why nothing may be made at `to`, when making it would let the system write
/// over a file of the user's.
///
/// On a disk with no extended attributes of its own, macOS keeps those of
/// `name` in a file `._name` next to it, and writes that file whenever `name`
/// is made or changed, over whatever has the name. So on such a disk an item
/// and an ordinary file called `._` plus its name are never put side by side,
/// whichever of the two would come second: that one is refused, and nothing
/// is written. A `._name` that is such a companion already is the system's,
/// and no clash; neither is any of this on a disk that stores attributes
/// itself, where `._name` is a name like any other.
///
/// `incoming` is the item about to get the name, when there is one already.
fn clash(to: &Path, incoming: Option<&Path>) -> Option<io::Error> {
    let (folder, name) = (to.parent()?, to.file_name()?);
    let (other, item) = match SideBySide::of(to) {
        // An ordinary `._name` coming in, next to `name`.
        Some(item) if !incoming.is_some_and(is_apple_double) => (folder.join(&item.0), item),
        Some(_) => return None,
        // `name` coming in, next to an ordinary `._name`.
        None => {
            let mut companion = OsString::from("._");
            companion.push(name);
            let companion = folder.join(companion);
            if is_apple_double(&companion) {
                return None;
            }
            let item = SideBySide(name.to_string_lossy().into_owned());
            (companion, item)
        }
    };
    (exists(&other) && keeps_companions(folder)).then(|| io::Error::other(item))
}

/// How a failure's sentence ends: with the reason when it is this clash,
/// which the user can do something about, and a full stop otherwise.
fn because(error: &io::Error) -> String {
    if is_clash(error) {
        format!(": {}", why_failed(error))
    } else {
        ".".to_string()
    }
}

fn is_clash(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<SideBySide>())
}

/// Whether the disk a folder is on has no extended attributes of its own, so
/// that macOS keeps those of `name` in a `._name` file next to it, as on FAT
/// disks. A disk that doesn't say counts as one that does.
fn keeps_companions(folder: &Path) -> bool {
    #[cfg(test)]
    if tests::is_pretend_fat(folder) {
        return true;
    }
    let asked = has_own_attributes(folder).or_else(|| {
        // Older systems only answer for the top folder of the disk.
        has_own_attributes(&mount_point(folder).ok()?)
    });
    asked != Some(true)
}

/// Whether the disk a path is on stores extended attributes itself, when it
/// says: the capability the system goes by when it decides to keep them in
/// `._name` files instead.
#[cfg(target_os = "macos")]
fn has_own_attributes(path: &Path) -> Option<bool> {
    use std::ffi::{CString, c_char, c_int, c_uint, c_void};
    use std::os::unix::ffi::OsStrExt;

    /// `struct attrlist`: which attributes are asked for.
    #[repr(C)]
    struct Asked {
        bitmapcount: u16,
        reserved: u16,
        commonattr: u32,
        volattr: u32,
        dirattr: u32,
        fileattr: u32,
        forkattr: u32,
    }
    /// The answer: its length, then `vol_capabilities_attr_t`.
    #[repr(C)]
    #[derive(Default)]
    struct Answer {
        length: u32,
        capabilities: [u32; 4],
        valid: [u32; 4],
    }
    unsafe extern "C" {
        fn getattrlist(
            path: *const c_char,
            asked: *mut c_void,
            answer: *mut c_void,
            size: usize,
            options: c_uint,
        ) -> c_int;
    }
    const ATTR_BIT_MAP_COUNT: u16 = 5;
    const ATTR_VOL_INFO: u32 = 0x8000_0000;
    const ATTR_VOL_CAPABILITIES: u32 = 0x0002_0000;
    const VOL_CAPABILITIES_INTERFACES: usize = 1;
    const VOL_CAP_INT_EXTENDED_ATTR: u32 = 0x0000_4000;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut asked = Asked {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: ATTR_VOL_INFO | ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut answer = Answer::default();
    // SAFETY: the path is a NUL-terminated string, and both structures are
    // laid out as the call expects and live until it returns; it writes at
    // most `size` bytes into the answer.
    let answered = unsafe {
        getattrlist(
            path.as_ptr(),
            (&raw mut asked).cast(),
            (&raw mut answer).cast(),
            size_of::<Answer>(),
            0,
        )
    } == 0;
    let known =
        answered && answer.valid[VOL_CAPABILITIES_INTERFACES] & VOL_CAP_INT_EXTENDED_ATTR != 0;
    known.then(|| answer.capabilities[VOL_CAPABILITIES_INTERFACES] & VOL_CAP_INT_EXTENDED_ATTR != 0)
}

/// Only macOS keeps such files.
#[cfg(not(target_os = "macos"))]
fn has_own_attributes(_path: &Path) -> Option<bool> {
    Some(true)
}

type Rename<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;
type VolumeTrash<'a> = &'a dyn Fn(&Path) -> io::Result<PathBuf>;

#[cfg(test)]
fn move_with(items: &[PathBuf], folder: &Path, cancel: &dyn Cancel, rename: Rename) -> Report {
    each(Kind::Move, items, cancel, |item| {
        move_one(item, folder, cancel, rename)
    })
}

fn move_one(item: &Path, folder: &Path, cancel: &dyn Cancel, rename: Rename) -> Turn {
    let name = shown(item);
    let Some(file_name) = item.file_name() else {
        return Turn::failed(format!("Couldn't move {name}."));
    };
    if inside(folder, item) {
        return Turn::failed(format!("Can't put {name} inside itself."));
    }
    if item
        .parent()
        .is_some_and(|parent| same_folder(parent, folder))
    {
        return Turn {
            steps: Vec::new(),
            end: End::Skipped,
        };
    }
    let from = item.to_path_buf();
    for candidate in candidates(file_name, names_like_folder(item)) {
        let to = folder.join(candidate);
        let (step, end) = match move_new(item, &to, cancel, rename) {
            Ok(Landed::Moved) => return Turn::done(Step::Moved { from, to }),
            Err(Unfinished {
                why: Halt::Taken, ..
            }) => continue,
            // There are two of it now, so what undo takes away is the copy.
            Ok(Landed::Copied) => (
                Some(created(to)),
                End::Failed(Error(format!(
                    "Copied {name}, but couldn't remove the original."
                ))),
            ),
            // The original is whole. Only what this move made itself goes on
            // record: the part of a folder that got across to another volume.
            Err(Unfinished { why, left }) => {
                let end = match why {
                    Halt::Failed(error) if left => End::Failed(Error(format!(
                        "Moved only part of {name}: {}",
                        why_failed(&error)
                    ))),
                    Halt::Failed(error) => End::Failed(Error(format!(
                        "Couldn't move {name}: {}",
                        why_failed(&error)
                    ))),
                    _ => End::Stopped,
                };
                (left.then(|| created(to)), end)
            }
        };
        let steps = step.into_iter().collect();
        return Turn { steps, end };
    }
    Turn::failed(format!("Couldn't move {name}."))
}

/// How a move got its item to the new place.
enum Landed {
    Moved,
    /// Across volumes it was copied whole, but the original couldn't be removed.
    Copied,
}

/// Moves `from` to `to`, which has to be free: a rename where that works, and
/// across volumes a copy whose original is removed only once the copy is whole.
fn move_new(
    from: &Path,
    to: &Path,
    cancel: &dyn Cancel,
    rename: Rename,
) -> Result<Landed, Unfinished> {
    match rename(from, to) {
        Ok(()) => Ok(Landed::Moved),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_new(from, to, cancel)?;
            Ok(match remove(from) {
                Ok(()) => Landed::Moved,
                Err(_) => Landed::Copied,
            })
        }
        Err(error) => Err(Unfinished::refused(error)),
    }
}

fn trash_with(items: &[PathBuf], trash_dir: &Path, rename: Rename, volume: VolumeTrash) -> Report {
    each(Kind::Trash, items, &Never, |item| {
        match trash_item(item, trash_dir, rename, volume) {
            Ok(to) => Turn::done(Step::Trashed {
                from: item.to_path_buf(),
                to,
            }),
            Err(error) => Turn {
                steps: Vec::new(),
                end: End::Failed(error),
            },
        }
    })
}

/// Moves one item into the trash and says where it went. It is only ever
/// renamed there, never copied and removed: when no trash can take it, it stays.
fn trash_item(
    item: &Path,
    trash_dir: &Path,
    rename: Rename,
    volume: VolumeTrash,
) -> Result<PathBuf, Error> {
    let name = shown(item);
    let refused = |why: String| Error(format!("Couldn't move {name} to the trash: {why}"));
    if inside(trash_dir, item) {
        return Err(Error(format!("Can't put {name} inside itself.")));
    }
    if item
        .parent()
        .is_some_and(|parent| same_folder(parent, trash_dir))
    {
        return Err(refused("It is already there.".to_string()));
    }
    // When it can't be made, the rename below says why.
    let _ = fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(trash_dir);
    match park(item, trash_dir, rename) {
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => volume(item)
            .and_then(|dir| park(item, &dir, rename))
            .map_err(|error| refused(why_failed(&error))),
        parked => parked.map_err(|error| refused(why_failed(&error))),
    }
}

/// Renames an item into a folder under its own name, or the next free one.
fn park(item: &Path, dir: &Path, rename: Rename) -> io::Result<PathBuf> {
    let name = item
        .file_name()
        .ok_or_else(|| io::Error::other("it has no name"))?;
    park_as(item, dir, candidates(name, names_like_folder(item)), rename)
}

/// Renames an item into a folder under the first of `names` that is free.
fn park_as(
    item: &Path,
    dir: &Path,
    names: impl Iterator<Item = OsString>,
    rename: Rename,
) -> io::Result<PathBuf> {
    for candidate in names {
        let to = dir.join(candidate);
        match rename(item, &to) {
            Ok(()) => return Ok(to),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

/// The trash of the volume an item is on.
fn volume_trash(item: &Path) -> io::Result<PathBuf> {
    let folder = item
        .parent()
        .ok_or_else(|| io::Error::other("it has no folder"))?;
    trash_on(&mount_point(folder)?, user_id())
}

/// A user's trash on a volume, `.Trashes/<uid>` at its top, made when missing.
fn trash_on(mount: &Path, uid: u32) -> io::Result<PathBuf> {
    let shared = mount.join(".Trashes");
    if !exists(&shared) {
        fs::create_dir(&shared)?;
        // As macOS makes it: everyone may add a folder of their own, nobody may list.
        let _ = fs::set_permissions(&shared, fs::Permissions::from_mode(0o1333));
    }
    let mine = shared.join(uid.to_string());
    match fs::DirBuilder::new().mode(0o700).create(&mine) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists || !mine.is_dir() => Err(error),
        _ => Ok(mine),
    }
}

/// The top folder of the volume a folder is on: the highest one above it that
/// is still on the same device.
fn mount_point(folder: &Path) -> io::Result<PathBuf> {
    let folder = folder.canonicalize()?;
    let device = fs::metadata(&folder)?.dev();
    let mut top = folder.as_path();
    while let Some(above) = top.parent() {
        if fs::metadata(above)?.dev() != device {
            break;
        }
        top = above;
    }
    Ok(top.to_path_buf())
}

/// The number of the user running the app.
pub fn user_id() -> u32 {
    unsafe extern "C" {
        safe fn getuid() -> u32;
    }
    getuid()
}

/// Removes a file, a folder with all it holds, or a link itself.
fn remove(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Renames, but only onto a name that is free: `AlreadyExists` otherwise,
/// with nothing touched.
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    if only_case_differs(from, to) {
        // `to` is the item itself under the name it is getting.
        return fs::rename(from, to);
    }
    if let Some(error) = clash(to, Some(from)) {
        return Err(error);
    }
    match rename_exclusive(from, to) {
        Err(error) if error.kind() == io::ErrorKind::Unsupported => rename_held(from, to),
        done => done,
    }
}

/// The rename that fails by itself when the new name exists, in one step.
#[cfg(target_os = "macos")]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::{CString, c_char, c_int, c_uint};
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn renamex_np(from: *const c_char, to: *const c_char, flags: c_uint) -> c_int;
    }
    const RENAME_EXCL: c_uint = 0x0000_0004;
    const ENOTSUP: i32 = 45;

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated strings that live until the call returns.
    if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_EXCL) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ENOTSUP) {
        // A volume that can't do it, like a FAT disk.
        return Err(io::ErrorKind::Unsupported.into());
    }
    Err(error)
}

#[cfg(not(target_os = "macos"))]
fn rename_exclusive(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// For volumes without an exclusive rename: an empty stand-in of our own
/// takes the name first, and the rename only ever replaces that.
fn rename_held(from: &Path, to: &Path) -> io::Result<()> {
    let is_dir = fs::symlink_metadata(from)?.is_dir();
    if is_dir {
        fs::create_dir(to)?;
    } else {
        OpenOptions::new().write(true).create_new(true).open(to)?;
    }
    fs::rename(from, to).inspect_err(|_| {
        // Take the stand-in away again, if that is still what is there.
        if is_dir {
            let _ = fs::remove_dir(to);
        } else if fs::symlink_metadata(to).is_ok_and(|meta| meta.is_file() && meta.len() == 0) {
            let _ = fs::remove_file(to);
        }
    })
}

/// flea's "keep both" names: the name itself, then `name copy.ext`,
/// `name copy 2.ext` and so on.
fn candidates(name: &OsStr, is_dir: bool) -> impl Iterator<Item = OsString> {
    let plain = name.to_os_string();
    let text = name.to_string_lossy().into_owned();
    (0..TRIES).map(move |n| {
        let (stem, extension) = split_name(&text, is_dir);
        match n {
            0 => plain.clone(),
            1 => format!("{stem} copy{extension}").into(),
            n => format!("{stem} copy {n}{extension}").into(),
        }
    })
}

/// Whether an item is named like a folder, with no extension to keep at the
/// end. Application bundles are folders named like files.
fn names_like_folder(item: &Path) -> bool {
    let is_app = item
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"));
    !is_app && fs::symlink_metadata(item).is_ok_and(|meta| meta.is_dir())
}

/// Whether `folder` is the folder `item` or somewhere inside it.
fn inside(folder: &Path, item: &Path) -> bool {
    if !fs::symlink_metadata(item).is_ok_and(|meta| meta.is_dir()) {
        return false;
    }
    match (folder.canonicalize(), item.canonicalize()) {
        (Ok(folder), Ok(item)) => folder.starts_with(item),
        _ => folder.starts_with(item),
    }
}

/// Whether an item is directly in a folder.
fn in_folder(item: &Path, folder: &Path) -> bool {
    item.parent()
        .is_some_and(|parent| same_folder(parent, folder))
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Whether two names in one folder are the same item, spelled in another
/// letter case: what a rename from `notes` to `Notes` looks like on a Mac.
fn only_case_differs(a: &Path, b: &Path) -> bool {
    let (Some(name_a), Some(name_b)) = (a.file_name(), b.file_name()) else {
        return false;
    };
    let lower = |name: &OsStr| name.to_string_lossy().to_lowercase();
    let same_item = match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    };
    same_item && a.parent() == b.parent() && name_a != name_b && lower(name_a) == lower(name_b)
}

/// Whether two paths name one item: the same path, or, on a volume that
/// ignores case, two spellings of a name that lead to the same item.
fn same_entry(a: &Path, b: &Path) -> bool {
    a == b
        || match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
}

/// Whether anything is at a path, a link that leads nowhere included.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// An item's name as it is shown.
fn shown(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

/// The name of the folder an item is in.
fn folder_name(path: &Path) -> String {
    path.parent().map(shown).unwrap_or_default()
}

/// Why something failed, as a sentence.
fn why_failed(error: &io::Error) -> String {
    if is_clash(error) {
        // It starts with a file's name, which keeps its small letters.
        return format!("{error}.");
    }
    match error.kind() {
        io::ErrorKind::PermissionDenied => "Permission was refused.".to_string(),
        io::ErrorKind::NotFound => "It isn't there any more.".to_string(),
        io::ErrorKind::StorageFull => "The disk is full.".to_string(),
        io::ErrorKind::ReadOnlyFilesystem => "The disk can't be written to.".to_string(),
        _ => {
            let text = error.to_string();
            let text = text.split(" (os error").next().unwrap_or_default();
            let mut letters = text.chars();
            match letters.next() {
                Some(first) => format!("{}{}.", first.to_uppercase(), letters.as_str()),
                None => "Something went wrong.".to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

    struct World {
        root: PathBuf,
        trash: PathBuf,
        _dir: tempfile::TempDir,
    }

    /// An empty folder to work in, holding a trash of its own.
    fn world() -> World {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let trash = root.join(".Trash");
        fs::create_dir(&trash).unwrap();
        World {
            root,
            trash,
            _dir: dir,
        }
    }

    impl World {
        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        /// Writes a file, making the folders above it.
        fn file(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.path(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        fn folder(&self, relative: &str) -> PathBuf {
            let path = self.path(relative);
            fs::create_dir_all(&path).unwrap();
            path
        }

        fn read(&self, relative: &str) -> String {
            fs::read_to_string(self.path(relative)).unwrap()
        }

        fn names(&self, relative: &str) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.path(relative))
                .unwrap()
                .map(|item| item.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        fn has(&self, relative: &str) -> bool {
            fs::symlink_metadata(self.path(relative)).is_ok()
        }
    }

    fn go() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// Cancels as soon as something is true of the world.
    struct When<F: Fn() -> bool>(F);

    impl<F: Fn() -> bool> Cancel for When<F> {
        fn cancelled(&self) -> bool {
            (self.0)()
        }
    }

    /// A rename that treats `volume` as another disk: nothing is renamed across its edge.
    fn crossing(volume: PathBuf) -> impl Fn(&Path, &Path) -> io::Result<()> {
        move |from, to| {
            if from.starts_with(&volume) == to.starts_with(&volume) {
                rename_new(from, to)
            } else {
                Err(io::ErrorKind::CrossesDevices.into())
            }
        }
    }

    fn said(report: &Report) -> Option<String> {
        report.error.as_ref().map(Error::to_string)
    }

    fn created(at: PathBuf) -> Step {
        Step::Created { at, parked: None }
    }

    #[test]
    fn copying_puts_a_copy_in_the_folder_and_leaves_the_original() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let into = w.folder("Into");
        let report = copy_into(&[a], &into, &go());
        assert_eq!(w.read("Into/a.txt"), "alpha");
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!((report.asked, report.done, report.stopped), (1, 1, false));
        assert_eq!(report.error, None);
        assert_eq!(report.change.kind, Kind::Copy);
        assert_eq!(report.change.steps, [created(into.join("a.txt"))]);
        assert_eq!(report.change.landed(), [into.join("a.txt")]);
    }

    #[test]
    fn a_taken_name_keeps_both_and_nothing_is_overwritten() {
        let w = world();
        let items = [
            w.file("a.txt", "new"),
            w.file(".env", "new env"),
            w.file("README", "new readme"),
            w.file("My.Stuff/inside.txt", "new inside")
                .parent()
                .unwrap()
                .to_path_buf(),
        ];
        w.file("Into/a.txt", "old");
        w.file("Into/.env", "old env");
        w.file("Into/README", "old readme");
        w.file("Into/My.Stuff/kept.txt", "old inside");
        for _ in 0..3 {
            let report = copy_into(&items, &w.path("Into"), &go());
            assert_eq!((report.done, said(&report)), (4, None));
        }

        assert_eq!(w.names("Into").len(), 16, "{:?}", w.names("Into"));
        for (old, text) in [
            ("a.txt", "old"),
            (".env", "old env"),
            ("README", "old readme"),
            ("My.Stuff/kept.txt", "old inside"),
        ] {
            assert_eq!(
                w.read(&format!("Into/{old}")),
                text,
                "{old} was there first"
            );
        }
        assert_eq!(w.names("Into/My.Stuff"), ["kept.txt"]);
        for (copy, text) in [
            ("a copy.txt", "new"),
            ("a copy 2.txt", "new"),
            ("a copy 3.txt", "new"),
            (".env copy", "new env"),
            (".env copy 3", "new env"),
            ("README copy", "new readme"),
            ("README copy 2", "new readme"),
            // A folder has no extension to keep at the end.
            ("My.Stuff copy/inside.txt", "new inside"),
            ("My.Stuff copy 3/inside.txt", "new inside"),
        ] {
            assert_eq!(w.read(&format!("Into/{copy}")), text, "{copy}");
        }
    }

    #[test]
    fn folders_are_copied_whole_and_links_stay_links() {
        let w = world();
        w.file("Album/cover.jpg", "jpeg");
        w.file("Album/Disc 1/track.txt", "song");
        let script = w.file("Album/run.sh", "#!/bin/sh");
        fs::set_permissions(&script, Permissions::from_mode(0o750)).unwrap();
        w.file("elsewhere.txt", "pointed at");
        symlink(w.path("elsewhere.txt"), w.path("Album/link")).unwrap();
        symlink("nowhere", w.path("Album/broken")).unwrap();
        symlink(w.path("Album"), w.path("shortcut")).unwrap();
        let into = w.folder("Into");

        let report = copy_into(&[w.path("Album"), w.path("shortcut")], &into, &go());
        assert_eq!((report.done, said(&report)), (2, None));
        assert_eq!(
            report.change.steps,
            [created(into.join("Album")), created(into.join("shortcut"))]
        );
        assert_eq!(w.read("Into/Album/cover.jpg"), "jpeg");
        assert_eq!(w.read("Into/Album/Disc 1/track.txt"), "song");
        assert_eq!(w.read("Into/Album/run.sh"), "#!/bin/sh");
        let mode = fs::metadata(w.path("Into/Album/run.sh")).unwrap();
        assert_eq!(mode.permissions().mode() & 0o777, 0o750);
        assert_eq!(
            fs::read_link(w.path("Into/Album/link")).unwrap(),
            w.path("elsewhere.txt")
        );
        assert_eq!(
            fs::read_link(w.path("Into/Album/broken")).unwrap(),
            Path::new("nowhere")
        );
        // A link picked by itself is copied as the link, not as the folder behind it.
        assert_eq!(
            fs::read_link(w.path("Into/shortcut")).unwrap(),
            w.path("Album")
        );
        assert_eq!(w.names("Album").len(), 5, "the original is as it was");
    }

    #[test]
    fn a_folder_is_never_put_inside_itself_or_its_own_folders() {
        let w = world();
        w.file("Box/Deep/a.txt", "a");
        symlink(w.path("Box/Deep"), w.path("door")).unwrap();
        // The folder itself, one inside it, and one inside it reached through a link.
        for into in ["Box", "Box/Deep", "door"] {
            let copied = copy_into(&[w.path("Box")], &w.path(into), &go());
            let moved = move_into(&[w.path("Box")], &w.path(into), &go());
            for report in [copied, moved] {
                assert_eq!(
                    said(&report).as_deref(),
                    Some("Can't put Box inside itself."),
                    "{into}"
                );
                assert!(report.change.steps.is_empty());
                assert_eq!(report.done, 0);
            }
        }
        assert_eq!(w.names("Box"), ["Deep"]);
        assert_eq!(w.names("Box/Deep"), ["a.txt"]);
        assert_eq!(w.read("Box/Deep/a.txt"), "a");
    }

    #[test]
    fn moving_takes_the_items_to_the_folder() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("Dir/b.txt", "beta");
        let (dir, into) = (w.path("Dir"), w.folder("Into"));
        let report = move_into(&[a.clone(), dir.clone()], &into, &go());
        assert_eq!((report.done, said(&report)), (2, None));
        assert_eq!(report.change.kind, Kind::Move);
        assert_eq!(
            report.change.steps,
            [
                Step::Moved {
                    from: a,
                    to: into.join("a.txt")
                },
                Step::Moved {
                    from: dir,
                    to: into.join("Dir")
                }
            ]
        );
        assert_eq!(w.read("Into/a.txt"), "alpha");
        assert_eq!(w.read("Into/Dir/b.txt"), "beta");
        assert_eq!(w.names(""), [".Trash", "Into"]);
    }

    #[test]
    fn moving_onto_a_taken_name_keeps_both() {
        let w = world();
        let a = w.file("a.txt", "new");
        w.file("Dir/y.txt", "new");
        w.file("Into/a.txt", "old");
        w.file("Into/Dir/x.txt", "old");
        let into = w.path("Into");
        let report = move_into(&[a.clone(), w.path("Dir")], &into, &go());
        assert_eq!((report.done, said(&report)), (2, None));
        assert_eq!(
            report.change.steps[0],
            Step::Moved {
                from: a,
                to: into.join("a copy.txt")
            }
        );
        assert_eq!(w.read("Into/a.txt"), "old");
        assert_eq!(w.read("Into/a copy.txt"), "new");
        assert_eq!(w.names("Into/Dir"), ["x.txt"]);
        assert_eq!(w.read("Into/Dir/x.txt"), "old");
        assert_eq!(w.read("Into/Dir copy/y.txt"), "new");
        assert!(!w.has("a.txt") && !w.has("Dir"));
    }

    #[test]
    fn moving_something_to_the_folder_it_is_in_does_nothing() {
        let w = world();
        let a = w.file("Here/a.txt", "alpha");
        let report = move_into(&[a], &w.path("Here"), &go());
        assert_eq!((report.done, report.skipped, said(&report)), (0, 1, None));
        assert!(report.change.steps.is_empty());
        assert_eq!(w.names("Here"), ["a.txt"]);
        assert_eq!(w.read("Here/a.txt"), "alpha");
    }

    #[test]
    fn moving_to_another_volume_copies_then_removes_the_original() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("Dir/b.txt", "beta");
        let usb = w.folder("USB");
        w.file("USB/a.txt", "already there");
        let report = move_with(
            &[a.clone(), w.path("Dir")],
            &usb,
            &go(),
            &crossing(usb.clone()),
        );
        assert_eq!((report.done, said(&report)), (2, None));
        assert_eq!(
            report.change.steps,
            [
                Step::Moved {
                    from: a,
                    to: usb.join("a copy.txt")
                },
                Step::Moved {
                    from: w.path("Dir"),
                    to: usb.join("Dir")
                }
            ]
        );
        assert_eq!(w.read("USB/a.txt"), "already there");
        assert_eq!(w.read("USB/a copy.txt"), "alpha");
        assert_eq!(w.read("USB/Dir/b.txt"), "beta");
        assert!(!w.has("a.txt") && !w.has("Dir"));
    }

    #[test]
    fn a_move_to_another_volume_stopped_halfway_leaves_the_original_whole() {
        let w = world();
        for name in ["1.txt", "2.txt", "3.txt"] {
            w.file(&format!("Big/{name}"), name);
        }
        let usb = w.folder("USB");
        let arrived = usb.join("Big");
        let first_file_is_over = When(|| fs::read_dir(&arrived).is_ok_and(|dir| dir.count() > 0));
        let report = move_with(
            &[w.path("Big")],
            &usb,
            &first_file_is_over,
            &crossing(usb.clone()),
        );
        assert!(report.stopped);
        assert_eq!((report.done, said(&report)), (0, None));
        assert_eq!(w.names("Big"), ["1.txt", "2.txt", "3.txt"]);
        assert_eq!(w.read("Big/2.txt"), "2.txt");
        // The part that got across is on record, so undo can take it away again.
        assert_eq!(w.names("USB/Big").len(), 1);
        assert_eq!(report.change.steps, [created(arrived)]);
    }

    #[test]
    fn an_original_that_cannot_be_removed_after_the_copy_is_left_and_reported() {
        let w = world();
        let a = w.file("Locked/a.txt", "alpha");
        let usb = w.folder("USB");
        fs::set_permissions(w.path("Locked"), Permissions::from_mode(0o555)).unwrap();
        let report = move_with(&[a], &usb, &go(), &crossing(usb.clone()));
        fs::set_permissions(w.path("Locked"), Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            said(&report).as_deref(),
            Some("Copied a.txt, but couldn't remove the original.")
        );
        assert_eq!(w.read("Locked/a.txt"), "alpha");
        assert_eq!(w.read("USB/a.txt"), "alpha");
        assert_eq!(report.change.steps, [created(usb.join("a.txt"))]);
    }

    #[test]
    fn duplicating_makes_a_copy_next_to_the_original() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("Dir/in.txt", "inside");
        let items = [a, w.path("Dir")];
        let report = duplicate(&items, &go());
        assert_eq!((report.done, said(&report)), (2, None));
        assert_eq!(report.change.kind, Kind::Duplicate);
        assert_eq!(
            report.change.steps,
            [created(w.path("a copy.txt")), created(w.path("Dir copy"))]
        );
        assert_eq!(w.read("a copy.txt"), "alpha");
        assert_eq!(w.read("Dir copy/in.txt"), "inside");
        duplicate(&items, &go());
        assert_eq!(w.read("a copy 2.txt"), "alpha");
        assert_eq!(w.read("Dir copy 2/in.txt"), "inside");
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!(w.names("Dir"), ["in.txt"]);
    }

    #[test]
    fn copying_stopped_halfway_keeps_what_was_done_on_record() {
        let w = world();
        let items = [
            w.file("1.txt", "one"),
            w.file("2.txt", "two"),
            w.file("3.txt", "three"),
        ];
        let into = w.folder("Into");
        let second_is_over = When(|| into.join("2.txt").exists());
        let report = copy_into(&items, &into, &second_is_over);
        assert!(report.stopped);
        assert_eq!((report.asked, report.done, said(&report)), (3, 2, None));
        assert_eq!(w.names("Into"), ["1.txt", "2.txt"]);
        assert_eq!(w.read("Into/2.txt"), "two");
        assert_eq!(
            report.change.steps,
            [created(into.join("1.txt")), created(into.join("2.txt"))]
        );

        // Undo takes back what was copied before the stop, into the trash.
        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap().unwrap();
        assert!(w.names("Into").is_empty());
        assert_eq!(w.read(".Trash/1.txt"), "one");
        assert_eq!(w.read(".Trash/2.txt"), "two");
    }

    #[test]
    fn work_that_ends_before_it_took_a_name_leaves_what_has_that_name_alone() {
        let w = world();
        let a = w.file("a.txt", "new");
        w.file("Dir/b.txt", "new");
        w.file("Into/a.txt", "old");
        w.file("Into/Dir/kept.txt", "old");
        w.file("Into/gone.txt", "old");
        let into = w.path("Into");
        let items = [a, w.path("Dir")];

        // Stopped between the look at the item and the taking of its name.
        for item in &items {
            let looks = std::cell::Cell::new(0);
            let after_the_first_look = When(|| looks.replace(looks.get() + 1) > 0);
            let copied = copy_into(std::slice::from_ref(item), &into, &after_the_first_look);
            looks.set(0);
            let moved = move_with(
                std::slice::from_ref(item),
                &into,
                &after_the_first_look,
                &crossing(into.clone()),
            );
            for report in [copied, moved] {
                assert!(report.stopped);
                assert_eq!(report.change.steps, [], "nothing there is ours to undo");
            }
        }
        // An item that can't be read: it fails before any name is taken.
        let failed = copy_into(&[w.path("gone.txt")], &into, &go());
        assert_eq!(said(&failed).as_deref(), Some("Couldn't copy gone.txt."));
        assert_eq!(failed.change.steps, []);

        assert_eq!(w.names("Into"), ["Dir", "a.txt", "gone.txt"]);
        assert_eq!(w.read("Into/a.txt"), "old");
        assert_eq!(w.names("Into/Dir"), ["kept.txt"]);
        assert_eq!(w.read("a.txt"), "new");
        assert_eq!(w.names("Dir"), ["b.txt"]);
    }

    /// How the `._name` file starts that macOS keeps next to `name` on a volume
    /// with no extended attributes of its own, like a FAT disk.
    const APPLE_DOUBLE: &[u8] = b"\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        ";

    thread_local! {
        /// The folders this test pretends are disks that keep companion files.
        static PRETEND_FAT: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Whether a folder is on one of the pretend FAT disks of the test under way.
    pub(super) fn is_pretend_fat(folder: &Path) -> bool {
        PRETEND_FAT.with(|disks| disks.borrow().iter().any(|disk| folder.starts_with(disk)))
    }

    /// Stands in for a FAT volume mounted at a folder. Every item that arrives
    /// anywhere on it gets a `._name` companion that this code never asked
    /// for, written over whatever has that name unless it is a companion
    /// already: what macOS was seen to do there. It rides on the cancel check,
    /// which runs before each item is copied, so the companion of one item is
    /// there before the next; `catch_up` does the same after the last one.
    struct Fat {
        root: PathBuf,
        seen: std::cell::RefCell<Vec<PathBuf>>,
    }

    impl Fat {
        /// What is on the disk already keeps the companion it has, or none.
        fn at(root: PathBuf) -> Self {
            PRETEND_FAT.with(|disks| disks.borrow_mut().push(root.clone()));
            let fat = Self {
                seen: Default::default(),
                root,
            };
            fat.arrivals(&fat.root);
            fat
        }

        /// The items in a folder and below that weren't there the last time.
        fn arrivals(&self, folder: &Path) -> Vec<PathBuf> {
            let mut arrived = Vec::new();
            for item in fs::read_dir(folder).into_iter().flatten().flatten() {
                let path = item.path();
                let is_companion = item.file_name().as_encoded_bytes().starts_with(b"._");
                if !is_companion && !self.seen.borrow().contains(&path) {
                    self.seen.borrow_mut().push(path.clone());
                    arrived.push(path.clone());
                }
                if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                    arrived.extend(self.arrivals(&path));
                }
            }
            arrived
        }

        /// The system gets round to the items that arrived since the last time.
        fn catch_up(&self) {
            for item in self.arrivals(&self.root) {
                let mut name = OsString::from("._");
                name.push(item.file_name().unwrap());
                let companion = item.with_file_name(name);
                if !is_apple_double(&companion) {
                    fs::write(companion, APPLE_DOUBLE).unwrap();
                }
            }
        }
    }

    impl Cancel for Fat {
        fn cancelled(&self) -> bool {
            self.catch_up();
            false
        }
    }

    fn os(names: &[&str]) -> Vec<OsString> {
        names.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_folder_copies_whole_where_the_system_keeps_companion_files() {
        let w = world();
        w.file("Album/one.txt", "first");
        w.file("Album/two.txt", "second");
        w.folder("Album/Disc");
        for companion in ["._one.txt", "._two.txt", "._Disc"] {
            fs::write(w.path("Album").join(companion), APPLE_DOUBLE).unwrap();
        }
        // Whichever of an item and its companion the folder lists first.
        let orders = [
            (
                "Item first",
                [
                    "one.txt",
                    "._one.txt",
                    "Disc",
                    "._Disc",
                    "two.txt",
                    "._two.txt",
                ],
            ),
            (
                "Companion first",
                [
                    "._one.txt",
                    "one.txt",
                    "._Disc",
                    "Disc",
                    "._two.txt",
                    "two.txt",
                ],
            ),
        ];
        for (to, order) in orders {
            let dest = w.folder(to);
            let copied =
                copy_children(&w.path("Album"), &dest, &os(&order), &Fat::at(dest.clone()));
            assert!(copied.is_ok(), "{to}");
            assert_eq!(
                w.names(to),
                [
                    "._Disc",
                    "._one.txt",
                    "._two.txt",
                    "Disc",
                    "one.txt",
                    "two.txt"
                ]
            );
            assert_eq!(w.read(&format!("{to}/one.txt")), "first");
            assert_eq!(w.read(&format!("{to}/two.txt")), "second");
        }

        // The same through a whole command, in the order the folder really lists.
        let report = duplicate(&[w.path("Album")], &Fat::at(w.path("Album copy")));
        assert_eq!((report.done, said(&report)), (1, None));
        assert_eq!(w.names("Album copy"), w.names("Album"));
        assert_eq!(w.read("Album copy/one.txt"), "first");
        assert_eq!(w.read("Album copy/two.txt"), "second");
        assert_eq!(w.names("Album").len(), 6, "the original is as it was");
    }

    #[test]
    fn a_dot_underscore_file_is_copied_like_any_other_where_the_system_adds_none() {
        let w = world();
        w.file("Box/x.txt", "x");
        w.file("Box/._x.txt", "my own notes");
        w.file("Box/._alone", "no item of that name");
        fs::write(w.path("Box/._y"), APPLE_DOUBLE).unwrap();
        w.file("Box/y", "y");
        let into = w.folder("Into");
        let report = copy_into(&[w.path("Box")], &into, &go());
        assert_eq!((report.done, said(&report)), (1, None));
        assert_eq!(
            w.names("Into/Box"),
            ["._alone", "._x.txt", "._y", "x.txt", "y"]
        );
        assert_eq!(w.read("Into/Box/._x.txt"), "my own notes");
        assert_eq!(w.read("Into/Box/._alone"), "no item of that name");
        assert_eq!(fs::read(w.path("Into/Box/._y")).unwrap(), APPLE_DOUBLE);
    }

    /// What the app says when a file of the user's is where the system
    /// would write a companion.
    fn side_by_side(does: &str, item: &str, name: &str) -> Option<String> {
        Some(format!(
            "{does} {item}: {name} and ._{name} can't be side by side on this disk."
        ))
    }

    #[test]
    fn a_file_of_the_users_own_is_never_given_up_for_a_companion() {
        let w = world();
        w.file("Box/x.txt", "x");
        w.file("Box/._x.txt", "my own notes");
        let notes = |path: PathBuf| fs::read(path).is_ok_and(|bytes| bytes == b"my own notes");
        // Where the system keeps companions, `._x.txt` next to `x.txt` is the
        // system's to write. Whichever of the two the folder lists first, the
        // copy says so instead of letting the user's file be written over.
        for (to, order) in [
            ("Item first", ["x.txt", "._x.txt"]),
            ("Notes first", ["._x.txt", "x.txt"]),
        ] {
            let dest = w.folder(to);
            let fat = Fat::at(dest.clone());
            let copied = copy_children(&w.path("Box"), &dest, &os(&order), &fat);
            fat.catch_up();
            assert!(copied.is_err(), "{to}");
            // What did arrive is whole: the notes, or `x.txt` with its companion.
            let arrived = w.names(to);
            assert!(
                (arrived == ["._x.txt"] && notes(dest.join("._x.txt")))
                    || (arrived == ["._x.txt", "x.txt"] && is_apple_double(&dest.join("._x.txt"))),
                "{to}: {arrived:?}"
            );
        }

        // A whole copy, in the order the folder really lists: refused out
        // loud, the original untouched, the part that arrived on record.
        let usb = w.folder("USB");
        let fat = Fat::at(usb.clone());
        let report = copy_into(&[w.path("Box")], &usb, &fat);
        fat.catch_up();
        assert_eq!(said(&report), side_by_side("Couldn't copy", "Box", "x.txt"));
        assert_eq!(report.done, 0);
        assert_eq!(report.change.steps, [created(usb.join("Box"))]);
        assert!(notes(w.path("Box/._x.txt")));
        assert_eq!(w.read("Box/x.txt"), "x");

        // A move: the original stays whole where it was.
        w.file("Other/Box/x.txt", "x");
        w.file("Other/Box/._x.txt", "my own notes");
        let report = move_with(&[w.path("Other/Box")], &usb, &fat, &crossing(usb.clone()));
        fat.catch_up();
        assert_eq!(
            said(&report),
            side_by_side("Moved only part of", "Box", "x.txt")
        );
        assert_eq!(report.change.steps, [created(usb.join("Box copy"))]);
        assert_eq!(w.names("Other/Box"), ["._x.txt", "x.txt"]);
        assert!(notes(w.path("Other/Box/._x.txt")));
        assert_eq!(w.read("Other/Box/x.txt"), "x");
    }

    #[test]
    fn nothing_is_put_where_the_system_would_write_over_a_file_of_the_users() {
        let w = world();
        let usb = w.folder("USB");
        w.file("USB/Sub/._y.txt", "notes about y");
        w.file("USB/Sub/._z.txt", "notes about z");
        w.file("USB/Sub/._Plans", "notes about plans");
        w.file("USB/Sub/._b.txt", "notes about b");
        w.file("USB/Sub/q.txt", "q");
        w.file("USB/z.txt", "z");
        w.file("USB/Sub/a.txt", "a");
        let y = w.file("y.txt", "y");
        let own = w.file("._q.txt", "notes about q");
        let sub = usb.join("Sub");
        let fat = Fat::at(usb.clone());

        // Copied in next to the user's `._y.txt`.
        let copied = copy_into(&[y], &sub, &fat);
        assert_eq!(
            said(&copied),
            side_by_side("Couldn't copy", "y.txt", "y.txt")
        );
        // Moved in from the same disk, which is a rename.
        let moved = move_with(&[usb.join("z.txt")], &sub, &fat, &crossing(usb.clone()));
        assert_eq!(
            said(&moved),
            side_by_side("Couldn't move", "z.txt", "z.txt")
        );
        // A folder made there, and an item renamed to the name.
        let made = new_folder(&sub, "Plans").unwrap_err().to_string();
        assert_eq!(
            Some(made),
            side_by_side("Couldn't create", "Plans", "Plans")
        );
        let renamed = rename(&sub.join("a.txt"), "b.txt").unwrap_err().to_string();
        assert_eq!(
            Some(renamed),
            side_by_side("Couldn't rename", "a.txt", "b.txt")
        );
        // The other way round: the user's `._q.txt` brought in next to `q.txt`.
        let brought = copy_into(&[own], &sub, &fat);
        assert_eq!(
            said(&brought),
            side_by_side("Couldn't copy", "._q.txt", "q.txt")
        );

        fat.catch_up();
        for report in [copied, moved, brought] {
            assert_eq!(report.change.steps, [], "nothing was made");
        }
        assert_eq!(
            w.names("USB/Sub"),
            ["._Plans", "._b.txt", "._y.txt", "._z.txt", "a.txt", "q.txt"]
        );
        for (file, text) in [
            ("._y.txt", "y"),
            ("._z.txt", "z"),
            ("._Plans", "plans"),
            ("._b.txt", "b"),
        ] {
            assert_eq!(
                w.read(&format!("USB/Sub/{file}")),
                format!("notes about {text}")
            );
        }
        assert_eq!(w.read("USB/z.txt"), "z", "what was to move is where it was");
        assert_eq!(w.read("._q.txt"), "notes about q");
        assert_eq!(w.read("y.txt"), "y");
    }

    #[test]
    fn on_a_disk_with_attributes_of_its_own_such_names_sit_side_by_side() {
        let w = world();
        w.file("Sub/._y.txt", "notes about y");
        w.file("Sub/._Plans", "notes about plans");
        w.file("Sub/q.txt", "q");
        let items = [w.file("y.txt", "y"), w.file("._q.txt", "notes about q")];
        let report = copy_into(&items, &w.path("Sub"), &go());
        assert_eq!((report.done, said(&report)), (2, None));
        new_folder(&w.path("Sub"), "Plans").unwrap().unwrap();
        rename(&w.path("Sub/y.txt"), "Plans.txt").unwrap().unwrap();
        assert_eq!(
            w.names("Sub"),
            [
                "._Plans",
                "._q.txt",
                "._y.txt",
                "Plans",
                "Plans.txt",
                "q.txt"
            ]
        );
        assert_eq!(w.read("Sub/._y.txt"), "notes about y");
        assert_eq!(w.read("Sub/._Plans"), "notes about plans");
        assert_eq!(w.read("Sub/._q.txt"), "notes about q");
        // The temp folder is on such a disk, and the question has an answer.
        assert!(!keeps_companions(&w.root));
    }

    #[test]
    fn a_copy_that_fails_halfway_leaves_nothing_that_is_not_on_record() {
        let w = world();
        w.file("Dir/a.txt", "a");
        let locked = w.file("Dir/b.txt", "b");
        fs::set_permissions(&locked, Permissions::from_mode(0o000)).unwrap();
        let into = w.folder("Into");
        let report = copy_into(&[w.path("Dir")], &into, &go());
        assert_eq!(said(&report).as_deref(), Some("Couldn't copy Dir."));
        assert_eq!(report.change.steps, [created(into.join("Dir"))]);
        assert!(!w.has("Into/Dir/b.txt"), "no half file");
        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap().unwrap();
        assert!(w.names("Into").is_empty(), "undo takes the part away");
        assert!(w.has(".Trash/Dir"), "to the trash");

        // The same failure while moving back across volumes, as undo and redo
        // do: the part that arrived is taken away again, and the item stays
        // whole where it was.
        let usb = w.folder("USB");
        fs::rename(w.path("Dir"), usb.join("Dir")).unwrap();
        let back = w.path("Dir");
        let put = put_with(&usb.join("Dir"), &back, &w.trash, &crossing(usb.clone()));
        fs::set_permissions(usb.join("Dir/b.txt"), Permissions::from_mode(0o644)).unwrap();
        assert!(put.is_err());
        assert!(!exists(&back), "no part of it is left behind");
        assert_eq!(w.names("USB/Dir"), ["a.txt", "b.txt"]);
        assert_eq!(w.read("USB/Dir/b.txt"), "b");
        // Taken away to the trash, next to the one from before.
        assert_eq!(w.names(".Trash"), ["Dir", "Dir copy"]);
    }

    #[test]
    fn a_folder_copy_stopped_halfway_stays_and_can_be_undone() {
        let w = world();
        for name in ["1.txt", "2.txt", "3.txt"] {
            w.file(&format!("Big/{name}"), name);
        }
        let into = w.folder("Into");
        let arrived = into.join("Big");
        let first_file_is_over = When(|| fs::read_dir(&arrived).is_ok_and(|dir| dir.count() > 0));
        let report = copy_into(&[w.path("Big")], &into, &first_file_is_over);
        assert!(report.stopped);
        assert_eq!(report.done, 0, "the folder isn't whole");
        let part = w.names("Into/Big");
        assert_eq!(part.len(), 1);
        assert_eq!(w.read(&format!("Into/Big/{}", part[0])), part[0]);
        assert_eq!(report.change.steps, [created(arrived)]);

        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap().unwrap();
        assert!(!w.has("Into/Big"));
        assert_eq!(w.names(".Trash/Big"), part, "moved away, not deleted");
        assert_eq!(w.names("Big").len(), 3);
    }

    #[test]
    fn renaming_gives_the_item_its_new_name() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let change = rename(&a, "b.md").unwrap().unwrap();
        assert_eq!(change.kind, Kind::Rename);
        assert_eq!(
            change.steps,
            [Step::Moved {
                from: a.clone(),
                to: w.path("b.md")
            }]
        );
        assert_eq!(w.read("b.md"), "alpha");
        assert!(!w.has("a.txt"));
    }

    #[test]
    fn an_empty_or_unchanged_name_renames_nothing() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        for name in ["", "   ", "a.txt"] {
            assert_eq!(rename(&a, name), Ok(None), "{name:?}");
        }
        assert_eq!(w.names(""), [".Trash", "a.txt"]);
        assert_eq!(w.read("a.txt"), "alpha");
    }

    #[test]
    fn a_name_with_a_slash_or_one_that_is_taken_is_refused() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("b.txt", "beta");
        w.file("Dir/c.txt", "gamma");
        let slash = rename(&a, "Dir/a.txt").unwrap_err();
        assert_eq!(slash.to_string(), "A name cannot contain /.");
        for taken in ["b.txt", "Dir"] {
            let error = rename(&a, taken).unwrap_err();
            assert_eq!(error.to_string(), format!("{taken} already exists here."));
        }
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!(w.read("b.txt"), "beta");
        assert_eq!(w.names("Dir"), ["c.txt"]);
    }

    #[test]
    fn a_rename_can_change_only_the_letter_case() {
        let w = world();
        let a = w.file("notes.txt", "alpha");
        let change = rename(&a, "Notes.txt").unwrap().unwrap();
        assert_eq!(w.names(""), [".Trash", "Notes.txt"]);
        assert_eq!(w.read("Notes.txt"), "alpha");
        let mut history = History::default();
        history.record(change);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(w.names(""), [".Trash", "notes.txt"]);
    }

    #[test]
    fn a_new_folder_is_made_unless_the_name_is_taken() {
        let w = world();
        let change = new_folder(&w.root, "Plans").unwrap().unwrap();
        assert_eq!(change.kind, Kind::NewFolder);
        assert_eq!(change.steps, [created(w.path("Plans"))]);
        assert!(w.path("Plans").is_dir());

        w.file("Plans/kept.txt", "kept");
        w.file("taken.txt", "kept");
        for taken in ["Plans", "taken.txt"] {
            let error = new_folder(&w.root, taken).unwrap_err();
            assert_eq!(error.to_string(), format!("{taken} already exists here."));
        }
        let slash = new_folder(&w.root, "a/b").unwrap_err();
        assert_eq!(slash.to_string(), "A name cannot contain /.");
        assert_eq!(new_folder(&w.root, "  "), Ok(None));
        assert_eq!(w.names(""), [".Trash", "Plans", "taken.txt"]);
        assert_eq!(w.read("Plans/kept.txt"), "kept");
        assert_eq!(w.read("taken.txt"), "kept");
    }

    #[test]
    fn trashing_moves_the_items_into_the_trash_under_their_own_names() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("Dir/b.txt", "beta");
        let report = trash(&[a.clone(), w.path("Dir")], &w.trash);
        assert_eq!((report.done, said(&report)), (2, None));
        assert_eq!(report.change.kind, Kind::Trash);
        assert_eq!(
            report.change.steps,
            [
                Step::Trashed {
                    from: a,
                    to: w.trash.join("a.txt")
                },
                Step::Trashed {
                    from: w.path("Dir"),
                    to: w.trash.join("Dir")
                }
            ]
        );
        assert_eq!(w.read(".Trash/a.txt"), "alpha");
        assert_eq!(w.read(".Trash/Dir/b.txt"), "beta");
        assert_eq!(w.names(""), [".Trash"]);
    }

    #[test]
    fn a_name_taken_in_the_trash_gets_a_free_one_and_what_was_there_stays() {
        let w = world();
        w.file(".Trash/a.txt", "thrown away earlier");
        let a = w.file("a.txt", "today");
        let report = trash(std::slice::from_ref(&a), &w.trash);
        assert_eq!(
            report.change.steps,
            [Step::Trashed {
                from: a,
                to: w.trash.join("a copy.txt")
            }]
        );
        assert_eq!(w.read(".Trash/a.txt"), "thrown away earlier");
        assert_eq!(w.read(".Trash/a copy.txt"), "today");
    }

    #[test]
    fn a_trash_folder_that_is_missing_is_made() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let trash_dir = w.path("fresh/.Trash");
        let report = trash(&[a], &trash_dir);
        assert_eq!((report.done, said(&report)), (1, None));
        assert_eq!(w.read("fresh/.Trash/a.txt"), "alpha");
    }

    #[test]
    fn an_item_that_cannot_be_trashed_stays_where_it_is() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        fs::set_permissions(&w.trash, Permissions::from_mode(0o555)).unwrap();
        let report = trash(&[a], &w.trash);
        fs::set_permissions(&w.trash, Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            said(&report).as_deref(),
            Some("Couldn't move a.txt to the trash: Permission was refused.")
        );
        assert!(report.change.steps.is_empty());
        assert_eq!(w.read("a.txt"), "alpha");
        assert!(w.names(".Trash").is_empty());
    }

    #[test]
    fn an_item_on_another_volume_goes_to_that_volumes_trash() {
        let w = world();
        let usb = w.folder("USB");
        let a = w.file("USB/a.txt", "alpha");
        let items = std::slice::from_ref(&a);
        let report = trash_with(items, &w.trash, &crossing(usb.clone()), &|_| {
            trash_on(&usb, 501)
        });
        let to = usb.join(".Trashes/501/a.txt");
        assert_eq!(said(&report), None);
        assert_eq!(report.change.steps, [Step::Trashed { from: a, to }]);
        assert_eq!(w.read("USB/.Trashes/501/a.txt"), "alpha");
        assert!(!w.has("USB/a.txt"));
        assert!(w.names(".Trash").is_empty());

        // The volume's trash is used again once it is there.
        let b = w.file("USB/b.txt", "beta");
        trash_with(&[b], &w.trash, &crossing(usb.clone()), &|_| {
            trash_on(&usb, 501)
        });
        assert_eq!(w.read("USB/.Trashes/501/b.txt"), "beta");
        // Readable again, so the scratch folder can be cleared away.
        fs::set_permissions(usb.join(".Trashes"), Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn an_item_whose_volume_has_no_trash_stays_and_is_never_deleted() {
        let w = world();
        let usb = w.folder("USB");
        let a = w.file("USB/a.txt", "alpha");
        fs::set_permissions(&usb, Permissions::from_mode(0o555)).unwrap();
        let report = trash_with(&[a], &w.trash, &crossing(usb.clone()), &|_| {
            trash_on(&usb, 501)
        });
        fs::set_permissions(&usb, Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            said(&report).as_deref(),
            Some("Couldn't move a.txt to the trash: Permission was refused.")
        );
        assert!(report.change.steps.is_empty());
        assert_eq!(w.read("USB/a.txt"), "alpha");
        assert_eq!(w.names("USB"), ["a.txt"]);
    }

    #[test]
    fn a_volumes_top_folder_is_found_by_walking_up_to_where_the_disk_changes() {
        let w = world();
        let deep = w.folder("one/two");
        let top = mount_point(&deep).unwrap();
        assert!(deep.starts_with(&top), "{top:?}");
        let device = fs::metadata(&deep).unwrap().dev();
        assert_eq!(fs::metadata(&top).unwrap().dev(), device);
        if let Some(above) = top.parent() {
            assert_ne!(fs::metadata(above).unwrap().dev(), device);
        }
    }

    #[test]
    fn deleting_removes_files_folders_and_links_but_never_what_a_link_points_at() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("Dir/b.txt", "beta");
        w.file("target.txt", "kept");
        w.file("Kept/in.txt", "kept too");
        symlink(w.path("target.txt"), w.path("link")).unwrap();
        symlink(w.path("Kept"), w.path("door")).unwrap();
        symlink(w.path("Kept"), w.path("Dir/door")).unwrap();
        let items = [a, w.path("Dir"), w.path("link"), w.path("door")];
        let deleted = delete(&items);
        assert_eq!((deleted.done, deleted.error), (4, None));
        assert_eq!(w.names(""), [".Trash", "Kept", "target.txt"]);
        assert_eq!(w.read("target.txt"), "kept");
        assert_eq!(w.read("Kept/in.txt"), "kept too");
        assert!(w.names(".Trash").is_empty(), "gone for good, not trashed");
    }

    #[test]
    fn what_could_not_be_deleted_is_counted() {
        let w = world();
        let stuck = w.file("Locked/stuck.txt", "stays");
        let loose = w.file("loose.txt", "goes");
        fs::set_permissions(w.path("Locked"), Permissions::from_mode(0o555)).unwrap();
        let deleted = delete(&[stuck, loose]);
        fs::set_permissions(w.path("Locked"), Permissions::from_mode(0o755)).unwrap();
        assert_eq!(deleted.done, 1);
        assert_eq!(
            deleted.error.map(|error| error.to_string()).as_deref(),
            Some("Couldn't delete 1 item.")
        );
        assert_eq!(w.read("Locked/stuck.txt"), "stays");
        assert!(!w.has("loose.txt"));
    }

    #[test]
    fn undoing_a_copy_moves_the_copy_to_the_trash_and_redo_brings_it_back() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let into = w.folder("Into");
        let mut history = History::default();
        history.record(copy_into(&[a], &into, &go()).change);

        let undone = history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(undone.kind, Kind::Copy);
        assert!(!w.has("Into/a.txt"));
        assert_eq!(w.read(".Trash/a.txt"), "alpha", "in the trash, not deleted");
        assert_eq!(w.read("a.txt"), "alpha");

        let redone = history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(redone.landed(), [into.join("a.txt")]);
        assert_eq!(w.read("Into/a.txt"), "alpha");
        assert!(w.names(".Trash").is_empty());
        history.undo(&w.trash).unwrap().unwrap();
        assert!(!w.has("Into/a.txt"));
    }

    #[test]
    fn undoing_a_duplicate_moves_the_copy_to_the_trash() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let mut history = History::default();
        history.record(duplicate(&[a], &go()).change);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(w.names(""), [".Trash", "a.txt"]);
        assert_eq!(w.read(".Trash/a copy.txt"), "alpha");
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("a copy.txt"), "alpha");
    }

    #[test]
    fn undoing_a_move_moves_it_back_and_redo_moves_it_again() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        w.file("Dir/b.txt", "beta");
        let into = w.folder("Into");
        let mut history = History::default();
        history.record(move_into(&[a, w.path("Dir")], &into, &go()).change);

        let undone = history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(undone.origins(), [w.path("a.txt"), w.path("Dir")]);
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!(w.read("Dir/b.txt"), "beta");
        assert!(w.names("Into").is_empty());
        assert!(w.names(".Trash").is_empty());

        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("Into/a.txt"), "alpha");
        assert_eq!(w.read("Into/Dir/b.txt"), "beta");
        assert!(!w.has("a.txt") && !w.has("Dir"));
    }

    #[test]
    fn undoing_a_rename_gives_the_old_name_back() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let mut history = History::default();
        history.record(rename(&a, "b.txt").unwrap().unwrap());
        let undone = history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(
            (undone.kind, undone.name().as_str()),
            (Kind::Rename, "a.txt")
        );
        assert_eq!(w.names(""), [".Trash", "a.txt"]);
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.names(""), [".Trash", "b.txt"]);
        assert_eq!(w.read("b.txt"), "alpha");
    }

    #[test]
    fn undoing_a_new_folder_moves_it_to_the_trash_with_what_was_put_in_it() {
        let w = world();
        let mut history = History::default();
        history.record(new_folder(&w.root, "Plans").unwrap().unwrap());
        w.file("Plans/idea.txt", "written since");
        history.undo(&w.trash).unwrap().unwrap();
        assert!(!w.has("Plans"));
        assert_eq!(w.read(".Trash/Plans/idea.txt"), "written since");
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("Plans/idea.txt"), "written since");
    }

    #[test]
    fn undoing_a_trash_takes_it_back_out_and_redo_trashes_it_again() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let mut history = History::default();
        history.record(trash(&[a], &w.trash).change);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("a.txt"), "alpha");
        assert!(w.names(".Trash").is_empty());

        // Its old name in the trash went to something else in the meantime.
        w.file(".Trash/a.txt", "another one");
        let redone = history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(redone.kind, Kind::Trash);
        assert!(!w.has("a.txt"));
        assert_eq!(w.read(".Trash/a.txt"), "another one");
        assert_eq!(w.read(".Trash/a copy.txt"), "alpha");
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!(w.names(".Trash"), ["a.txt"]);
    }

    #[test]
    fn the_log_ends_at_both_ends_and_a_new_change_forgets_what_was_undone() {
        let w = world();
        let mut history = History::default();
        assert_eq!(history.undo(&w.trash), Ok(None));
        assert_eq!(history.redo(&w.trash), Ok(None));

        let a = w.file("a.txt", "alpha");
        history.record(rename(&a, "b.txt").unwrap().unwrap());
        // A change that did nothing isn't worth a step back.
        history.record(move_into(&[w.path("b.txt")], &w.root, &go()).change);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(history.undo(&w.trash), Ok(None));
        history.record(new_folder(&w.root, "Plans").unwrap().unwrap());
        assert_eq!(
            history.redo(&w.trash),
            Ok(None),
            "the rename can't come back"
        );
        assert_eq!(w.names(""), [".Trash", "Plans", "a.txt"]);
    }

    #[test]
    fn undo_refuses_when_the_item_is_not_where_it_was_left() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let into = w.folder("Into");
        let mut history = History::default();
        history.record(move_into(&[a], &into, &go()).change);
        fs::rename(into.join("a.txt"), into.join("elsewhere.txt")).unwrap();

        let error = history.undo(&w.trash).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't undo that: a.txt isn't where it was left."
        );
        assert_eq!(w.names("Into"), ["elsewhere.txt"]);
        assert!(!w.has("a.txt"));

        // Still on record: put back where it was left, it can be undone after all.
        fs::rename(into.join("elsewhere.txt"), into.join("a.txt")).unwrap();
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("a.txt"), "alpha");
    }

    #[test]
    fn undo_refuses_when_the_old_place_is_taken_and_clobbers_nothing() {
        let w = world();
        let a = w.file("Docs/a.txt", "moved");
        let b = w.file("Docs/b.txt", "trashed");
        let c = w.file("Docs/c.txt", "renamed");
        let into = w.folder("Into");
        let mut history = History::default();
        history.record(rename(&c, "d.txt").unwrap().unwrap());
        history.record(trash(&[b], &w.trash).change);
        history.record(move_into(&[a], &into, &go()).change);
        for name in ["a.txt", "b.txt", "c.txt"] {
            w.file(&format!("Docs/{name}"), "newer");
        }

        for name in ["a.txt", "b.txt", "c.txt"] {
            let error = history.undo(&w.trash).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("Can't undo that: {name} already exists in Docs.")
            );
            assert_eq!(w.read(&format!("Docs/{name}")), "newer");
            // Out of the way, the same undo goes through.
            fs::remove_file(w.path(&format!("Docs/{name}"))).unwrap();
            history.undo(&w.trash).unwrap().unwrap();
        }
        assert_eq!(w.read("Docs/a.txt"), "moved");
        assert_eq!(w.read("Docs/b.txt"), "trashed");
        assert_eq!(w.read("Docs/c.txt"), "renamed");
    }

    #[test]
    fn undoing_something_made_refuses_when_it_is_gone() {
        let w = world();
        let mut history = History::default();
        history.record(new_folder(&w.root, "Plans").unwrap().unwrap());
        fs::remove_dir(w.path("Plans")).unwrap();
        let error = history.undo(&w.trash).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't undo that: Plans isn't where it was left."
        );
    }

    #[test]
    fn redo_refuses_when_the_world_changed_and_clobbers_nothing() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let into = w.folder("Into");
        let mut history = History::default();
        history.record(move_into(&[a], &into, &go()).change);
        history.undo(&w.trash).unwrap().unwrap();

        w.file("Into/a.txt", "newer");
        let taken = history.redo(&w.trash).unwrap_err();
        assert_eq!(
            taken.to_string(),
            "Can't redo that: a.txt already exists in Into."
        );
        assert_eq!(w.read("Into/a.txt"), "newer");
        assert_eq!(w.read("a.txt"), "alpha");

        fs::remove_file(w.path("Into/a.txt")).unwrap();
        fs::rename(w.path("a.txt"), w.path("gone.txt")).unwrap();
        let missing = history.redo(&w.trash).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "Can't redo that: a.txt isn't where it was left."
        );
        assert_eq!(w.read("gone.txt"), "alpha");

        // A copy that was undone waits in the trash; emptied from there, it can't come back.
        let mut copies = History::default();
        copies.record(duplicate(&[w.path("gone.txt")], &go()).change);
        copies.undo(&w.trash).unwrap().unwrap();
        fs::remove_file(w.path(".Trash/gone copy.txt")).unwrap();
        let emptied = copies.redo(&w.trash).unwrap_err();
        assert_eq!(
            emptied.to_string(),
            "Can't redo that: gone copy.txt isn't where it was left."
        );
    }

    #[test]
    fn a_rename_never_lands_on_something_that_is_there() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("b.txt", "beta");
        w.file("Full/x.txt", "x");
        let empty = w.folder("Empty");
        type Renamer = fn(&Path, &Path) -> io::Result<()>;
        // The one macOS does in a single step, and the one for volumes without it.
        for rename in [rename_new as Renamer, rename_held as Renamer] {
            for (from, to) in [
                (&a, &b),
                (&a, &empty),
                (&empty, &a),
                (&empty, &w.path("Full")),
            ] {
                let error = rename(from, to).unwrap_err();
                assert_eq!(
                    error.kind(),
                    io::ErrorKind::AlreadyExists,
                    "{from:?} {to:?}"
                );
            }
            assert_eq!(w.read("a.txt"), "alpha");
            assert_eq!(w.read("b.txt"), "beta");
            assert_eq!(w.names("Full"), ["x.txt"]);
            assert!(empty.is_dir());
        }
        rename_held(&a, &w.path("c.txt")).unwrap();
        rename_held(&empty, &w.path("Moved")).unwrap();
        rename_new(&b, &w.path("d.txt")).unwrap();
        assert_eq!(w.names(""), [".Trash", "Full", "Moved", "c.txt", "d.txt"]);
        assert_eq!(w.read("c.txt"), "alpha");
        assert_eq!(w.read("d.txt"), "beta");
    }

    #[test]
    fn a_name_splits_before_the_extension_of_a_file() {
        assert_eq!(split_name("notes.txt", false), ("notes", ".txt"));
        assert_eq!(split_name("archive.tar.gz", false), ("archive.tar", ".gz"));
        assert_eq!(split_name("README", false), ("README", ""));
        assert_eq!(split_name(".env", false), (".env", ""));
        assert_eq!(split_name("odd.", false), ("odd.", ""));
        assert_eq!(split_name("My.Stuff", true), ("My.Stuff", ""));
        assert_eq!(split_name("añb.txt", false), ("añb", ".txt"));
    }

    // Compressing. The archiver is a stand-in that writes what it is told to:
    // the real one is another program, behind the `System` port.

    #[test]
    fn compressing_one_item_makes_a_zip_beside_it_under_its_name_without_extension() {
        let w = world();
        let notes = w.file("Docs/notes.txt", "hello");
        let trip = w.folder("Docs/Trip");
        assert_eq!(
            archive_name(std::slice::from_ref(&notes), Format::Zip).unwrap(),
            "notes.zip"
        );
        let report = compress(
            std::slice::from_ref(&notes),
            Format::Zip,
            &go(),
            |archive| fs::write(archive, "zip of notes"),
        );
        assert_eq!(w.names("Docs"), ["Trip", "notes.txt", "notes.zip"]);
        assert_eq!(w.read("Docs/notes.zip"), "zip of notes");
        assert_eq!(w.read("Docs/notes.txt"), "hello");
        assert_eq!((report.asked, report.done, report.stopped), (1, 1, false));
        assert_eq!(said(&report), None);
        assert_eq!(report.change.kind, Kind::Compress);
        let archive = w.path("Docs/notes.zip");
        assert_eq!(report.change.steps, [created(archive.clone())]);
        assert_eq!(report.change.landed(), [archive]);

        // A folder's archive has the folder's whole name.
        let report = compress(&[trip], Format::Zip, &go(), |archive| {
            fs::write(archive, "zip of Trip")
        });
        assert_eq!(said(&report), None);
        assert_eq!(w.read("Docs/Trip.zip"), "zip of Trip");
        assert_eq!(
            w.names("Docs"),
            ["Trip", "Trip.zip", "notes.txt", "notes.zip"]
        );
    }

    #[test]
    fn compressing_several_items_makes_one_archive_named_after_their_folder() {
        let w = world();
        let items = [w.file("Docs/a.txt", "a"), w.folder("Docs/Trip")];
        assert_eq!(archive_name(&items, Format::Zip).unwrap(), "Docs.zip");
        let report = compress(&items, Format::Zip, &go(), |archive| {
            fs::write(archive, "zip of both")
        });
        assert_eq!(w.names("Docs"), ["Docs.zip", "Trip", "a.txt"]);
        assert_eq!(w.read("Docs/Docs.zip"), "zip of both");
        assert_eq!((report.asked, report.done, said(&report)), (2, 2, None));
        assert_eq!(
            report.change.steps,
            [created(w.path("Docs/Docs.zip"))],
            "one archive, one thing to undo"
        );
    }

    #[test]
    fn a_taken_archive_name_keeps_both_and_nothing_is_overwritten() {
        let w = world();
        let notes = w.file("notes.txt", "hello");
        w.file("notes.zip", "old archive");
        let items = [notes];
        assert_eq!(archive_name(&items, Format::Zip).unwrap(), "notes copy.zip");
        let report = compress(&items, Format::Zip, &go(), |archive| {
            fs::write(archive, "second")
        });
        assert_eq!(said(&report), None);
        assert_eq!(report.change.steps, [created(w.path("notes copy.zip"))]);
        assert_eq!(w.read("notes.zip"), "old archive");
        assert_eq!(w.read("notes copy.zip"), "second");

        // A file that takes the name while the archive is being written is
        // not written over either.
        let late = w.path("notes copy 2.zip");
        let report = compress(&items, Format::Zip, &go(), |archive| {
            fs::write(&late, "appeared meanwhile")?;
            fs::write(archive, "third")
        });
        assert_eq!(said(&report), None);
        assert_eq!(w.read("notes copy 2.zip"), "appeared meanwhile");
        assert_eq!(w.read("notes copy 3.zip"), "third");
        assert_eq!(
            w.names(""),
            [
                ".Trash",
                "notes copy 2.zip",
                "notes copy 3.zip",
                "notes copy.zip",
                "notes.txt",
                "notes.zip",
            ],
            "and nothing else is left behind"
        );
    }

    #[test]
    fn a_compress_that_fails_or_is_stopped_leaves_no_archive_and_nothing_to_undo() {
        let w = world();
        let notes = w.file("Docs/notes.txt", "hello");
        let items = [notes];
        let report = compress(&items, Format::Zip, &go(), |archive| {
            fs::write(archive, "half an archive")?;
            Err(io::Error::other("the archive couldn't be written"))
        });
        assert_eq!(
            said(&report).as_deref(),
            Some("Couldn't compress: The archive couldn't be written.")
        );
        assert_eq!((report.done, report.stopped), (0, false));
        assert!(report.change.steps.is_empty());
        assert_eq!(w.names("Docs"), ["notes.txt"], "hidden leftovers included");

        // Stopped: the same, without it being a failure.
        let stop = AtomicBool::new(false);
        let report = compress(&items, Format::Zip, &stop, |archive| {
            fs::write(archive, "half an archive")?;
            stop.store(true, Ordering::Relaxed);
            Err(io::ErrorKind::Interrupted.into())
        });
        assert_eq!(
            (report.done, report.stopped, said(&report)),
            (0, true, None)
        );
        assert!(report.change.steps.is_empty());
        assert_eq!(w.names("Docs"), ["notes.txt"]);
        assert_eq!(w.read("Docs/notes.txt"), "hello");

        // A folder that takes nothing new: said, and the archiver never runs.
        fs::set_permissions(w.path("Docs"), Permissions::from_mode(0o555)).unwrap();
        let report = compress(&items, Format::Zip, &go(), |_| {
            panic!("there is nowhere to write")
        });
        fs::set_permissions(w.path("Docs"), Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            said(&report).as_deref(),
            Some("Couldn't compress: Permission was refused.")
        );
        assert_eq!(w.names("Docs"), ["notes.txt"]);
    }

    #[test]
    fn items_from_different_folders_are_not_compressed_together() {
        let w = world();
        let items = [w.file("One/a.txt", "a"), w.file("Two/b.txt", "b")];
        let why = "Couldn't compress: The items are in different folders.";
        assert_eq!(
            archive_name(&items, Format::Zip).unwrap_err().to_string(),
            why
        );
        let report = compress(&items, Format::Zip, &go(), |_| panic!("nothing to write"));
        assert_eq!(said(&report).as_deref(), Some(why));
        assert_eq!(w.names("One"), ["a.txt"]);
        assert_eq!(w.names("Two"), ["b.txt"]);
        assert_eq!(w.names(""), [".Trash", "One", "Two"]);
    }

    // Restoring from the trash, and emptying it.

    /// Trashes one item and says where it is in the trash.
    fn trashed(w: &World, item: &Path) -> PathBuf {
        let report = trash(&[item.to_path_buf()], &w.trash);
        match report.change.steps.as_slice() {
            [Step::Trashed { to, .. }] => to.clone(),
            steps => panic!("not trashed: {steps:?}"),
        }
    }

    #[test]
    fn restoring_moves_an_item_back_to_where_it_came_from_and_can_be_undone() {
        let w = world();
        let notes = w.file("Docs/notes.txt", "hello");
        let trip = w
            .file("Docs/Trip/a.jpg", "jpg")
            .parent()
            .unwrap()
            .to_path_buf();
        let (in_trash, trip_in_trash) = (trashed(&w, &notes), trashed(&w, &trip));
        assert!(w.names("Docs").is_empty());

        let items = [
            (in_trash.clone(), Some(notes.clone())),
            (trip_in_trash.clone(), Some(trip.clone())),
        ];
        let report = restore(&items, &w.trash);
        assert_eq!((report.asked, report.done, said(&report)), (2, 2, None));
        assert_eq!(report.change.kind, Kind::Restore);
        assert_eq!(w.read("Docs/notes.txt"), "hello");
        assert_eq!(w.read("Docs/Trip/a.jpg"), "jpg");
        assert!(w.names(".Trash").is_empty());
        let moved = |from: &PathBuf, to: &PathBuf| Step::Moved {
            from: from.clone(),
            to: to.clone(),
        };
        assert_eq!(
            report.change.steps,
            [moved(&in_trash, &notes), moved(&trip_in_trash, &trip)]
        );

        // Undone, they are in the trash again; redone, back out.
        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap();
        assert_eq!(w.names(".Trash"), ["Trip", "notes.txt"]);
        assert!(w.names("Docs").is_empty());
        history.redo(&w.trash).unwrap();
        assert_eq!(w.names("Docs"), ["Trip", "notes.txt"]);
    }

    #[test]
    fn a_restore_never_overwrites_and_needs_the_folder_it_came_from() {
        let w = world();
        // The name was taken since.
        let notes = w.file("Docs/notes.txt", "hello");
        let notes_in_trash = trashed(&w, &notes);
        w.file("Docs/notes.txt", "written since");
        let report = restore(&[(notes_in_trash.clone(), Some(notes))], &w.trash);
        assert_eq!(
            said(&report).as_deref(),
            Some("Couldn't restore notes.txt: notes.txt already exists in Docs.")
        );
        assert_eq!(w.read("Docs/notes.txt"), "written since");
        assert_eq!(w.read(".Trash/notes.txt"), "hello");
        assert!(report.change.steps.is_empty());

        // The folder it was in is gone.
        let b = w.file("Gone/b.txt", "beta");
        let b_in_trash = trashed(&w, &b);
        fs::remove_dir(w.path("Gone")).unwrap();
        let report = restore(&[(b_in_trash.clone(), Some(b))], &w.trash);
        assert_eq!(
            said(&report).as_deref(),
            Some("Couldn't restore b.txt: Gone isn't there any more.")
        );
        assert!(!w.has("Gone"), "it is not made again");
        assert_eq!(w.read(".Trash/b.txt"), "beta");

        // Nobody knows where it came from.
        let c = w.file(".Trash/c.txt", "gamma");
        let report = restore(&[(c, None)], &w.trash);
        assert_eq!(
            said(&report).as_deref(),
            Some("c.txt has no record of where it came from.")
        );
        assert_eq!((report.done, report.change.steps.len()), (0, 0));
        assert_eq!(w.names(".Trash"), ["b.txt", "c.txt", "notes.txt"]);
    }

    #[test]
    fn the_trash_holds_what_is_directly_in_it_and_a_link_is_no_trash() {
        let w = world();
        w.file(".Trash/a.txt", "a");
        w.file(".Trash/Folder/inside.txt", "in");
        w.file(".Trash/.env", "hidden, but trashed like the rest");
        // Finder's own notes about the folder are nobody's trash.
        w.file(".Trash/.DS_Store", "");
        let held = |name: &str| w.trash.join(name);
        assert_eq!(
            trash_items(&w.trash).unwrap(),
            [held(".env"), held("Folder"), held("a.txt")]
        );

        // One that was never made holds nothing.
        assert!(trash_items(&w.path("never-made")).unwrap().is_empty());

        // One that can't be listed says so.
        fs::set_permissions(&w.trash, Permissions::from_mode(0o000)).unwrap();
        let refused = trash_items(&w.trash).unwrap_err();
        fs::set_permissions(&w.trash, Permissions::from_mode(0o755)).unwrap();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);

        // A link is not followed to the folder it leads to.
        let elsewhere = w.file("Elsewhere/keep.txt", "keep");
        let link = w.path("Linked Trash");
        symlink(elsewhere.parent().unwrap(), &link).unwrap();
        assert!(trash_items(&link).is_err());
    }

    // Paste conflicts: what a paste does with the names it meets.

    fn inode(path: &Path) -> u64 {
        fs::symlink_metadata(path).unwrap().ino()
    }

    #[test]
    fn clashes_are_the_names_a_paste_would_meet_in_the_order_given() {
        let w = world();
        let (a, b, c) = (w.file("a.txt", "a"), w.file("b.txt", "b"), w.file("c", "c"));
        let into = w.folder("Into");
        w.file("Into/c", "c there");
        w.file("Into/a.txt", "a there");
        assert_eq!(clashes(&[a.clone(), b, c], &into, false), ["a.txt", "c"]);
        // A copy into its own folder meets itself; a move there stays put.
        assert_eq!(clashes(std::slice::from_ref(&a), &w.root, false), ["a.txt"]);
        assert!(clashes(&[a], &w.root, true).is_empty());
    }

    #[test]
    fn keep_both_names_the_newcomer_like_flea_never_touching_the_one_there() {
        let w = world();
        let a = w.file("a.txt", "new");
        let into = w.folder("Into");
        let there = w.file("Into/a.txt", "old");
        let before = inode(&there);
        for _ in 0..2 {
            let report = transfer(
                std::slice::from_ref(&a),
                &into,
                false,
                Collide::KeepBoth,
                &w.trash,
                &go(),
            );
            assert_eq!((said(&report), report.done), (None, 1));
        }
        assert_eq!(w.names("Into"), ["a copy 2.txt", "a copy.txt", "a.txt"]);
        assert_eq!(
            (w.read("Into/a.txt"), inode(&there)),
            ("old".to_string(), before)
        );
        assert_eq!(w.read("Into/a copy 2.txt"), "new");
        assert!(w.names(".Trash").is_empty());
    }

    #[test]
    fn replace_sends_the_one_there_to_the_trash_first_and_undo_brings_it_back_whole() {
        let w = world();
        let a = w.file("a.txt", "new");
        let into = w.folder("Into");
        let there = w.file("Into/a.txt", "old");
        let original = inode(&there);
        let report = transfer(
            std::slice::from_ref(&a),
            &into,
            false,
            Collide::Replace,
            &w.trash,
            &go(),
        );
        assert_eq!((said(&report), report.done), (None, 1));
        assert_eq!(w.read("Into/a.txt"), "new");
        assert_eq!(w.read("a.txt"), "new", "a copy leaves the original");
        // Moved there whole, never written over in place.
        assert_eq!(w.names(".Trash"), ["a.txt"]);
        assert_eq!(w.read(".Trash/a.txt"), "old");
        assert_eq!(inode(&w.path(".Trash/a.txt")), original);
        assert_ne!(inode(&there), original);
        let parked = w.path(".Trash/a.txt");
        assert_eq!(
            report.change.steps,
            [
                Step::Trashed {
                    from: there.clone(),
                    to: parked.clone()
                },
                created(there.clone())
            ]
        );

        // One undo step: the original back where it was, the copy to the trash.
        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("Into/a.txt"), "old");
        assert_eq!(inode(&there), original);
        assert_eq!(w.names("Into"), ["a.txt"]);
        // The copy goes first, while the original still has its name there.
        assert_eq!(w.names(".Trash"), ["a copy.txt"]);
        assert_eq!(
            w.read(".Trash/a copy.txt"),
            "new",
            "the copy waits there for redo"
        );
        // Redo replaces again; undo takes it back again.
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("Into/a.txt"), "new");
        assert_eq!(w.names(".Trash"), ["a.txt"]);
        assert_eq!(inode(&w.path(".Trash/a.txt")), original);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(
            (w.read("Into/a.txt"), inode(&there)),
            ("old".to_string(), original)
        );
        assert_eq!(w.read(".Trash/a copy.txt"), "new");
    }

    #[test]
    fn replace_while_moving_puts_the_item_there_and_undo_puts_both_back() {
        let w = world();
        let a = w.file("From/a.txt", "new");
        let moving = inode(&a);
        let into = w.folder("Into");
        let there = w.file("Into/a.txt", "old");
        let original = inode(&there);
        let report = transfer(
            std::slice::from_ref(&a),
            &into,
            true,
            Collide::Replace,
            &w.trash,
            &go(),
        );
        assert_eq!((said(&report), report.done), (None, 1));
        assert!(w.names("From").is_empty());
        assert_eq!(
            (w.read("Into/a.txt"), inode(&there)),
            ("new".to_string(), moving)
        );
        assert_eq!(inode(&w.path(".Trash/a.txt")), original);

        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(
            (w.read("From/a.txt"), inode(&a)),
            ("new".to_string(), moving)
        );
        assert_eq!(
            (w.read("Into/a.txt"), inode(&there)),
            ("old".to_string(), original)
        );
        assert!(w.names(".Trash").is_empty());
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(inode(&there), moving);
        assert_eq!(inode(&w.path(".Trash/a.txt")), original);
    }

    #[test]
    fn replace_takes_a_whole_folder_to_the_trash_with_all_it_holds() {
        let w = world();
        let a = w.file("Box", "a file now");
        let into = w.folder("Into");
        w.file("Into/Box/kept.txt", "kept");
        let report = transfer(&[a], &into, false, Collide::Replace, &w.trash, &go());
        assert_eq!(said(&report), None);
        assert_eq!(w.read("Into/Box"), "a file now");
        assert_eq!(w.read(".Trash/Box/kept.txt"), "kept");
    }

    #[test]
    fn skip_leaves_each_item_whose_name_is_taken_and_counts_it() {
        let w = world();
        let (a, b) = (w.file("a.txt", "new a"), w.file("b.txt", "new b"));
        let into = w.folder("Into");
        w.file("Into/a.txt", "old a");
        let report = transfer(&[a, b], &into, true, Collide::Skip, &w.trash, &go());
        assert_eq!(said(&report), None);
        assert_eq!((report.done, report.declined, report.skipped), (1, 1, 0));
        assert_eq!(w.read("Into/a.txt"), "old a");
        assert_eq!(w.read("Into/b.txt"), "new b");
        assert_eq!(
            w.read("a.txt"),
            "new a",
            "the skipped one stays where it was"
        );
        assert!(!w.has("b.txt"));
        assert!(w.names(".Trash").is_empty());
    }

    #[test]
    fn replace_never_loses_the_one_there_when_the_new_one_cannot_go_in() {
        let w = world();
        let into = w.folder("Into");
        let there = w.file("Into/a.txt", "old");
        let original = inode(&there);

        // An item that can't be read: the one there is put back as it was.
        let locked = w.file("a.txt", "new");
        fs::set_permissions(&locked, Permissions::from_mode(0o000)).unwrap();
        let report = transfer(
            std::slice::from_ref(&locked),
            &into,
            false,
            Collide::Replace,
            &w.trash,
            &go(),
        );
        fs::set_permissions(&locked, Permissions::from_mode(0o644)).unwrap();
        assert!(said(&report).is_some_and(|said| said.starts_with("Couldn't copy a.txt")));
        assert_eq!(
            (w.read("Into/a.txt"), inode(&there)),
            ("old".to_string(), original)
        );
        assert!(w.names(".Trash").is_empty());
        assert!(report.change.steps.is_empty());

        // Replacing an item with itself changes nothing.
        let report = transfer(
            std::slice::from_ref(&there),
            &into,
            false,
            Collide::Replace,
            &w.trash,
            &go(),
        );
        assert_eq!((said(&report), report.done, report.declined), (None, 0, 1));
        assert_eq!(
            (w.read("Into/a.txt"), inode(&there)),
            ("old".to_string(), original)
        );

        // Nor can the one there go when it holds what replaces it.
        let inner = w.file("Docs/Docs", "inside");
        let docs = w.path("Docs");
        let report = transfer(&[inner], &w.root, false, Collide::Replace, &w.trash, &go());
        assert_eq!(
            said(&report).as_deref(),
            Some("Can't replace Docs: it holds Docs.")
        );
        assert_eq!(w.read("Docs/Docs"), "inside");
        assert!(docs.is_dir());
        assert!(w.names(".Trash").is_empty());
    }

    /// Whether the volume a folder is on tells names apart only by letters,
    /// not by case, as APFS and HFS+ do by default.
    fn ignores_case(folder: &Path) -> bool {
        let probe = folder.join("case-probe");
        fs::write(&probe, "").unwrap();
        let ignores = folder.join("CASE-PROBE").exists();
        fs::remove_file(probe).unwrap();
        ignores
    }

    #[test]
    fn a_replace_by_a_name_that_differs_only_in_case_gives_the_original_its_own_back() {
        let w = world();
        let into = w.folder("Into");
        if !ignores_case(&into) {
            // A case-sensitive volume: there is no clash to replace.
            return;
        }
        let a = w.file("a.txt", "new");
        let there = w.file("Into/A.TXT", "old");
        let original = inode(&there);
        // What is asked about is the name on disk.
        assert_eq!(clashes(std::slice::from_ref(&a), &into, false), ["A.TXT"]);
        let report = transfer(&[a], &into, false, Collide::Replace, &w.trash, &go());
        assert_eq!((said(&report), report.done), (None, 1));
        assert_eq!(w.names("Into"), ["a.txt"]);
        assert_eq!(w.read("Into/a.txt"), "new");
        // In the trash under its own spelling, so that Restore keeps it.
        assert_eq!(w.names(".Trash"), ["A.TXT"]);
        assert_eq!(inode(&w.path(".Trash/A.TXT")), original);

        let mut history = History::default();
        history.record(report.change);
        for _ in 0..2 {
            history.undo(&w.trash).unwrap().unwrap();
            assert_eq!(w.names("Into"), ["A.TXT"], "its own spelling");
            assert_eq!(w.read("Into/A.TXT"), "old");
            assert_eq!(inode(&w.path("Into/A.TXT")), original);
            history.redo(&w.trash).unwrap().unwrap();
            assert_eq!(w.names("Into"), ["a.txt"]);
            assert_eq!(w.read("Into/a.txt"), "new");
            assert!(w.names(".Trash").contains(&"A.TXT".to_string()));
        }
    }

    #[test]
    fn a_clash_is_named_after_the_entry_with_that_exact_name_when_there_is_one() {
        // Simulates what a case-sensitive volume relies on: the folder holds
        // an entry with the exact name, and another name for the same file
        // (a hard link); the exact one is the name used.
        let w = world();
        let into = w.folder("Into");
        let there = w.file("Into/a.txt", "old");
        fs::hard_link(&there, w.path("Into/Z.txt")).unwrap();
        let a = w.file("a.txt", "new");
        assert_eq!(clashes(std::slice::from_ref(&a), &into, false), ["a.txt"]);
        // A name nothing has is no clash, whatever the volume.
        let b = w.file("b.txt", "b");
        assert!(clashes(&[b], &into, false).is_empty());
    }

    #[test]
    fn a_case_only_clash_is_never_named_after_another_hard_link() {
        // `A.TXT` also has other names for the same file; whichever the
        // folder lists first, the clash for `a.txt` is `A.TXT`.
        let w = world();
        let into = w.folder("Into");
        let there = w.file("Into/A.TXT", "old");
        for other in ["Z.txt", "B.txt", "m.txt", "_x.txt", "0.txt"] {
            fs::hard_link(&there, w.path(&format!("Into/{other}"))).unwrap();
        }
        let a = w.file("a.txt", "new");
        assert_eq!(clashes(std::slice::from_ref(&a), &into, false), ["A.TXT"]);
        assert_eq!(
            taken_place(&into, OsStr::new("a.txt")),
            Some(into.join("A.TXT"))
        );
    }

    #[test]
    fn replace_on_a_fat_disk_is_refused_before_anything_moves() {
        let w = world();
        let usb = w.folder("USB");
        let _fat = Fat::at(usb.clone());
        let there = w.file("USB/x.txt", "old");
        w.file("USB/._x.txt", "my own notes");
        let a = w.file("x.txt", "new");
        let report = transfer(&[a], &usb, false, Collide::Replace, &w.trash, &go());
        assert_eq!(
            said(&report),
            side_by_side("Couldn't copy", "x.txt", "x.txt")
        );
        assert_eq!(w.read("USB/x.txt"), "old");
        assert_eq!(w.read("USB/._x.txt"), "my own notes");
        assert!(there.exists());
        assert!(w.names(".Trash").is_empty());
        assert!(report.change.steps.is_empty());
    }

    fn tag_names(path: &Path) -> Vec<String> {
        crate::tags::labels(path)
    }

    fn put_tags(path: &Path, labels: &[&str]) {
        let labels: Vec<String> = labels.iter().map(|label| label.to_string()).collect();
        crate::tags::write(path, Some(&crate::tags::encode(&labels))).unwrap();
    }

    #[test]
    fn a_tag_is_added_where_it_is_missing_and_removed_only_where_every_item_has_it() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("b.txt", "beta");
        put_tags(&a, &["Red\n6"]);
        let a_before = crate::tags::read(&a).unwrap();

        // Red on a, not on b: added to b, a left alone.
        let (what, change) = retag(&[a.clone(), b.clone()], Tagging::Toggle(0)).unwrap();
        assert_eq!(what, Retagged::Added(0));
        assert_eq!(change.kind, Kind::Tag);
        assert_eq!(change.steps.len(), 1);
        assert_eq!(crate::tags::read(&a).unwrap(), a_before);
        assert_eq!(tag_names(&b), ["Red\n6"]);

        // Now both have it: taken off both.
        let (what, change) = retag(&[a.clone(), b.clone()], Tagging::Toggle(0)).unwrap();
        assert_eq!(what, Retagged::Removed(0));
        assert_eq!(change.steps.len(), 2);
        assert_eq!(crate::tags::read(&a).unwrap(), None);
        assert_eq!(crate::tags::read(&b).unwrap(), None);
        // The files themselves never change.
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!(w.read("b.txt"), "beta");
    }

    #[test]
    fn tags_this_app_does_not_know_are_kept_through_every_change() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        put_tags(&a, &["Work", "Blue\n4", "Custom\n0"]);
        retag(std::slice::from_ref(&a), Tagging::Toggle(0)).unwrap();
        assert_eq!(tag_names(&a), ["Work", "Blue\n4", "Custom\n0", "Red\n6"]);
        retag(std::slice::from_ref(&a), Tagging::Toggle(4)).unwrap();
        assert_eq!(tag_names(&a), ["Work", "Custom\n0", "Red\n6"]);
        let (what, _) = retag(std::slice::from_ref(&a), Tagging::RemoveAll).unwrap();
        assert_eq!(what, Retagged::Cleared);
        assert_eq!(tag_names(&a), ["Work", "Custom\n0"]);
        // With only tags of its own, Remove All changes nothing.
        let (_, change) = retag(std::slice::from_ref(&a), Tagging::RemoveAll).unwrap();
        assert!(change.steps.is_empty());
        assert_eq!(tag_names(&a), ["Work", "Custom\n0"]);
    }

    #[test]
    fn undoing_a_tag_change_puts_back_the_exact_attribute_and_redo_does_it_again() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("b.txt", "beta");
        put_tags(&a, &["Custom\n0"]);
        let a_before = crate::tags::read(&a).unwrap();
        let mut history = History::default();
        let (_, change) = retag(&[a.clone(), b.clone()], Tagging::Toggle(2)).unwrap();
        history.record(change);
        let (a_after, b_after) = (
            crate::tags::read(&a).unwrap(),
            crate::tags::read(&b).unwrap(),
        );

        let undone = history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(undone.kind, Kind::Tag);
        assert_eq!(crate::tags::read(&a).unwrap(), a_before);
        // b had no attribute at all, and has none again.
        assert_eq!(crate::tags::read(&b).unwrap(), None);
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(crate::tags::read(&a).unwrap(), a_after);
        assert_eq!(crate::tags::read(&b).unwrap(), b_after);

        // Moved away, it isn't tagged back where it isn't.
        fs::rename(&b, w.path("c.txt")).unwrap();
        let error = history.undo(&w.trash).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't undo that: b.txt isn't where it was left."
        );
        assert_eq!(crate::tags::read(&a).unwrap(), a_after);
        fs::rename(w.path("c.txt"), &b).unwrap();

        // Tagged again since by someone else: that is never undone over.
        put_tags(&a, &["Purple\n3"]);
        let error = history.undo(&w.trash).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't undo that: the tags of a.txt changed since."
        );
        assert_eq!(tag_names(&a), ["Purple\n3"]);
        assert_eq!(crate::tags::read(&b).unwrap(), b_after);
    }

    #[test]
    fn a_disk_that_cannot_keep_tags_says_so_and_nothing_is_changed() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("Stick/b.txt", "beta");
        crate::tags::testing::pretend_unsupported(&w.path("Stick"));
        let error = retag(&[a.clone(), b.clone()], Tagging::Toggle(0)).unwrap_err();
        assert_eq!(error.to_string(), crate::tags::UNSUPPORTED);
        // a was tagged first, and untagged again when b couldn't be.
        assert_eq!(crate::tags::read(&a).unwrap(), None);
        assert_eq!(crate::tags::read(&b).unwrap(), None);
    }

    #[test]
    fn tags_are_never_written_where_the_system_would_write_over_a_file_of_the_users() {
        let w = world();
        let usb = w.folder("USB");
        let _fat = Fat::at(usb.clone());
        let x = w.file("USB/x.txt", "x");
        w.file("USB/._x.txt", "my own notes");
        let error = retag(std::slice::from_ref(&x), Tagging::Toggle(0)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Couldn't tag x.txt: x.txt and ._x.txt can't be side by side on this disk."
        );
        assert_eq!(w.read("USB/._x.txt"), "my own notes");
        assert_eq!(crate::tags::read(&x).unwrap(), None);
    }

    /// `com.apple.FinderInfo` as Foundation left it, read with `xattr -px`
    /// after setting tags with `NSURLTagNamesKey`: 32 bytes, the Finder
    /// flags' colour bits (byte 9, `0x0E`) holding the last colour tag's
    /// number. `kind` is the file type and creator, `flags` the high byte.
    fn finder_info(kind: &[u8; 8], flags: u8, colour_byte: u8) -> Vec<u8> {
        let mut info = vec![0u8; 32];
        info[..8].copy_from_slice(kind);
        info[8] = flags;
        info[9] = colour_byte;
        info
    }

    fn put_info(path: &Path, info: &[u8]) {
        let wanted = crate::tags::Stored {
            tags: crate::tags::read(path).unwrap(),
            info: Some(info.to_vec()),
        };
        crate::tags::store(path, &wanted).unwrap();
    }

    fn info_of(path: &Path) -> Option<Vec<u8>> {
        crate::tags::read_stored(path).unwrap().info
    }

    const NOTHING: [u8; 8] = [0; 8];
    const TEXT: &[u8; 8] = b"TEXTttxt";

    #[test]
    fn a_colour_tag_sets_finders_colour_label_as_foundation_does() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        // Red alone: what Foundation wrote for ["Red"].
        retag(std::slice::from_ref(&a), Tagging::Toggle(0)).unwrap();
        assert_eq!(info_of(&a), Some(finder_info(&NOTHING, 0, 0x0C)));
        // Red then Blue: the last colour, as for ["Red", "Blue"].
        retag(std::slice::from_ref(&a), Tagging::Toggle(4)).unwrap();
        assert_eq!(tag_names(&a), ["Red\n6", "Blue\n4"]);
        assert_eq!(info_of(&a), Some(finder_info(&NOTHING, 0, 0x08)));
        // A file type, a creator and another Finder flag stay as they were,
        // as they did for ["Green"].
        let b = w.file("b.txt", "beta");
        put_info(&b, &finder_info(TEXT, 0x40, 0));
        retag(std::slice::from_ref(&b), Tagging::Toggle(3)).unwrap();
        assert_eq!(info_of(&b), Some(finder_info(TEXT, 0x40, 0x04)));
        // A tag of the user's own with no colour leaves no label behind.
        let c = w.file("c.txt", "gamma");
        put_tags(&c, &["Work\n0"]);
        retag(std::slice::from_ref(&c), Tagging::Toggle(0)).unwrap();
        assert_eq!(info_of(&c), Some(finder_info(&NOTHING, 0, 0x0C)));
    }

    #[test]
    fn removing_the_last_colour_tag_takes_finders_colour_label_off_too() {
        let w = world();
        // Tagged Red the way Foundation tags it: both attributes.
        let a = w.file("a.txt", "alpha");
        put_tags(&a, &["Red\n6"]);
        put_info(&a, &finder_info(&NOTHING, 0, 0x0C));
        let (what, _) = retag(std::slice::from_ref(&a), Tagging::Toggle(0)).unwrap();
        assert_eq!(what, Retagged::Removed(0));
        assert_eq!(crate::tags::read(&a).unwrap(), None);
        // Nothing else was in it: it goes, as Foundation's does.
        assert_eq!(info_of(&a), None);

        // Other Finder flags keep the attribute, with only the colour gone.
        let b = w.file("b.txt", "beta");
        put_tags(&b, &["Red\n6", "Blue\n4"]);
        put_info(&b, &finder_info(TEXT, 0x40, 0x08));
        retag(std::slice::from_ref(&b), Tagging::Toggle(4)).unwrap();
        // Blue gone, Red left: Red's colour.
        assert_eq!(info_of(&b), Some(finder_info(TEXT, 0x40, 0x0C)));
        retag(std::slice::from_ref(&b), Tagging::Toggle(0)).unwrap();
        assert_eq!(info_of(&b), Some(finder_info(TEXT, 0x40, 0)));

        // Remove All Tags on Red and Blue, a tag of the user's own staying.
        let c = w.file("c.txt", "gamma");
        put_tags(&c, &["Red\n6", "Blue\n4", "Work\n0"]);
        put_info(&c, &finder_info(&NOTHING, 0, 0x08));
        let d = w.file("d.txt", "delta");
        put_tags(&d, &["Red\n6", "Blue\n4"]);
        put_info(&d, &finder_info(&NOTHING, 0, 0x08));
        retag(&[c.clone(), d.clone()], Tagging::RemoveAll).unwrap();
        assert_eq!(tag_names(&c), ["Work\n0"]);
        assert_eq!(info_of(&c), None);
        assert_eq!(crate::tags::read(&d).unwrap(), None);
        assert_eq!(info_of(&d), None);
    }

    #[test]
    fn undo_and_redo_put_back_both_attributes_byte_for_byte() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        put_tags(&a, &["Red\n6", "Work\n0"]);
        put_info(&a, &finder_info(TEXT, 0x40, 0x0C));
        let before = crate::tags::read_stored(&a).unwrap();
        let mut history = History::default();
        history.record(
            retag(std::slice::from_ref(&a), Tagging::RemoveAll)
                .unwrap()
                .1,
        );
        let after = crate::tags::read_stored(&a).unwrap();
        assert_eq!(after.info, Some(finder_info(TEXT, 0x40, 0)));
        history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(crate::tags::read_stored(&a).unwrap(), before);
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(crate::tags::read_stored(&a).unwrap(), after);

        // A file that had neither attribute has neither after undo.
        let b = w.file("b.txt", "beta");
        history.record(
            retag(std::slice::from_ref(&b), Tagging::Toggle(1))
                .unwrap()
                .1,
        );
        assert_eq!(info_of(&b), Some(finder_info(&NOTHING, 0, 0x0E)));
        history.undo(&w.trash).unwrap().unwrap();
        let none = crate::tags::Stored::default();
        assert_eq!(crate::tags::read_stored(&b).unwrap(), none);

        // Finder's label changed since by someone else: never undone over.
        history.redo(&w.trash).unwrap().unwrap();
        put_info(&b, &finder_info(TEXT, 0, 0x0E));
        let error = history.undo(&w.trash).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't undo that: the tags of b.txt changed since."
        );
        assert_eq!(info_of(&b), Some(finder_info(TEXT, 0, 0x0E)));
    }

    #[test]
    fn tags_that_cannot_be_read_are_never_written_over() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        crate::tags::write(&a, Some(b"not a property list")).unwrap();
        let error = retag(std::slice::from_ref(&a), Tagging::Toggle(0)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Couldn't tag a.txt: its tags can't be read."
        );
        assert_eq!(
            crate::tags::read(&a).unwrap().as_deref(),
            Some(&b"not a property list"[..])
        );
    }

    fn moves(pairs: &[(&PathBuf, &PathBuf)]) -> Vec<(PathBuf, PathBuf)> {
        pairs
            .iter()
            .map(|(from, to)| (from.to_path_buf(), to.to_path_buf()))
            .collect()
    }

    #[test]
    fn a_batch_renames_through_temporary_names_so_names_can_swap_and_go_round() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("b.txt", "beta");
        let c = w.file("c.txt", "gamma");
        // a and b swap names.
        let change = bulk_rename(&moves(&[(&a, &b), (&b, &a)])).unwrap();
        assert_eq!(change.kind, Kind::BulkRename);
        assert_eq!(change.steps.len(), 1);
        assert_eq!(w.read("a.txt"), "beta");
        assert_eq!(w.read("b.txt"), "alpha");
        assert_eq!(w.names(""), [".Trash", "a.txt", "b.txt", "c.txt"]);
        // a → b, b → c, c → a.
        let mut history = History::default();
        history.record(bulk_rename(&moves(&[(&a, &b), (&b, &c), (&c, &a)])).unwrap());
        assert_eq!(w.read("b.txt"), "beta");
        assert_eq!(w.read("c.txt"), "alpha");
        assert_eq!(w.read("a.txt"), "gamma");
        assert_eq!(w.names(""), [".Trash", "a.txt", "b.txt", "c.txt"]);

        // One undo takes the whole batch back, and redo does it again.
        let undone = history.undo(&w.trash).unwrap().unwrap();
        assert_eq!(undone.kind, Kind::BulkRename);
        assert_eq!(w.read("a.txt"), "beta");
        assert_eq!(w.read("b.txt"), "alpha");
        assert_eq!(w.read("c.txt"), "gamma");
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("a.txt"), "gamma");
        assert_eq!(w.names(""), [".Trash", "a.txt", "b.txt", "c.txt"]);

        // An item moved away since is never renamed back where it isn't.
        fs::rename(&c, w.path("elsewhere.txt")).unwrap();
        let error = history.undo(&w.trash).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Can't undo that: c.txt isn't where it was left."
        );
        assert_eq!(w.read("a.txt"), "gamma");
        assert_eq!(w.read("b.txt"), "beta");
    }

    #[test]
    fn a_batch_that_fails_part_way_puts_every_item_back_under_its_old_name() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("b.txt", "beta");
        let taken = w.file("taken.txt", "someone else's");
        let x = w.path("x.txt");
        // a gets its new name first; b's is taken by then.
        let error = bulk_rename(&moves(&[(&a, &x), (&b, &taken)])).unwrap_err();
        assert_eq!(error.to_string(), "Couldn't rename b.txt.");
        assert_eq!(w.read("a.txt"), "alpha");
        assert_eq!(w.read("b.txt"), "beta");
        assert_eq!(w.read("taken.txt"), "someone else's");
        assert_eq!(w.names(""), [".Trash", "a.txt", "b.txt", "taken.txt"]);
    }

    #[test]
    fn a_batch_of_names_is_refused_in_flea_words_before_anything_is_renamed() {
        let w = world();
        let a = w.file("a.txt", "alpha");
        let b = w.file("b.txt", "beta");
        let c = w.file("c.txt", "gamma");
        w.file("other.txt", "other");
        let items = [a.clone(), b.clone(), c.clone()];
        let plan = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
            plan_renames(&items, &names).map_err(|error| error.to_string())
        };
        let refused = |text: &str| Err(text.to_string());
        assert_eq!(
            plan(&["a.txt", "b.txt"]),
            refused("Bulk rename refused: the number of names changed")
        );
        assert_eq!(plan(&["a.txt", "  ", "c.txt"]), refused("Line 2: empty"));
        assert_eq!(
            plan(&["a/x.txt", "b.txt", "c.txt"]),
            refused("Line 1: contains /")
        );
        assert_eq!(plan(&["a.txt", "..", "c.txt"]), refused("Line 2: invalid"));
        assert_eq!(plan(&["a.txt", "b\0", "c.txt"]), refused("Line 2: invalid"));
        let long = "x".repeat(256);
        assert_eq!(
            plan(&["a.txt", "b.txt", &long]),
            refused("Line 3: too long")
        );
        assert_eq!(
            plan(&["d.txt", "e.txt", "D.TXT"]),
            refused("Line 3: same as line 1. Give each item a different name.")
        );
        // An unchanged name counts: c keeps it, so a can't have it.
        assert_eq!(
            plan(&["c.txt", "b.txt", "c.txt"]),
            refused("Line 3: same as line 1. Give each item a different name.")
        );
        assert_eq!(
            plan(&["other.txt", "b.txt", "c.txt"]),
            refused("Bulk rename refused: a destination already exists")
        );
        assert_eq!(
            w.names(""),
            [".Trash", "a.txt", "b.txt", "c.txt", "other.txt"]
        );

        // Unchanged names are left out; names taken by the batch itself are fine.
        assert_eq!(
            plan(&["b.txt", "a.txt", "c.txt"]),
            Ok(moves(&[(&a, &b), (&b, &a)]))
        );
        assert_eq!(plan(&["a.txt", "b.txt", "c.txt"]), Ok(Vec::new()));
        assert_eq!(
            plan(&["A.txt", "b.txt", "c.txt"]),
            Ok(vec![(a.clone(), w.path("A.txt"))])
        );
        // A folder and what is in it can't be renamed together.
        let folder = w.folder("Dir");
        let inside = w.file("Dir/d.txt", "delta");
        let names = vec!["Box".to_string(), "e.txt".to_string()];
        assert_eq!(
            plan_renames(&[folder, inside], &names)
                .unwrap_err()
                .to_string(),
            "Rename selected folders separately from their selected contents"
        );
    }

    #[test]
    fn the_names_are_edited_in_a_private_file_of_the_system_temporary_folder() {
        let names = vec!["a.txt".to_string(), "b c.txt".to_string()];
        let seen = std::cell::RefCell::new(None);
        let edited = edit_names(&names, |file| {
            assert!(file.starts_with(std::env::temp_dir()));
            assert_eq!(fs::read_to_string(file).unwrap(), "a.txt\nb c.txt\n");
            let folder = file.parent().unwrap();
            let mode = fs::metadata(folder).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
            *seen.borrow_mut() = Some(file.to_path_buf());
            fs::write(file, "x.txt\r\ny z.txt\n")
        });
        assert_eq!(
            edited.unwrap(),
            Some(vec!["x.txt".to_string(), "y z.txt".to_string()])
        );
        let file = seen.into_inner().unwrap();
        assert!(!file.exists() && !file.parent().unwrap().exists());

        // An editor that fails gives nothing back, and leaves nothing behind.
        let seen = std::cell::RefCell::new(None);
        let edited = edit_names(&names, |file| {
            *seen.borrow_mut() = Some(file.to_path_buf());
            Err(io::Error::other("the editor failed"))
        });
        assert_eq!(edited.unwrap(), None);
        assert!(!seen.into_inner().unwrap().parent().unwrap().exists());

        // A name holding a line break can't be one line of the file.
        let names = vec!["a\nb".to_string()];
        let error = edit_names(&names, |_| unreachable!()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Bulk rename cannot represent a filename containing a line break"
        );
    }

    #[test]
    fn an_archive_is_named_after_its_item_or_folder_and_never_takes_a_name_in_use() {
        let w = world();
        let notes = w.file("Work/notes.txt", "hello");
        let pack = w.folder("Work/pack");
        let dotted = w.file("Work/.profile", "dot");
        let name = |items: &[PathBuf], format| archive_name(items, format).unwrap();
        // One item: its name without its extension.
        assert_eq!(
            name(std::slice::from_ref(&notes), Format::TarGz),
            "notes.tar.gz"
        );
        assert_eq!(name(std::slice::from_ref(&pack), Format::Zip), "pack.zip");
        assert_eq!(
            name(std::slice::from_ref(&dotted), Format::Tar),
            ".profile.tar"
        );
        // Several: the folder they are in.
        assert_eq!(
            name(&[notes.clone(), pack.clone()], Format::TarXz),
            "Work.tar.xz"
        );
        // Taken: flea's copy names, the format's whole extension kept.
        w.file("Work/pack.tar.gz", "taken");
        assert_eq!(
            name(std::slice::from_ref(&pack), Format::TarGz),
            "pack copy.tar.gz"
        );
        w.file("Work/pack copy.tar.gz", "taken");
        assert_eq!(
            name(std::slice::from_ref(&pack), Format::TarGz),
            "pack copy 2.tar.gz"
        );
    }

    #[test]
    fn an_archive_of_each_format_is_made_beside_its_items_under_a_free_name() {
        use crate::system::{MacSystem, System};
        let w = world();
        let pack = w.folder("pack");
        w.file("pack/a.txt", "alpha");
        w.file("pack.tar.gz", "someone else's");
        let system = MacSystem::default();
        for format in [
            Format::Zip,
            Format::Tar,
            Format::TarGz,
            Format::TarBz2,
            Format::TarXz,
        ] {
            let items = std::slice::from_ref(&pack);
            let write = system.archiver(items, format);
            let report = compress(items, format, &go(), |archive| write(archive, &go()));
            assert_eq!(said(&report), None, "{format:?}");
            let named = match format {
                Format::TarGz => "pack copy.tar.gz".to_string(),
                _ => format!("pack.{}", format.extension()),
            };
            assert_eq!(report.change.steps, [created(w.path(&named))]);
        }
        assert_eq!(w.read("pack.tar.gz"), "someone else's");
        assert_eq!(
            w.names(""),
            [
                ".Trash",
                "pack",
                "pack copy.tar.gz",
                "pack.tar",
                "pack.tar.bz2",
                "pack.tar.gz",
                "pack.tar.xz",
                "pack.zip"
            ]
        );
    }

    /// Extracts with the system's own tools, as the app does.
    fn extract_here(archive: &Path, cancel: &dyn Cancel) -> Report {
        use crate::system::{MacSystem, System};
        let (format, _) = Format::of_archive(&shown(archive)).expect("an archive");
        let (list, unpack) = MacSystem::default().extractor(archive, format).unwrap();
        let go = go();
        extract(archive, cancel, || list(&go), |into| unpack(into, &go))
    }

    #[test]
    fn extracting_makes_a_new_folder_beside_the_archive_named_after_it() {
        use crate::system::{MacSystem, System};
        let w = world();
        let pack = w.folder("Src/pack");
        w.file("Src/pack/a.txt", "alpha");
        for format in [
            Format::Zip,
            Format::Tar,
            Format::TarGz,
            Format::TarBz2,
            Format::TarXz,
        ] {
            let archive = w.path(&format!("pack.{}", format.extension()));
            let write = MacSystem::default().archiver(std::slice::from_ref(&pack), format);
            write(&archive, &go()).unwrap();
            let report = extract_here(&archive, &go());
            assert_eq!(said(&report), None, "{format:?}");
            assert_eq!(report.change.kind, Kind::Extract);
            assert_eq!(report.done, 1);
            let folder = match format {
                Format::Zip => "pack".to_string(),
                _ => format!(
                    "pack copy{}",
                    match format {
                        Format::Tar => "".to_string(),
                        Format::TarGz => " 2".to_string(),
                        Format::TarBz2 => " 3".to_string(),
                        _ => " 4".to_string(),
                    }
                ),
            };
            assert_eq!(
                report.change.steps,
                [created(w.path(&folder))],
                "{format:?}"
            );
            assert_eq!(w.read(&format!("{folder}/pack/a.txt")), "alpha");
        }
        // Nothing of the work is left behind, and the archives are whole.
        assert_eq!(
            w.names(""),
            [
                ".Trash",
                "Src",
                "pack",
                "pack copy",
                "pack copy 2",
                "pack copy 3",
                "pack copy 4",
                "pack.tar",
                "pack.tar.bz2",
                "pack.tar.gz",
                "pack.tar.xz",
                "pack.zip"
            ]
        );

        // Undo moves the folder to the trash; redo brings it back.
        let report = extract_here(&w.path("pack.zip"), &go());
        assert_eq!(report.change.steps, [created(w.path("pack copy 5"))]);
        let mut history = History::default();
        history.record(report.change);
        history.undo(&w.trash).unwrap().unwrap();
        assert!(!w.has("pack copy 5"));
        assert_eq!(w.read(".Trash/pack copy 5/pack/a.txt"), "alpha");
        history.redo(&w.trash).unwrap().unwrap();
        assert_eq!(w.read("pack copy 5/pack/a.txt"), "alpha");
    }

    #[test]
    fn an_archive_whose_items_would_land_outside_its_folder_is_refused_with_nothing_written() {
        let w = world();
        let src = w.folder("Src");
        w.file("Src/x.txt", "evil");
        std::os::unix::fs::symlink("../../outside", src.join("up")).unwrap();
        std::os::unix::fs::symlink("/etc", src.join("abs")).unwrap();
        std::os::unix::fs::symlink("sub/../x.txt", src.join("fine")).unwrap();
        let tar = |args: &[&str]| {
            let made = std::process::Command::new("/usr/bin/tar")
                .current_dir(&src)
                .args(args)
                .status()
                .unwrap();
            assert!(made.success());
        };
        let x = src.join("x.txt");
        tar(&["-c", "-f", "../slip.tar", "-s", ",^,../,", "x.txt"]);
        tar(&[
            "--format",
            "zip",
            "-c",
            "-f",
            "../slip.zip",
            "-s",
            ",^,../,",
            "x.txt",
        ]);
        tar(&["-c", "-P", "-f", "../absolute.tar", x.to_str().unwrap()]);
        tar(&["-c", "-f", "../up.tar", "x.txt", "up"]);
        tar(&["-c", "-f", "../abs.tar.gz", "-z", "x.txt", "abs"]);
        let outside = "Couldn't extract {}: it holds items that would land outside its folder.";
        let linked = "Couldn't extract {}: a link in it points outside its folder.";
        for (archive, why) in [
            ("slip.tar", outside),
            ("slip.zip", outside),
            ("absolute.tar", outside),
            ("up.tar", linked),
            ("abs.tar.gz", linked),
        ] {
            let report = extract_here(&w.path(archive), &go());
            assert_eq!(said(&report), Some(why.replace("{}", archive)), "{archive}");
            assert!(report.change.steps.is_empty());
            assert_eq!(
                w.names(""),
                [
                    ".Trash",
                    "Src",
                    "abs.tar.gz",
                    "absolute.tar",
                    "slip.tar",
                    "slip.zip",
                    "up.tar"
                ],
                "{archive}"
            );
            assert!(!w.root.parent().unwrap().join("x.txt").exists());
        }
        // A link that stays inside is fine.
        tar(&["-c", "-f", "../fine.tar", "x.txt", "fine"]);
        let report = extract_here(&w.path("fine.tar"), &go());
        assert_eq!(said(&report), None);
        assert_eq!(w.read("fine/x.txt"), "evil");
    }

    #[test]
    fn an_extract_that_fails_or_is_stopped_leaves_nothing_behind() {
        let w = world();
        let archive = w.file("pack.zip", "PK not really");
        // Damaged, or locked with a password: said, and nothing is left.
        let report = extract_here(&archive, &go());
        assert_eq!(
            said(&report),
            Some("Couldn't extract pack.zip: it is damaged or needs a password.".to_string())
        );
        assert_eq!(w.names(""), [".Trash", "pack.zip"]);

        // Stopped halfway: the part written so far goes too.
        let stop = AtomicBool::new(false);
        let report = extract(
            &archive,
            &stop,
            || Ok(vec!["a.txt".to_string(), "b.txt".to_string()]),
            |into| {
                fs::write(into.join("a.txt"), "half")?;
                stop.store(true, Ordering::Relaxed);
                Err(io::ErrorKind::Interrupted.into())
            },
        );
        assert!(report.stopped);
        assert!(report.change.steps.is_empty());
        assert_eq!(w.names(""), [".Trash", "pack.zip"]);
    }

    #[test]
    fn an_archive_is_known_by_its_ending_and_unpacks_under_its_name_without_it() {
        let cases = [
            ("pack.zip", Some((Format::Zip, "pack"))),
            ("Pack.TAR.GZ", Some((Format::TarGz, "Pack"))),
            ("pack.tgz", Some((Format::TarGz, "pack"))),
            ("pack.tar.bz2", Some((Format::TarBz2, "pack"))),
            ("pack.tar.xz", Some((Format::TarXz, "pack"))),
            ("pack.tar", Some((Format::Tar, "pack"))),
            ("pack.7z", Some((Format::SevenZ, "pack"))),
            ("notes.txt", None),
            (".zip", None),
        ];
        for (name, expected) in cases {
            let found = Format::of_archive(name).map(|(format, stem)| (format, stem.to_string()));
            let expected = expected.map(|(format, stem)| (format, stem.to_string()));
            assert_eq!(found, expected, "{name}");
        }
    }
}
