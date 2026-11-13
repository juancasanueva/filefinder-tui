//! Glyphs for places and file kinds: Nerd Font by default, plain Unicode on request.

use std::sync::OnceLock;

use crate::model::Category;

// Recents, Trash, Drive and Pin arrive with the sidebar sections.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Folder,
    File,
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Movies,
    Applications,
    Code,
    Cloud,
    Recents,
    Trash,
    Drive,
    Pin,
    Image,
    Video,
    Audio,
    Pdf,
    Text,
    Archive,
    App,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSet {
    Nerd,
    Plain,
}

static ICON_SET: OnceLock<IconSet> = OnceLock::new();

/// `FILEFINDER_ICONS=plain` swaps Nerd Font glyphs for plain Unicode.
pub fn init_from_env() {
    let plain = std::env::var("FILEFINDER_ICONS").is_ok_and(|value| value == "plain");
    let _ = ICON_SET.set(if plain { IconSet::Plain } else { IconSet::Nerd });
}

impl Icon {
    pub fn glyph(self) -> &'static str {
        self.glyph_in(*ICON_SET.get().unwrap_or(&IconSet::Nerd))
    }

    pub fn glyph_in(self, set: IconSet) -> &'static str {
        if set == IconSet::Plain {
            return match self {
                Icon::Folder
                | Icon::Home
                | Icon::Desktop
                | Icon::Documents
                | Icon::Downloads
                | Icon::Music
                | Icon::Pictures
                | Icon::Movies
                | Icon::Applications
                | Icon::Code
                | Icon::Cloud => "▪",
                Icon::Recents => "◷",
                Icon::Trash => "×",
                Icon::Drive => "▬",
                Icon::Pin => "•",
                _ => "▫",
            };
        }
        match self {
            Icon::Folder => "\u{f114}",
            Icon::File => "\u{f016}",
            Icon::Home => "\u{f015}",
            Icon::Desktop => "\u{f108}",
            Icon::Documents => "\u{f0f6}",
            Icon::Downloads => "\u{f019}",
            Icon::Music => "\u{f001}",
            Icon::Pictures => "\u{f03e}",
            Icon::Movies => "\u{f008}",
            Icon::Applications => "\u{f00a}",
            Icon::Code => "\u{f121}",
            Icon::Cloud => "\u{f0c2}",
            Icon::Recents => "\u{f017}",
            Icon::Trash => "\u{f014}",
            Icon::Drive => "\u{f0a0}",
            Icon::Pin => "\u{f08d}",
            Icon::Image => "\u{f1c5}",
            Icon::Video => "\u{f1c8}",
            Icon::Audio => "\u{f1c7}",
            Icon::Pdf => "\u{f1c1}",
            Icon::Text => "\u{f0f6}",
            Icon::Archive => "\u{f1c6}",
            Icon::App => "\u{f135}",
        }
    }

    pub fn for_category(category: Category) -> Icon {
        match category {
            Category::Folder => Icon::Folder,
            Category::Image => Icon::Image,
            Category::Video => Icon::Video,
            Category::Audio => Icon::Audio,
            Category::Pdf => Icon::Pdf,
            Category::Text => Icon::Text,
            Category::Code => Icon::Code,
            Category::Archive => Icon::Archive,
            Category::App => Icon::App,
            Category::Other => Icon::File,
        }
    }

    /// Well-known home folders get their own glyph, the way folio's XDG folders do.
    pub fn for_folder_name(name: &str) -> Icon {
        match name {
            "Desktop" => Icon::Desktop,
            "Documents" => Icon::Documents,
            "Downloads" => Icon::Downloads,
            "Music" => Icon::Music,
            "Pictures" => Icon::Pictures,
            "Movies" | "Videos" => Icon::Movies,
            "Applications" => Icon::Applications,
            "Developer" | "Projects" | "programming" | "code" | "src" => Icon::Code,
            "Dropbox" | "iCloud Drive" | "OneDrive" | "Google Drive" => Icon::Cloud,
            _ => Icon::Folder,
        }
    }
}
