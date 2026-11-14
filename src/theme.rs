//! flea's terminal palette: its fallback colours, since macOS has no Omarchy
//! theme to follow, drawn at the colour depth the terminal offers. The
//! background is always the terminal's own; only the fills below paint one.

use std::cell::Cell;
use std::ffi::OsStr;
use std::ops::RangeInclusive;

use ratatui::style::{Color, Modifier, Style};

pub const FOREGROUND: Color = Color::Rgb(0xca, 0xcc, 0xcc);
/// Muted text, the metadata ink and the frame.
pub const MUTED: Color = Color::Rgb(0x90, 0x92, 0x92);
/// The cursor row of a list the keyboard has left.
pub const SELECTED: Color = Color::Rgb(0x31, 0x34, 0x36);
/// The cursor row of the list the keyboard is in.
pub const CURSOR: Color = Color::Rgb(0x31, 0x34, 0x36);
/// A marked row.
pub const MARKED: Color = Color::Rgb(0x51, 0x54, 0x55);
/// The current tab, and the menu's cursor row.
pub const TAB: Color = Color::Rgb(0x1f, 0x22, 0x24);
/// Behind a match.
#[allow(dead_code)] // flea's palette in full; matches, prompts and cards come later.
pub const MATCH: Color = Color::Rgb(0x2a, 0x2d, 0x2f);
/// The rules and the lines between panes.
pub const BORDER: Color = Color::Rgb(0x26, 0x29, 0x2b);
/// The rule inside a card.
pub const CARD_RULE: Color = Color::Rgb(0x43, 0x46, 0x47);
/// Behind a card's default button.
pub const WASH: Color = Color::Rgb(0x2a, 0x2d, 0x2f);
/// Behind a prompt's field.
pub const FIELD: Color = Color::Rgb(0x1d, 0x20, 0x22);
/// Behind the selected part of a field.
#[allow(dead_code)] // flea's palette in full; matches, prompts and cards come later.
pub const FIELD_SELECTION: Color = Color::Rgb(0x47, 0x49, 0x4b);
/// A file's extension.
#[allow(dead_code)] // flea's palette in full; matches, prompts and cards come later.
pub const EXTENSION: Color = Color::Rgb(0x90, 0x92, 0x92);
pub const ERROR: Color = Color::Rgb(0xb0, 0x69, 0x69);
/// What can't be used right now.
#[allow(dead_code)] // flea's palette in full; matches, prompts and cards come later.
pub const UNAVAILABLE: Color = Color::Rgb(0x68, 0x42, 0x43);
pub const SYMLINK: Color = Color::Rgb(0x94, 0xe2, 0xd5);
pub const EXECUTABLE: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
/// Text behind a card.
pub const DIMMED: Color = Color::Rgb(0x6d, 0x70, 0x70);

// The glyph in front of a name takes a colour by what the item is, from the
// same palette as the tags, so that kinds stand apart at a glance.
pub const ICON_FOLDER: Color = TAGS[4];
pub const ICON_IMAGE: Color = TAGS[5];
pub const ICON_VIDEO: Color = TAGS[0];
pub const ICON_AUDIO: Color = TAGS[2];
pub const ICON_ARCHIVE: Color = TAGS[1];
pub const ICON_CODE: Color = TAGS[3];
pub const ICON_PDF: Color = TAGS[0];
/// Without a theme the accent is the foreground itself.
pub const ACCENT: Color = FOREGROUND;

/// The seven Finder tags' colours, in their order: Red, Orange, Yellow,
/// Green, Blue, Purple, Gray.
/// Code's colours by `bat`'s role: the foreground, then flea's red, green,
/// yellow, blue, magenta and cyan.
pub const SYNTAX: [Color; 7] = [
    FOREGROUND,
    Color::Rgb(0xf3, 0x8b, 0xa8),
    Color::Rgb(0xa6, 0xe3, 0xa1),
    Color::Rgb(0xf9, 0xe2, 0xaf),
    Color::Rgb(0x89, 0xb4, 0xfa),
    Color::Rgb(0xcb, 0xa6, 0xf7),
    Color::Rgb(0x94, 0xe2, 0xd5),
];

pub const TAGS: [Color; 7] = [
    Color::Rgb(0xf7, 0x76, 0x8e),
    Color::Rgb(0xeb, 0x92, 0x7b),
    Color::Rgb(0xe0, 0xaf, 0x68),
    Color::Rgb(0x9e, 0xce, 0x6a),
    Color::Rgb(0x7a, 0xa2, 0xf7),
    Color::Rgb(0xad, 0x8e, 0xe6),
    Color::Rgb(0x56, 0x5f, 0x89),
];

/// The mark a tag gets after a name: a square in its colour, or without
/// colour, its digit, which tells the seven apart all the same.
pub fn tag_mark(index: usize) -> (String, Color) {
    let glyph = match depth() {
        Depth::Mono => (index + 1).to_string(),
        _ => "■".to_string(),
    };
    (glyph, ink(TAGS[index % TAGS.len()]))
}

/// How many colours the terminal can show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Depth {
    /// `NO_COLOR`: no colour at all.
    Mono,
    /// The sixteen basic colours, the last resort.
    Sixteen,
    /// The 256-colour palette.
    Indexed,
    #[default]
    Truecolor,
}

impl Depth {
    /// The one place the depth is decided, from the environment: `NO_COLOR`
    /// set and not empty asks for none; `COLORTERM` says when the terminal
    /// takes any colour; a terminal type known for sixteen gets those; any
    /// other gets the 256-colour palette.
    pub fn from_env(
        no_color: Option<&OsStr>,
        colorterm: Option<&OsStr>,
        term: Option<&OsStr>,
    ) -> Self {
        if no_color.is_some_and(|value| !value.is_empty()) {
            return Depth::Mono;
        }
        if matches!(said(colorterm), "truecolor" | "24bit") {
            return Depth::Truecolor;
        }
        match said(term) {
            "dumb" | "linux" | "ansi" | "vt100" | "vt220" => Depth::Sixteen,
            _ => Depth::Indexed,
        }
    }

    /// What this process's environment asks for.
    pub fn detect() -> Self {
        let var = std::env::var_os;
        Self::from_env(
            var("NO_COLOR").as_deref(),
            var("COLORTERM").as_deref(),
            var("TERM").as_deref(),
        )
    }

    /// Without a fill to tell it apart, the cursor row is reverse video.
    pub fn reverses(self) -> bool {
        matches!(self, Depth::Mono | Depth::Sixteen)
    }
}

/// What an environment variable says, as text; nothing when it isn't.
fn said(value: Option<&OsStr>) -> &str {
    value.and_then(OsStr::to_str).unwrap_or_default()
}

thread_local! {
    /// The depth the frame being drawn uses. Drawing sets it from the app.
    static DEPTH: Cell<Depth> = const { Cell::new(Depth::Truecolor) };
}

pub fn set_depth(depth: Depth) {
    DEPTH.with(|cell| cell.set(depth));
}

pub fn depth() -> Depth {
    DEPTH.with(Cell::get)
}

/// A palette colour as the terminal can show it: itself in truecolor, the
/// nearest of 256 or of 16, or nothing at all without colour.
pub fn ink(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    match depth() {
        Depth::Truecolor => color,
        Depth::Indexed => Color::Indexed(nearest([r, g, b], 16..=255)),
        Depth::Sixteen => basic(nearest([r, g, b], 0..=15)),
        Depth::Mono => Color::Reset,
    }
}

/// The palette index nearest a colour, among `range`.
fn nearest(rgb: [u8; 3], range: RangeInclusive<u8>) -> u8 {
    let distance = |index: &u8| {
        let candidate = indexed(*index);
        rgb.iter()
            .zip(candidate)
            .map(|(a, b)| (i32::from(*a) - b).pow(2))
            .sum::<i32>()
    };
    range.min_by_key(distance).unwrap_or(7)
}

/// The colour a 256-colour palette index stands for.
fn indexed(index: u8) -> [i32; 3] {
    const BASIC: [[i32; 3]; 16] = [
        [0, 0, 0],
        [128, 0, 0],
        [0, 128, 0],
        [128, 128, 0],
        [0, 0, 128],
        [128, 0, 128],
        [0, 128, 128],
        [192, 192, 192],
        [128, 128, 128],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [0, 0, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    let index = usize::from(index);
    if index < 16 {
        BASIC[index]
    } else if index < 232 {
        let n = index - 16;
        let level = |c: usize| if c == 0 { 0 } else { 55 + c as i32 * 40 };
        [level(n / 36), level(n / 6 % 6), level(n % 6)]
    } else {
        [8 + (index as i32 - 232) * 10; 3]
    }
}

fn basic(index: u8) -> Color {
    const NAMED: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    NAMED[usize::from(index) % 16]
}

/// Text in a colour of the palette.
pub fn fg(color: Color) -> Style {
    Style::new().fg(ink(color))
}

/// A fill of the palette behind text in another.
pub fn on(color: Color, fill: Color) -> Style {
    fg(color).bg(ink(fill))
}

pub fn base() -> Style {
    fg(FOREGROUND)
}

/// Muted text and the metadata ink.
pub fn muted() -> Style {
    fg(MUTED)
}

pub fn title() -> Style {
    base().add_modifier(Modifier::BOLD)
}

/// The rules and the lines between panes.
pub fn border() -> Style {
    fg(BORDER)
}

/// The rule inside a card and between the menu's groups.
pub fn card_rule() -> Style {
    fg(CARD_RULE)
}

/// The frame of a card or of the menu.
pub fn frame() -> Style {
    fg(MUTED)
}

/// The ground a card dims the screen toward: flea's fallback background,
/// since the terminal's own can't be asked.
const GROUND: [u8; 3] = [0x10, 0x13, 0x15];

/// A style as it shows behind a card: in truecolor each colour mixed
/// halfway to the ground, so the foreground becomes the dimmed text; with
/// fewer colours the text in the dimmed ink, fills as they were.
pub fn dimmed(style: Style) -> Style {
    let half = |color: Option<Color>| match (depth(), color) {
        // flea's own figure for the foreground behind a card.
        (Depth::Truecolor, Some(FOREGROUND)) => Some(DIMMED),
        (Depth::Truecolor, Some(Color::Rgb(r, g, b))) => {
            let mix = |c: u8, ground: u8| (u16::from(c) + u16::from(ground)).div_ceil(2) as u8;
            Some(Color::Rgb(
                mix(r, GROUND[0]),
                mix(g, GROUND[1]),
                mix(b, GROUND[2]),
            ))
        }
        _ => None,
    };
    let fg = half(style.fg).unwrap_or(ink(DIMMED));
    let bg = half(style.bg).or(style.bg);
    Style {
        fg: Some(fg),
        bg,
        ..style
    }
}

pub fn error() -> Style {
    fg(ERROR)
}

/// What is being typed in a field.
pub fn field() -> Style {
    on(FOREGROUND, FIELD)
}

/// A tab's label in the header: the tab showing on the tab fill in the
/// foreground, the others muted.
pub fn tab_label(current: bool) -> Style {
    if current {
        on(FOREGROUND, TAB).add_modifier(cursor_modifier(true))
    } else {
        muted()
    }
}

/// What stands in for a fill the depth can't show: reverse video on the
/// cursor row.
pub fn cursor_modifier(cursor: bool) -> Modifier {
    if cursor && depth().reverses() {
        Modifier::REVERSED
    } else {
        Modifier::empty()
    }
}

/// A row of a list behind its text: the cursor's fill where the keyboard
/// is, the selected fill where it isn't, the marked fill, or none. Without
/// fills the cursor row is reverse video, and without colour a marked row
/// is underlined.
pub fn row_fill(cursor: bool, focused: bool, marked: bool) -> Style {
    let style = Style::new().add_modifier(cursor_modifier(cursor));
    if depth().reverses() {
        if marked && !cursor && depth() == Depth::Mono {
            return style.add_modifier(Modifier::UNDERLINED);
        }
        if cursor {
            return style;
        }
    }
    match (cursor, focused, marked) {
        (true, true, _) => style.bg(ink(CURSOR)),
        (true, false, _) => style.bg(ink(SELECTED)),
        (false, _, true) => style.bg(ink(MARKED)),
        _ => style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_depth_follows_no_color_then_colorterm_then_the_terminal_type() {
        let os = |text: &'static str| Some(OsStr::new(text));
        assert_eq!(Depth::from_env(os("1"), os("truecolor"), None), Depth::Mono);
        // Set but empty asks for nothing.
        assert_eq!(
            Depth::from_env(os(""), os("truecolor"), None),
            Depth::Truecolor
        );
        assert_eq!(Depth::from_env(None, os("24bit"), None), Depth::Truecolor);
        assert_eq!(
            Depth::from_env(None, None, os("xterm-256color")),
            Depth::Indexed
        );
        assert_eq!(Depth::from_env(None, os("yes"), None), Depth::Indexed);
        assert_eq!(Depth::from_env(None, None, os("linux")), Depth::Sixteen);
        assert!(Depth::Mono.reverses() && Depth::Sixteen.reverses());
        assert!(!Depth::Indexed.reverses() && !Depth::Truecolor.reverses());
    }

    #[test]
    fn the_256_colour_palette_takes_the_nearest_index() {
        set_depth(Depth::Indexed);
        for (color, index) in [
            (FOREGROUND, 252),
            (MUTED, 246),
            (SELECTED, 236),
            (CURSOR, 236),
            (MATCH, 236),
            (MARKED, 240),
            (TAB, 235),
            (BORDER, 235),
            (CARD_RULE, 238),
            (FIELD, 234),
            (SYMLINK, 116),
            (EXECUTABLE, 151),
            (ERROR, 131),
        ] {
            assert_eq!(ink(color), Color::Indexed(index), "{color:?}");
        }
    }

    #[test]
    fn truecolor_keeps_the_palette_and_mono_drops_every_colour() {
        set_depth(Depth::Truecolor);
        assert_eq!(ink(FOREGROUND), Color::Rgb(0xca, 0xcc, 0xcc));
        assert_eq!(base().fg, Some(FOREGROUND));
        assert_eq!(base().bg, None, "the terminal's own background");
        set_depth(Depth::Mono);
        assert_eq!(ink(FOREGROUND), Color::Reset);
        assert_eq!(ink(ERROR), Color::Reset);
        set_depth(Depth::Sixteen);
        assert_eq!(ink(FOREGROUND), Color::Gray);
    }

    #[test]
    fn rows_fill_by_cursor_focus_and_mark_or_fall_back_to_attributes() {
        set_depth(Depth::Truecolor);
        assert_eq!(row_fill(true, true, false).bg, Some(CURSOR));
        assert_eq!(row_fill(true, false, true).bg, Some(SELECTED));
        assert_eq!(row_fill(false, true, true).bg, Some(MARKED));
        assert_eq!(row_fill(false, true, false), Style::new());

        set_depth(Depth::Mono);
        let cursor = row_fill(true, true, false);
        assert_eq!(cursor.bg, None);
        assert!(cursor.add_modifier.contains(Modifier::REVERSED));
        let marked = row_fill(false, true, true);
        assert!(marked.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(marked.bg, None);

        set_depth(Depth::Sixteen);
        assert!(
            row_fill(true, false, false)
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(row_fill(false, true, true).bg, Some(Color::DarkGray));
    }
}
