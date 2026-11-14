//! Pictures of files: thumbnails made off the UI thread and kept ready to draw,
//! and what a file's header and first lines say about it.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader};
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FilterType, Resize};

use crate::model::{self, Category, Entry, SortMode};
use crate::system::quiet;

/// The longest side of a thumbnail, in pixels: enough for the preview pane.
const THUMB_PIXELS: u32 = 512;
/// Pictures kept ready to draw, each fitted to one size in cells.
const READY: usize = 256;
/// Requests waiting for the worker; the oldest give way.
const WAITING: usize = 400;
/// Thumbnails the worker keeps to fit again at another size.
const SOURCES: usize = 48;
/// Files remembered as having no picture.
const FAILED: usize = 4000;
/// How long Quick Look gets for one file before it is given up on.
const QUICK_LOOK_PATIENCE: Duration = Duration::from_secs(4);
/// How much of a text file is read for its opening lines.
const OPENING_BYTES: u64 = 4096;
const OPENING_LINES: usize = 40;
/// The most items of a folder the preview lists.
const CHILDREN: usize = 1000;

/// The port that turns a file into a small picture. It is called on a worker
/// thread, never while drawing.
pub trait Thumbnailer: Send + Sync {
    /// A picture of the file, its longest side about 512 pixels at most, or
    /// `None` when there is none to show.
    fn thumbnail(&self, path: &Path) -> Option<DynamicImage>;
}

/// Decodes common pictures itself and asks Quick Look for the rest.
pub struct MacThumbnailer {
    /// A folder of this app's own for Quick Look to write into.
    scratch: PathBuf,
    runs: AtomicU64,
}

impl MacThumbnailer {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            scratch: cache_dir.join("quicklook"),
            runs: AtomicU64::new(0),
        }
    }

    /// Has `qlmanage` draw the file into a folder of its own and reads the result.
    fn quick_look(&self, path: &Path) -> Option<DynamicImage> {
        self.in_scratch(|out| quick_look_into(path, out))
    }

    /// Runs `make` with a folder of its own to write into, removed after.
    fn in_scratch(&self, make: impl FnOnce(&Path) -> Option<DynamicImage>) -> Option<DynamicImage> {
        let run = self.runs.fetch_add(1, Ordering::Relaxed);
        let out = self.scratch.join(format!("{}-{run}", process::id()));
        fs::create_dir_all(&out).ok()?;
        let image = make(&out);
        let _ = fs::remove_dir_all(&out);
        image
    }
}

impl Thumbnailer for MacThumbnailer {
    fn thumbnail(&self, path: &Path) -> Option<DynamicImage> {
        let name = path.file_name()?.to_string_lossy();
        let extension = model::extension(&name).unwrap_or_default();
        match model::category(&name, false) {
            Category::Image if DECODED_HERE.contains(&extension.as_str()) => {
                // A picture this can't read may still be one Quick Look can.
                decode(path).or_else(|| self.quick_look(path))
            }
            Category::Image | Category::Video | Category::Pdf => self.quick_look(path),
            Category::App => self.in_scratch(|out| app_icon_into(path, out)),
            _ if QUICK_LOOK_DOCUMENTS.contains(&extension.as_str()) => self.quick_look(path),
            _ => None,
        }
    }
}

/// The pictures the `image` crate reads.
const DECODED_HERE: [&str; 9] = [
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "ico",
];

/// Documents Quick Look draws a first page of. Kinds it doesn't know are left
/// alone: `qlmanage` waits on some of them for good.
const QUICK_LOOK_DOCUMENTS: [&str; 10] = [
    "rtf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers", "key",
];

fn decode(path: &Path) -> Option<DynamicImage> {
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    // A photo taken with the camera on its side is stored that way.
    image.apply_orientation(orientation);
    Some(shrink(image))
}

fn shrink(image: DynamicImage) -> DynamicImage {
    if image.width().max(image.height()) > THUMB_PIXELS {
        image.thumbnail(THUMB_PIXELS, THUMB_PIXELS)
    } else {
        image
    }
}

/// `qlmanage -t` writes `<out>/<file name>.png`.
fn quick_look_into(path: &Path, out: &Path) -> Option<DynamicImage> {
    let mut command = Command::new("qlmanage");
    command
        .args(["-t", "-s", &THUMB_PIXELS.to_string(), "-o"])
        .arg(out)
        .arg(path);
    let child = quiet(&mut command).spawn().ok()?;
    if !finished_in_time(child, QUICK_LOOK_PATIENCE) {
        return None;
    }
    let mut name = path.file_name()?.to_os_string();
    name.push(".png");
    image::open(out.join(name)).ok().map(shrink)
}

/// An app's icon: the `.icns` file its Info.plist names, which `sips`
/// turns into `<out>/icon.png`. Apps that keep their icon only in an asset
/// catalog have none here. Quick Look isn't asked: it can wait on an app
/// for good.
fn app_icon_into(app: &Path, out: &Path) -> Option<DynamicImage> {
    let contents = app.join("Contents");
    let named = Command::new("plutil")
        .args(["-extract", "CFBundleIconFile", "raw", "-o", "-"])
        .arg(contents.join("Info.plist"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let name = String::from_utf8(named.stdout).ok()?;
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return None;
    }
    let file = if name.ends_with(".icns") {
        name.to_string()
    } else {
        format!("{name}.icns")
    };
    let icon = contents.join("Resources").join(file);
    if !icon.is_file() {
        return None;
    }
    let png = out.join("icon.png");
    let mut command = Command::new("sips");
    command
        .args(["-s", "format", "png", "-Z", &THUMB_PIXELS.to_string()])
        .arg(&icon)
        .arg("--out")
        .arg(&png);
    let child = quiet(&mut command).spawn().ok()?;
    // `sips` says it succeeded even when it wrote nothing: the picture decides.
    finished_in_time(child, QUICK_LOOK_PATIENCE);
    image::open(&png).ok().map(shrink)
}

/// Waits for a child, killing it when it takes too long.
fn finished_in_time(mut child: Child, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// One picture to make: a file as it was at a moment, fitted to a size in cells.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
    cells: (u16, u16),
}

struct Done {
    key: Key,
    protocol: Option<Protocol>,
}

#[derive(Default)]
struct Queue {
    /// The newest request is last, and is the next one made.
    waiting: Vec<Key>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Ready {
    protocol: Protocol,
    /// When it was last asked for, on the service's own clock.
    used: u64,
    /// Whether it has been on screen. A terminal with graphics is sent the
    /// picture the first time it is drawn, so that time must not be lost.
    seen: bool,
}

/// The thumbnail service. Drawing asks it for pictures that are ready and it
/// has a worker thread make the ones that aren't.
pub struct Thumbs {
    shared: Arc<Shared>,
    results: Receiver<Done>,
    ready: HashMap<Key, Ready>,
    /// Asked for and not back yet.
    pending: HashSet<Key>,
    /// Files with no picture, and when they were last modified then.
    failed: HashMap<PathBuf, Option<SystemTime>>,
    clock: u64,
    covered: bool,
    /// Whether the terminal draws pictures itself, not with half blocks.
    sharp: bool,
}

impl Thumbs {
    /// Starts the worker. The picker says how this terminal shows pictures.
    pub fn new(picker: Picker, thumbnailer: Arc<dyn Thumbnailer>) -> Self {
        let shared = Arc::new(Shared::default());
        let (sender, results) = mpsc::channel();
        let queue = shared.clone();
        let sharp = picker.protocol_type() != ProtocolType::Halfblocks;
        // Without its thread the service still answers: nothing is ever ready.
        let _ = thread::Builder::new()
            .name("thumbnails".to_string())
            .spawn(move || work(&queue, thumbnailer.as_ref(), &picker, &sender));
        Self {
            shared,
            results,
            ready: HashMap::new(),
            pending: HashSet::new(),
            failed: HashMap::new(),
            clock: 0,
            covered: false,
            sharp,
        }
    }

    /// Whether the terminal draws pictures itself, sharp enough to show one
    /// in the two cells of a row's glyph. Half blocks make a smudge of it.
    pub fn sharp(&self) -> bool {
        self.sharp
    }

    /// The picture of a file fitted into `size` cells when it is ready. When it
    /// isn't, it is asked for, ahead of everything asked for before.
    pub fn get(
        &mut self,
        path: &Path,
        modified: Option<SystemTime>,
        size: Size,
    ) -> Option<&Protocol> {
        if size.width == 0 || size.height == 0 || self.failed.get(path) == Some(&modified) {
            return None;
        }
        let key = Key {
            path: path.to_path_buf(),
            modified,
            cells: (size.width, size.height),
        };
        if !self.ready.contains_key(&key) {
            self.ask(key);
            return None;
        }
        self.clock += 1;
        let ready = self.ready.get_mut(&key)?;
        ready.used = self.clock;
        if self.covered && !ready.seen {
            return None;
        }
        ready.seen = true;
        Some(&ready.protocol)
    }

    /// While something is drawn over the window, pictures that haven't been on
    /// screen yet are kept back: their first drawing has to reach the terminal.
    pub fn cover(&mut self, covered: bool) {
        self.covered = covered;
    }

    /// Takes in what the worker has finished. Called from the event loop.
    pub fn collect(&mut self) {
        while let Ok(done) = self.results.try_recv() {
            self.pending.remove(&done.key);
            match done.protocol {
                Some(protocol) => {
                    self.failed.remove(&done.key.path);
                    self.keep(done.key, protocol);
                }
                None => {
                    if self.failed.len() >= FAILED {
                        self.failed.clear();
                    }
                    self.failed.insert(done.key.path, done.key.modified);
                }
            }
        }
    }

    /// Whether everything asked for has come back.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }

    fn ask(&mut self, key: Key) {
        let mut queue = self.shared.queue();
        if let Some(index) = queue.waiting.iter().position(|waiting| *waiting == key) {
            // Still wanted: back in front of what was asked for since.
            let key = queue.waiting.remove(index);
            queue.waiting.push(key);
        } else if !self.pending.contains(&key) {
            self.pending.insert(key.clone());
            queue.waiting.push(key);
            if queue.waiting.len() > WAITING {
                let oldest = queue.waiting.remove(0);
                self.pending.remove(&oldest);
            }
            self.shared.wake.notify_one();
        }
    }

    fn keep(&mut self, key: Key, protocol: Protocol) {
        if self.ready.len() >= READY {
            let oldest = self.ready.iter().min_by_key(|(_, ready)| ready.used);
            if let Some(oldest) = oldest.map(|(key, _)| key.clone()) {
                self.ready.remove(&oldest);
            }
        }
        self.clock += 1;
        let ready = Ready {
            protocol,
            used: self.clock,
            seen: false,
        };
        self.ready.insert(key, ready);
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        self.shared.queue().closed = true;
        self.shared.wake.notify_one();
    }
}

/// The worker: makes the newest request first until the service is dropped.
fn work(shared: &Shared, thumbnailer: &dyn Thumbnailer, picker: &Picker, results: &Sender<Done>) {
    // The thumbnails made lately, so another size is fitted from the same one.
    let mut sources: Vec<(PathBuf, Option<SystemTime>, Option<DynamicImage>)> = Vec::new();
    loop {
        let key = {
            let mut queue = shared.queue();
            loop {
                if queue.closed {
                    return;
                }
                if let Some(key) = queue.waiting.pop() {
                    break key;
                }
                queue = shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let known = sources
            .iter()
            .position(|(path, modified, _)| *path == key.path && *modified == key.modified);
        let source = match known {
            Some(index) => sources.remove(index),
            None => {
                let image = thumbnailer.thumbnail(&key.path);
                (key.path.clone(), key.modified, image)
            }
        };
        let protocol = source.2.as_ref().and_then(|image| {
            let size = Size::new(key.cells.0, key.cells.1);
            let fit = Resize::Fit(Some(FilterType::Triangle));
            picker.new_protocol(image.clone(), size, fit).ok()
        });
        sources.push(source);
        if sources.len() > SOURCES {
            sources.remove(0);
        }
        if results.send(Done { key, protocol }).is_err() {
            return;
        }
    }
}

/// What the preview pane shows of an item beyond its listing.
pub struct Facts {
    path: PathBuf,
    modified: Option<SystemTime>,
    /// Whether hidden items were listed among a folder's, and in what order.
    listed: (bool, SortMode),
    /// A picture's width and height in pixels.
    pub dimensions: Option<(u32, u32)>,
    /// The first lines of a text file; none for anything else.
    pub opening: Vec<String>,
    /// How many lines a text file has, for one small enough to count.
    pub lines: Option<usize>,
    /// A folder's items, the first thousand of them, or why they couldn't
    /// be listed; `None` for anything but a folder.
    pub children: Option<Result<Vec<Entry>, String>>,
}

impl Facts {
    /// Reads them from the item: a picture's header, a text's first few
    /// kilobytes, a folder's items listed the way the list lists them.
    pub fn read(entry: &Entry, show_hidden: bool, sort: SortMode) -> Self {
        let category = entry.category();
        // A text by its extension, or a file without one whose start is
        // text (`pinned`, `.zshrc`). Only a regular file is opened to look:
        // opening a pipe would wait for a writer.
        let regular = || std::fs::metadata(&entry.path).is_ok_and(|meta| meta.is_file());
        let opening = if is_plain_text(entry) {
            opening_lines(&entry.path, false)
        } else if !entry.is_dir && model::extension(&entry.name).is_none() && regular() {
            opening_lines(&entry.path, true)
        } else {
            Vec::new()
        };
        let text = is_plain_text(entry) || !opening.is_empty();
        Self {
            path: entry.path.clone(),
            modified: entry.modified,
            listed: (show_hidden, sort),
            dimensions: (category == Category::Image)
                .then(|| dimensions(&entry.path))
                .flatten(),
            opening,
            lines: (text && entry.size <= COUNTED_BYTES)
                .then(|| line_count(&entry.path))
                .flatten(),
            children: entry
                .is_dir
                .then(|| children(&entry.path, show_hidden, sort)),
        }
    }

    /// Whether they were read from this item as it is now, listed this way.
    pub fn describes(&self, entry: &Entry, show_hidden: bool, sort: SortMode) -> bool {
        self.path == entry.path
            && self.modified == entry.modified
            && self.listed == (show_hidden, sort)
    }
}

/// A folder's items for the preview, or why they can't be shown.
fn children(folder: &Path, show_hidden: bool, sort: SortMode) -> Result<Vec<Entry>, String> {
    match model::list_dir(folder, show_hidden, sort) {
        Ok(mut entries) => {
            entries.truncate(CHILDREN);
            Ok(entries)
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err("You don’t have permission to open this folder.".to_string())
        }
        Err(_) => Err("That folder could not be read.".to_string()),
    }
}

/// Text and code, which preview as their own first lines. Rich text reads as markup.
fn is_plain_text(entry: &Entry) -> bool {
    matches!(entry.category(), Category::Text | Category::Code)
        && model::extension(&entry.name).as_deref() != Some("rtf")
}

/// A picture's size from its header, the way round it is shown.
fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let reader = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let on_its_side = matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    Some(if on_its_side {
        (height, width)
    } else {
        (width, height)
    })
}

/// The most a text file can hold for its lines to be counted.
const COUNTED_BYTES: u64 = 8 << 20;

/// How many lines a file has: its line ends, and a last line without one.
fn line_count(path: &Path) -> Option<usize> {
    let bytes = std::fs::read(path).ok()?;
    let ends = bytes.iter().filter(|byte| **byte == b'\n').count();
    let unended = bytes.last().is_some_and(|byte| *byte != b'\n');
    Some(ends + usize::from(unended))
}

/// The first lines of a file that is text; nothing when it isn't UTF-8.
/// Sniffing a file nothing names as text, a NUL byte says it isn't.
fn opening_lines(path: &Path, sniffing: bool) -> Vec<String> {
    let mut bytes = Vec::new();
    let read = File::open(path).and_then(|file| file.take(OPENING_BYTES).read_to_end(&mut bytes));
    if read.is_err() || (sniffing && bytes.contains(&0)) {
        return Vec::new();
    }
    let text = match str::from_utf8(&bytes) {
        Ok(text) => text,
        // The cut may fall inside a character; anything else wrong isn't text.
        Err(error) if error.error_len().is_none() => {
            str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default()
        }
        Err(_) => return Vec::new(),
    };
    text.lines()
        .take(OPENING_LINES)
        .map(|line| {
            let line = line.replace('\t', "    ");
            line.chars().filter(|c| !c.is_control()).collect()
        })
        .collect()
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use image::{Rgb, RgbImage};
    use ratatui::buffer::Cell;
    use ratatui::style::Color;
    use std::sync::OnceLock;
    use std::thread::ThreadId;

    /// What the fake's pictures are painted with.
    const PAINT: [u8; 3] = [200, 60, 40];

    /// Draws every file as a plain rectangle, four by three; three by four when
    /// the file says `tall`, and not at all when it says `broken`.
    #[derive(Default)]
    pub struct FakeThumbnailer {
        asked: Mutex<Vec<PathBuf>>,
        /// The threads it was called on.
        pub threads: Mutex<Vec<ThreadId>>,
        held: Mutex<bool>,
        released: Condvar,
    }

    impl FakeThumbnailer {
        /// The names of the files it was asked to draw, in order.
        pub fn asked(&self) -> Vec<String> {
            let asked = self.asked.lock().unwrap();
            asked
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        }

        /// Makes the worker wait in its next thumbnail until `release`.
        pub fn hold(&self) {
            *self.held.lock().unwrap() = true;
        }

        pub fn release(&self) {
            *self.held.lock().unwrap() = false;
            self.released.notify_all();
        }
    }

    impl Thumbnailer for FakeThumbnailer {
        fn thumbnail(&self, path: &Path) -> Option<DynamicImage> {
            self.asked.lock().unwrap().push(path.to_path_buf());
            self.threads.lock().unwrap().push(thread::current().id());
            let mut held = self.held.lock().unwrap();
            while *held {
                held = self.released.wait(held).unwrap();
            }
            drop(held);
            let (width, height) = match fs::read(path).ok()?.as_slice() {
                b"broken" => return None,
                b"tall" => (300, 400),
                _ => (400, 300),
            };
            Some(RgbImage::from_pixel(width, height, Rgb(PAINT)).into())
        }
    }

    /// A terminal without graphics: pictures become colored half blocks.
    pub fn picker() -> Picker {
        // Made once: inside tmux the picker runs a `tmux` command when it is made.
        static PICKER: OnceLock<Picker> = OnceLock::new();
        PICKER.get_or_init(Picker::halfblocks).clone()
    }

    /// Whether a cell shows a piece of one of the fake's pictures.
    pub fn painted(cell: &Cell) -> bool {
        let near = |color| match color {
            Color::Rgb(r, g, b) => [r, g, b].iter().zip(PAINT).all(|(c, p)| c.abs_diff(p) <= 2),
            _ => false,
        };
        near(cell.bg) && (cell.symbol() == " " || near(cell.fg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, RgbaImage};

    fn entry(path: PathBuf) -> Entry {
        Entry::from_path(path).unwrap()
    }

    #[test]
    fn a_text_s_lines_are_counted_with_a_last_one_without_its_end() {
        let dir = tempfile::tempdir().unwrap();
        for (text, lines) in [("a\nb\nc", 3), ("a\nb\n", 2), ("", 0), ("one", 1)] {
            let path = dir.path().join("t.txt");
            fs::write(&path, text).unwrap();
            assert_eq!(read(&entry(path.clone())).lines, Some(lines), "{text:?}");
        }
        // Not text: nothing counted.
        let path = dir.path().join("p.png");
        fs::write(&path, "a\nb").unwrap();
        assert_eq!(read(&entry(path.clone())).lines, None);
    }

    #[test]
    fn the_mac_thumbnailer_reads_pictures_itself_and_makes_them_small() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let thumbnailer = MacThumbnailer::new(&cache);
        let picture = dir.path().join("wide.png");
        GrayImage::new(2000, 1000).save(&picture).unwrap();
        let thumbnail = thumbnailer.thumbnail(&picture).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (512, 256));

        let small = dir.path().join("small.png");
        GrayImage::new(40, 30).save(&small).unwrap();
        let thumbnail = thumbnailer.thumbnail(&small).unwrap();
        assert_eq!((thumbnail.width(), thumbnail.height()), (40, 30));

        // Text, sound, archives and kinds nobody knows have no picture, and
        // Quick Look isn't bothered with them.
        for name in [
            "notes.txt",
            "main.rs",
            "song.mp3",
            "all.zip",
            "data.xyz",
            "README",
        ] {
            let file = dir.path().join(name);
            fs::write(&file, "x").unwrap();
            assert!(thumbnailer.thumbnail(&file).is_none(), "{name}");
        }
        assert!(!cache.exists(), "nothing was written anywhere");
    }

    /// An app bundle whose Info.plist names `icon` as its icon file, with
    /// that icon made from a picture `side` pixels square when given.
    fn app(dir: &Path, name: &str, icon: &str, side: Option<u32>) -> PathBuf {
        let bundle = dir.join(name);
        let contents = bundle.join("Contents");
        fs::create_dir_all(contents.join("Resources")).unwrap();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <plist version=\"1.0\"><dict>\
             <key>CFBundleIconFile</key><string>{icon}</string>\
             </dict></plist>\n"
        );
        fs::write(contents.join("Info.plist"), plist).unwrap();
        if let Some(side) = side {
            // `iconutil` makes an icon from a folder of pictures named by size.
            let set = dir.join(format!("{name}.iconset"));
            fs::create_dir_all(&set).unwrap();
            let png = set.join(format!("icon_{}x{}@2x.png", side / 2, side / 2));
            RgbaImage::new(side, side).save(&png).unwrap();
            let file = if icon.ends_with(".icns") {
                icon.to_string()
            } else {
                format!("{icon}.icns")
            };
            let status = Command::new("iconutil")
                .args(["-c", "icns"])
                .arg(&set)
                .arg("-o")
                .arg(contents.join("Resources").join(file))
                .status()
                .unwrap();
            assert!(status.success());
        }
        bundle
    }

    #[test]
    fn an_app_shows_the_icon_its_info_plist_names() {
        let dir = tempfile::tempdir().unwrap();
        let thumbnailer = MacThumbnailer::new(&dir.path().join("cache"));
        // The name with or without its extension, as apps write it.
        for icon in ["AppIcon", "Tool.icns"] {
            let bundle = app(dir.path(), &format!("{icon}.app"), icon, Some(1024));
            let picture = thumbnailer.thumbnail(&bundle).expect(icon);
            assert_eq!((picture.width(), picture.height()), (512, 512), "{icon}");
        }
        // An icon file that isn't there: no picture.
        let missing = app(dir.path(), "Missing.app", "Gone", None);
        assert!(thumbnailer.thumbnail(&missing).is_none());
        // No scratch is left behind.
        let left = fs::read_dir(dir.path().join("cache/quicklook")).map_or(0, |d| d.count());
        assert_eq!(left, 0);
    }

    #[test]
    fn a_picture_says_its_dimensions_from_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let picture = dir.path().join("harbour.png");
        GrayImage::new(1600, 1200).save(&picture).unwrap();
        assert_eq!(read(&entry(picture)).dimensions, Some((1600, 1200)));

        // Named like a picture without being one, and a text file.
        let fake = dir.path().join("fake.jpg");
        fs::write(&fake, "jpg").unwrap();
        assert_eq!(read(&entry(fake)).dimensions, None);
        let text = dir.path().join("notes.txt");
        fs::write(&text, "hello").unwrap();
        assert_eq!(read(&entry(text)).dimensions, None);
    }

    fn read(entry: &Entry) -> Facts {
        Facts::read(entry, false, SortMode::Name)
    }

    #[test]
    fn a_folder_lists_its_items_or_says_why_it_cannot() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Trip");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("b.txt"), "b").unwrap();
        fs::write(folder.join(".hidden"), "").unwrap();
        fs::create_dir(folder.join("Day")).unwrap();
        let names = |facts: &Facts| -> Vec<String> {
            let listed = facts.children.as_ref().unwrap().as_ref().unwrap();
            listed.iter().map(|entry| entry.name.clone()).collect()
        };
        let trip = entry(folder.clone());
        assert_eq!(names(&read(&trip)), ["Day", "b.txt"]);
        let all = Facts::read(&trip, true, SortMode::Name);
        assert_eq!(names(&all), ["Day", ".hidden", "b.txt"]);
        assert!(all.describes(&trip, true, SortMode::Name));
        assert!(!all.describes(&trip, false, SortMode::Name));
        assert!(read(&entry(folder.join("b.txt"))).children.is_none());

        fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).unwrap();
        let locked = read(&trip).children;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            locked.unwrap().unwrap_err(),
            "You don’t have permission to open this folder."
        );
    }

    #[test]
    fn opening_lines_come_from_a_bounded_start_of_text_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let facts = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            read(&entry(path))
        };
        let notes = facts("notes.md", b"# Title\r\n\tindented\n\nlast");
        assert_eq!(notes.opening, ["# Title", "    indented", "", "last"]);
        assert_eq!(
            facts("main.rs", b"fn main() {}\n").opening,
            ["fn main() {}"]
        );

        // One enormous line: only its first four kilobytes are read.
        let long = facts("long.txt", "x".repeat(100_000).as_bytes());
        assert_eq!(long.opening.len(), 1);
        assert_eq!(long.opening[0].len(), 4096);
        // The cut falls inside the last `é`: the text before it still counts.
        let accents = facts("accents.txt", format!("x{}", "é".repeat(3000)).as_bytes());
        assert_eq!(accents.opening, [format!("x{}", "é".repeat(2047))]);
        let many = facts("many.txt", "line\n".repeat(500).as_bytes());
        assert_eq!(many.opening.len(), 40);

        // Not UTF-8, not a text file, rich text: no lines.
        assert!(facts("latin.txt", b"caf\xe9 au lait\n").opening.is_empty());
        assert!(facts("data.bin", b"plain enough\n").opening.is_empty());
        assert!(facts("letter.rtf", b"{\\rtf1 Hello}").opening.is_empty());
        assert!(facts("empty.txt", b"").opening.is_empty());
    }

    #[test]
    fn a_file_without_an_extension_shows_its_lines_when_it_is_text() {
        let dir = tempfile::tempdir().unwrap();
        let facts = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            read(&entry(path))
        };
        let pinned = facts("pinned", b"/Users/me/Projects\n/Users/me/Music\n");
        assert_eq!(pinned.opening, ["/Users/me/Projects", "/Users/me/Music"]);
        assert_eq!(pinned.lines, Some(2));
        let rc = facts(".zshrc", b"export EDITOR=nvim\n");
        assert_eq!(rc.opening, ["export EDITOR=nvim"]);

        // A program, bytes that aren't UTF-8, or nothing at all: no lines.
        let program = facts("tool", b"\xcf\xfa\xed\xfe\x07\x00\x00\x01");
        assert!(program.opening.is_empty());
        assert_eq!(program.lines, None);
        assert!(facts("zeros", b"ab\0cd\n").opening.is_empty());
        assert!(facts("latin", b"caf\xe9\n").opening.is_empty());
        assert!(facts("blank", b"").opening.is_empty());
        // An unknown extension is still not read.
        assert!(facts("data.bin", b"plain enough\n").opening.is_empty());

        // A pipe is never opened: that would wait for a writer.
        let pipe = dir.path().join("pipe");
        let name = std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read(&entry(pipe)).opening.is_empty());
    }
}
