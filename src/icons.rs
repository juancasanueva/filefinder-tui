//! The glyph in front of a name: filled Material Design glyphs from the Nerd
//! Font by default, each kind in its own colour; plain marks with
//! `FILEFINDER_ICONS=plain`.

use std::sync::OnceLock;

use ratatui::style::Color;

use crate::app::ViewMode;
use crate::model::{self, Category, Entry};
use crate::places::Kind;
use crate::theme;

pub const FOLDER: &str = "\u{f024b}";
pub const FILE: &str = "\u{f0214}";
pub const TEXT: &str = "\u{f0219}";
/// A folder that can't be read.
pub const LOCK: &str = "\u{f033e}";
/// Over a search or filter that matches nothing.
pub const SEARCH: &str = "\u{f0349}";
const LINK_TO_FOLDER: &str = "\u{f19ee}";
const LINK_TO_FILE: &str = "\u{f1177}";
const PROGRAM: &str = "\u{f08c6}";
const SETTINGS: &str = "\u{f107b}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSet {
    Nerd,
    Plain,
}

static ICON_SET: OnceLock<IconSet> = OnceLock::new();

/// `FILEFINDER_ICONS=plain` swaps the Nerd Font glyphs for plain marks.
pub fn init_from_env() {
    let plain = std::env::var("FILEFINDER_ICONS").is_ok_and(|value| value == "plain");
    let _ = ICON_SET.set(if plain { IconSet::Plain } else { IconSet::Nerd });
}

/// The glyphs in use.
pub fn set() -> IconSet {
    *ICON_SET.get().unwrap_or(&IconSet::Nerd)
}

/// Whether the Nerd Font glyphs are in use; the plain set has no marks for
/// the preview's head.
pub fn has_marks() -> bool {
    set() == IconSet::Nerd
}

/// A mark that has no plain glyph of its own: the lock, the search mark, the
/// empty folder's. `plain` stands in for it.
pub fn mark(glyph: &'static str, plain: &'static str) -> &'static str {
    if has_marks() { glyph } else { plain }
}

/// The glyph in front of an item's name.
pub fn of(entry: &Entry) -> &'static str {
    glyph_in(
        set(),
        &entry.name,
        entry.is_dir,
        entry.link.is_some(),
        entry.mode,
    )
}

/// The colour of the glyph in front of an item's name.
pub fn tint(entry: &Entry) -> Color {
    tint_of(&entry.name, entry.is_dir, entry.link.is_some(), entry.mode)
}

/// What kind of glyph an item gets; it decides the glyph's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Folder,
    Link,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Pdf,
    Program,
    Plain,
}

/// The glyph and its shape for an item by its name, whether it is a folder
/// or a link, and its mode.
fn shape_of(name: &str, is_dir: bool, is_link: bool, mode: u32) -> (&'static str, Shape) {
    if is_link {
        let glyph = if is_dir { LINK_TO_FOLDER } else { LINK_TO_FILE };
        return (glyph, Shape::Link);
    }
    if is_dir {
        return (FOLDER, Shape::Folder);
    }
    let extension = model::extension(name).unwrap_or_default();
    let code = |glyph| (glyph, Shape::Code);
    match extension.as_str() {
        "rs" => code("\u{f1617}"),
        "py" | "pyw" => code("\u{f0320}"),
        "md" | "markdown" => code("\u{f0354}"),
        "js" | "mjs" | "jsx" => code("\u{f031e}"),
        "ts" | "tsx" => code("\u{f06e6}"),
        "json" => code("\u{f0626}"),
        "sh" | "bash" | "zsh" | "fish" => code("\u{f018d}"),
        "c" | "h" => code("\u{f0671}"),
        "cpp" | "cc" | "cxx" | "hpp" => code("\u{f0672}"),
        "go" => code("\u{f07d3}"),
        "html" | "htm" => code("\u{f031d}"),
        "css" | "scss" => code("\u{f031c}"),
        "lua" => code("\u{f08b1}"),
        "toml" | "yaml" | "yml" | "ini" | "conf" => code(SETTINGS),
        "pdf" => ("\u{f0226}", Shape::Pdf),
        "xls" | "xlsx" | "ods" | "csv" => ("\u{f021b}", Shape::Code),
        "ppt" | "pptx" | "odp" | "key" => ("\u{f0227}", Shape::Pdf),
        "doc" | "docx" => ("\u{f022c}", Shape::Plain),
        "stl" | "obj" | "3mf" | "step" | "stp" | "gcode" | "bgcode" => {
            ("\u{f01a7}", Shape::Archive)
        }
        "dat" | "bin" => (PROGRAM, Shape::Program),
        _ => match model::category(name, false) {
            Category::Audio => ("\u{f0223}", Shape::Audio),
            Category::Video => ("\u{f022b}", Shape::Video),
            Category::Image => ("\u{f021f}", Shape::Image),
            Category::Archive => ("\u{f05c4}", Shape::Archive),
            Category::Text => (TEXT, Shape::Plain),
            _ if matches!(extension.as_str(), "odt" | "rtf") => (TEXT, Shape::Plain),
            _ if mode & 0o111 != 0 => (PROGRAM, Shape::Program),
            _ => (FILE, Shape::Plain),
        },
    }
}

/// The glyph for an item by its name, whether it is a folder or a link, and
/// its mode, in a set of glyphs.
pub fn glyph_in(set: IconSet, name: &str, is_dir: bool, is_link: bool, mode: u32) -> &'static str {
    if set == IconSet::Plain {
        return if is_dir { "›" } else { " " };
    }
    shape_of(name, is_dir, is_link, mode).0
}

/// The colour of an item's glyph by its name, whether it is a folder or a
/// link, and its mode.
pub fn tint_of(name: &str, is_dir: bool, is_link: bool, mode: u32) -> Color {
    match shape_of(name, is_dir, is_link, mode).1 {
        Shape::Folder => theme::ICON_FOLDER,
        Shape::Link => theme::SYMLINK,
        Shape::Image => theme::ICON_IMAGE,
        Shape::Video => theme::ICON_VIDEO,
        Shape::Audio => theme::ICON_AUDIO,
        Shape::Archive => theme::ICON_ARCHIVE,
        Shape::Code => theme::ICON_CODE,
        Shape::Pdf => theme::ICON_PDF,
        Shape::Program => theme::EXECUTABLE,
        Shape::Plain => theme::MUTED,
    }
}

// The places panel's glyphs, Material Design: a house, a monitor, a folder
// of text, a folder with a download arrow, of music, of pictures, a film, a
// briefcase, a bin and a hard disk.
pub const PLACE_APPLICATIONS: &str = "\u{f003b}";
pub const PLACE_HOME: &str = "\u{f02dc}";
pub const PLACE_DESKTOP: &str = "\u{f0379}";
pub const PLACE_DOCUMENTS: &str = "\u{f0c82}";
pub const PLACE_DOWNLOADS: &str = "\u{f024d}";
pub const PLACE_MUSIC: &str = "\u{f1359}";
pub const PLACE_PICTURES: &str = "\u{f024f}";
pub const PLACE_MOVIES: &str = "\u{f0381}";
pub const PLACE_PROJECTS: &str = "\u{f00d6}";
pub const PLACE_TRASH: &str = "\u{f0a79}";
pub const PLACE_DRIVE: &str = "\u{f02ca}";
pub const PLACE_CLOUD: &str = "\u{f015f}";

/// The glyph in front of a place's name in the panel.
pub fn place(kind: Kind) -> &'static str {
    place_in(set(), kind)
}

/// A place's glyph in a set of glyphs; the plain set has `~` for home, `×`
/// for the trash and `›` for the folders and drives.
pub fn place_in(set: IconSet, kind: Kind) -> &'static str {
    match (set, kind) {
        (IconSet::Plain, Kind::Home) => "~",
        (IconSet::Plain, Kind::Trash) => "×",
        (IconSet::Plain, _) => "›",
        (IconSet::Nerd, Kind::Applications) => PLACE_APPLICATIONS,
        (IconSet::Nerd, Kind::Home) => PLACE_HOME,
        (IconSet::Nerd, Kind::Desktop) => PLACE_DESKTOP,
        (IconSet::Nerd, Kind::Documents) => PLACE_DOCUMENTS,
        (IconSet::Nerd, Kind::Downloads) => PLACE_DOWNLOADS,
        (IconSet::Nerd, Kind::Music) => PLACE_MUSIC,
        (IconSet::Nerd, Kind::Pictures) => PLACE_PICTURES,
        (IconSet::Nerd, Kind::Movies) => PLACE_MOVIES,
        (IconSet::Nerd, Kind::Projects) => PLACE_PROJECTS,
        (IconSet::Nerd, Kind::Trash) => PLACE_TRASH,
        (IconSet::Nerd, Kind::Drive) => PLACE_DRIVE,
        (IconSet::Nerd, Kind::Cloud) => PLACE_CLOUD,
    }
}

/// The colour of a place's glyph: the folders' blue, the media folders the
/// colour of what they hold, the trash and the drives muted.
pub fn place_tint(kind: Kind) -> Color {
    match kind {
        Kind::Applications | Kind::Home | Kind::Desktop | Kind::Documents | Kind::Downloads => {
            theme::ICON_FOLDER
        }
        Kind::Music => theme::ICON_AUDIO,
        Kind::Pictures => theme::ICON_IMAGE,
        Kind::Movies => theme::ICON_VIDEO,
        Kind::Projects => theme::ICON_CODE,
        Kind::Trash | Kind::Drive | Kind::Cloud => theme::MUTED,
    }
}

// The header's view buttons, Material Design: a list, columns, a grid and a
// window split in two.
pub const VIEW_LIST: &str = "\u{f0572}";
pub const VIEW_COLUMNS: &str = "\u{f056d}";
pub const VIEW_GRID: &str = "\u{f0570}";
pub const VIEW_DUAL: &str = "\u{f0bcc}";

/// The glyph of a view's button in the header.
pub fn view(view: ViewMode) -> &'static str {
    view_in(set(), view)
}

/// A view button's glyph in a set of glyphs.
pub fn view_in(set: IconSet, view: ViewMode) -> &'static str {
    match (set, view) {
        (IconSet::Plain, ViewMode::List) => "≡",
        (IconSet::Plain, ViewMode::Columns) => "∥",
        (IconSet::Plain, ViewMode::Grid) => "▦",
        (IconSet::Plain, ViewMode::Dual) => "◫",
        (IconSet::Nerd, ViewMode::List) => VIEW_LIST,
        (IconSet::Nerd, ViewMode::Columns) => VIEW_COLUMNS,
        (IconSet::Nerd, ViewMode::Grid) => VIEW_GRID,
        (IconSet::Nerd, ViewMode::Dual) => VIEW_DUAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    fn nerd(name: &str) -> &'static str {
        glyph_in(IconSet::Nerd, name, false, false, 0o644)
    }

    #[test]
    fn folders_files_and_links_have_filled_material_glyphs() {
        assert_eq!(
            glyph_in(IconSet::Nerd, "Trip", true, false, 0o755),
            "\u{f024b}"
        );
        assert_eq!(nerd("README"), "\u{f0214}");
        assert_eq!(
            glyph_in(IconSet::Nerd, "to", true, true, 0o755),
            "\u{f19ee}"
        );
        assert_eq!(
            glyph_in(IconSet::Nerd, "to.txt", false, true, 0o644),
            "\u{f1177}"
        );
    }

    #[test]
    fn extensions_and_kinds_pick_their_glyph() {
        for (name, glyph) in [
            ("main.rs", "\u{f1617}"),
            ("a.py", "\u{f0320}"),
            ("notes.md", "\u{f0354}"),
            ("a.js", "\u{f031e}"),
            ("a.ts", "\u{f06e6}"),
            ("a.json", "\u{f0626}"),
            ("run.zsh", "\u{f018d}"),
            ("a.h", "\u{f0671}"),
            ("a.cpp", "\u{f0672}"),
            ("a.go", "\u{f07d3}"),
            ("index.html", "\u{f031d}"),
            ("a.css", "\u{f031c}"),
            ("a.lua", "\u{f08b1}"),
            ("Cargo.toml", "\u{f107b}"),
            ("a.yml", "\u{f107b}"),
            ("a.pdf", "\u{f0226}"),
            ("a.csv", "\u{f021b}"),
            ("a.pptx", "\u{f0227}"),
            ("letter.docx", "\u{f022c}"),
            ("part.stl", "\u{f01a7}"),
            ("blob.bin", "\u{f08c6}"),
            ("song.mp3", "\u{f0223}"),
            ("clip.mov", "\u{f022b}"),
            ("a.jpg", "\u{f021f}"),
            ("pack.zip", "\u{f05c4}"),
            ("notes.txt", "\u{f0219}"),
        ] {
            assert_eq!(nerd(name), glyph, "{name}");
        }
        // Anything else that runs is a program.
        assert_eq!(
            glyph_in(IconSet::Nerd, "tool", false, false, 0o755),
            "\u{f08c6}"
        );
    }

    #[test]
    fn each_kind_of_item_has_its_own_colour() {
        let tint = |name: &str, is_dir: bool, is_link: bool, mode: u32| {
            tint_of(name, is_dir, is_link, mode)
        };
        assert_eq!(tint("Trip", true, false, 0o755), theme::ICON_FOLDER);
        assert_eq!(tint("a.jpg", false, false, 0o644), theme::ICON_IMAGE);
        assert_eq!(tint("clip.mov", false, false, 0o644), theme::ICON_VIDEO);
        assert_eq!(tint("song.mp3", false, false, 0o644), theme::ICON_AUDIO);
        assert_eq!(tint("pack.zip", false, false, 0o644), theme::ICON_ARCHIVE);
        assert_eq!(tint("main.rs", false, false, 0o644), theme::ICON_CODE);
        assert_eq!(tint("a.pdf", false, false, 0o644), theme::ICON_PDF);
        assert_eq!(tint("notes.txt", false, false, 0o644), theme::MUTED);
        assert_eq!(tint("README", false, false, 0o644), theme::MUTED);
        assert_eq!(tint("tool", false, false, 0o755), theme::EXECUTABLE);
        assert_eq!(tint("to", true, true, 0o755), theme::SYMLINK);
        // The five tones are told apart from each other and from the text.
        let tones = [
            theme::ICON_FOLDER,
            theme::ICON_IMAGE,
            theme::ICON_VIDEO,
            theme::ICON_AUDIO,
            theme::ICON_ARCHIVE,
            theme::ICON_CODE,
        ];
        for (i, a) in tones.iter().enumerate() {
            assert_ne!(*a, theme::FOREGROUND);
            for b in &tones[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn places_and_view_buttons_have_their_own_glyphs_and_plain_marks() {
        use crate::app::ViewMode;
        use crate::places::Kind;
        for (kind, nerd, plain, tint) in [
            (Kind::Applications, "\u{f003b}", "›", theme::ICON_FOLDER),
            (Kind::Home, "\u{f02dc}", "~", theme::ICON_FOLDER),
            (Kind::Desktop, "\u{f0379}", "›", theme::ICON_FOLDER),
            (Kind::Documents, "\u{f0c82}", "›", theme::ICON_FOLDER),
            (Kind::Downloads, "\u{f024d}", "›", theme::ICON_FOLDER),
            (Kind::Music, "\u{f1359}", "›", theme::ICON_AUDIO),
            (Kind::Pictures, "\u{f024f}", "›", theme::ICON_IMAGE),
            (Kind::Movies, "\u{f0381}", "›", theme::ICON_VIDEO),
            (Kind::Projects, "\u{f00d6}", "›", theme::ICON_CODE),
            (Kind::Trash, "\u{f0a79}", "×", theme::MUTED),
            (Kind::Drive, "\u{f02ca}", "›", theme::MUTED),
            (Kind::Cloud, "\u{f015f}", "›", theme::MUTED),
        ] {
            assert_eq!(place_in(IconSet::Nerd, kind), nerd, "{kind:?}");
            assert_eq!(place_in(IconSet::Plain, kind), plain, "{kind:?}");
            assert_eq!(place_tint(kind), tint, "{kind:?}");
        }
        for (view, nerd, plain) in [
            (ViewMode::List, "\u{f0572}", "≡"),
            (ViewMode::Columns, "\u{f056d}", "∥"),
            (ViewMode::Grid, "\u{f0570}", "▦"),
            (ViewMode::Dual, "\u{f0bcc}", "◫"),
        ] {
            assert_eq!(view_in(IconSet::Nerd, view), nerd, "{view:?}");
            assert_eq!(view_in(IconSet::Plain, view), plain, "{view:?}");
        }
    }

    #[test]
    fn the_plain_set_marks_folders_and_leaves_files_blank() {
        assert_eq!(glyph_in(IconSet::Plain, "Trip", true, false, 0o755), "›");
        assert_eq!(glyph_in(IconSet::Plain, "to", true, true, 0o755), "›");
        assert_eq!(glyph_in(IconSet::Plain, "a.jpg", false, false, 0o644), " ");
    }
}
