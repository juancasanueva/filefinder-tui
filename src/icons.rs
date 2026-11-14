//! The glyph in front of a name: flea's Nerd Font Codicons by default, plain
//! marks with `FILEFINDER_ICONS=plain`.

use std::sync::OnceLock;

use crate::app::ViewMode;
use crate::model::{self, Category, Entry};
use crate::places::Kind;

pub const FOLDER: &str = "\u{ea83}";
pub const FILE: &str = "\u{ea7b}";
pub const TEXT: &str = "\u{ec5e}";
/// A folder that can't be read.
pub const LOCK: &str = "\u{ea75}";
/// Over a search or filter that matches nothing.
pub const SEARCH: &str = "\u{ea6d}";
const LINK_TO_FOLDER: &str = "\u{eaed}";
const LINK_TO_FILE: &str = "\u{eaee}";
const BINARY: &str = "\u{eae8}";

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

/// The glyph for an item by its name, whether it is a folder or a link, and
/// its mode, in a set of glyphs.
pub fn glyph_in(set: IconSet, name: &str, is_dir: bool, is_link: bool, mode: u32) -> &'static str {
    if set == IconSet::Plain {
        return if is_dir { "›" } else { " " };
    }
    if is_link {
        return if is_dir { LINK_TO_FOLDER } else { LINK_TO_FILE };
    }
    if is_dir {
        return FOLDER;
    }
    let extension = model::extension(name).unwrap_or_default();
    match extension.as_str() {
        "rs" => "\u{e68b}",
        "py" | "pyw" => "\u{e606}",
        "md" | "markdown" => "\u{eb1d}",
        "js" | "mjs" | "jsx" => "\u{e60c}",
        "ts" | "tsx" => "\u{e628}",
        "json" => "\u{e60b}",
        "sh" | "bash" | "zsh" | "fish" => "\u{e691}",
        "c" | "h" => "\u{e649}",
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e646}",
        "go" => "\u{e627}",
        "html" | "htm" => "\u{e60e}",
        "css" | "scss" => "\u{e614}",
        "lua" => "\u{e620}",
        "toml" => "\u{e615}",
        "yaml" | "yml" => "\u{e6a8}",
        "pdf" => "\u{eaeb}",
        "xls" | "xlsx" | "ods" | "csv" => "\u{ebb7}",
        "ppt" | "pptx" | "odp" => "\u{f0428}",
        "stl" | "obj" | "3mf" | "step" | "stp" | "gcode" | "bgcode" => "\u{eb29}",
        "dat" | "bin" => BINARY,
        _ => match model::category(name, false) {
            Category::Audio => "\u{ec1b}",
            Category::Video => "\u{ead9}",
            Category::Image => "\u{eaea}",
            Category::Archive => "\u{eaef}",
            Category::Text => TEXT,
            _ if matches!(extension.as_str(), "doc" | "docx" | "odt" | "rtf") => TEXT,
            _ if mode & 0o111 != 0 => BINARY,
            _ => FILE,
        },
    }
}

// The places panel's glyphs, Codicons: home, a screen, a text page, a
// download, music, a picture, a video camera, a project board, a bin and a
// server for a drive.
pub const PLACE_HOME: &str = "\u{eb06}";
pub const PLACE_DESKTOP: &str = "\u{ea7a}";
pub const PLACE_DOCUMENTS: &str = TEXT;
pub const PLACE_DOWNLOADS: &str = "\u{ea78}";
pub const PLACE_MUSIC: &str = "\u{ec1b}";
pub const PLACE_PICTURES: &str = "\u{eaea}";
pub const PLACE_MOVIES: &str = "\u{ead9}";
pub const PLACE_PROJECTS: &str = "\u{eb30}";
pub const PLACE_TRASH: &str = "\u{ea81}";
pub const PLACE_DRIVE: &str = "\u{eb50}";

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
    }
}

// The header's view buttons, Codicons: a list, an editor layout for the
// columns, four squares for the grid, a window split in two.
pub const VIEW_LIST: &str = "\u{eb17}";
pub const VIEW_COLUMNS: &str = "\u{eae3}";
pub const VIEW_GRID: &str = "\u{eae6}";
pub const VIEW_DUAL: &str = "\u{eb56}";

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

    fn nerd(name: &str) -> &'static str {
        glyph_in(IconSet::Nerd, name, false, false, 0o644)
    }

    #[test]
    fn folders_files_and_links_have_their_codicons() {
        assert_eq!(
            glyph_in(IconSet::Nerd, "Trip", true, false, 0o755),
            "\u{ea83}"
        );
        assert_eq!(nerd("README"), "\u{ea7b}");
        assert_eq!(glyph_in(IconSet::Nerd, "to", true, true, 0o755), "\u{eaed}");
        assert_eq!(
            glyph_in(IconSet::Nerd, "to.txt", false, true, 0o644),
            "\u{eaee}"
        );
    }

    #[test]
    fn extensions_and_kinds_pick_their_glyph() {
        for (name, glyph) in [
            ("main.rs", "\u{e68b}"),
            ("a.py", "\u{e606}"),
            ("notes.md", "\u{eb1d}"),
            ("a.js", "\u{e60c}"),
            ("a.ts", "\u{e628}"),
            ("a.json", "\u{e60b}"),
            ("run.zsh", "\u{e691}"),
            ("a.h", "\u{e649}"),
            ("a.cpp", "\u{e646}"),
            ("a.go", "\u{e627}"),
            ("index.html", "\u{e60e}"),
            ("a.css", "\u{e614}"),
            ("a.lua", "\u{e620}"),
            ("Cargo.toml", "\u{e615}"),
            ("a.yml", "\u{e6a8}"),
            ("a.pdf", "\u{eaeb}"),
            ("a.csv", "\u{ebb7}"),
            ("a.pptx", "\u{f0428}"),
            ("part.stl", "\u{eb29}"),
            ("blob.bin", "\u{eae8}"),
            ("song.mp3", "\u{ec1b}"),
            ("clip.mov", "\u{ead9}"),
            ("a.jpg", "\u{eaea}"),
            ("pack.zip", "\u{eaef}"),
            ("notes.txt", "\u{ec5e}"),
            ("letter.docx", "\u{ec5e}"),
        ] {
            assert_eq!(nerd(name), glyph, "{name}");
        }
        // Anything else that runs is a binary.
        assert_eq!(
            glyph_in(IconSet::Nerd, "tool", false, false, 0o755),
            "\u{eae8}"
        );
    }

    #[test]
    fn places_and_view_buttons_have_codicons_and_plain_marks() {
        use crate::app::ViewMode;
        use crate::places::Kind;
        for (kind, nerd, plain) in [
            (Kind::Home, "\u{eb06}", "~"),
            (Kind::Desktop, "\u{ea7a}", "›"),
            (Kind::Documents, "\u{ec5e}", "›"),
            (Kind::Downloads, "\u{ea78}", "›"),
            (Kind::Music, "\u{ec1b}", "›"),
            (Kind::Pictures, "\u{eaea}", "›"),
            (Kind::Movies, "\u{ead9}", "›"),
            (Kind::Projects, "\u{eb30}", "›"),
            (Kind::Trash, "\u{ea81}", "×"),
            (Kind::Drive, "\u{eb50}", "›"),
        ] {
            assert_eq!(place_in(IconSet::Nerd, kind), nerd, "{kind:?}");
            assert_eq!(place_in(IconSet::Plain, kind), plain, "{kind:?}");
        }
        for (view, nerd, plain) in [
            (ViewMode::List, "\u{eb17}", "≡"),
            (ViewMode::Columns, "\u{eae3}", "∥"),
            (ViewMode::Grid, "\u{eae6}", "▦"),
            (ViewMode::Dual, "\u{eb56}", "◫"),
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
