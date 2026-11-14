//! The glyph in front of a name: flea's Nerd Font Codicons by default, plain
//! marks with `FILEFINDER_ICONS=plain`.

use std::sync::OnceLock;

use crate::model::{self, Category, Entry};

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
    fn the_plain_set_marks_folders_and_leaves_files_blank() {
        assert_eq!(glyph_in(IconSet::Plain, "Trip", true, false, 0o755), "›");
        assert_eq!(glyph_in(IconSet::Plain, "to", true, true, 0o755), "›");
        assert_eq!(glyph_in(IconSet::Plain, "a.jpg", false, false, 0o644), " ");
    }
}
