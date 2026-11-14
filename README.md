# filefinder-tui

A keyboard-first terminal file manager for macOS, written in Rust with [ratatui](https://ratatui.rs/). It follows the terminal interface of [flea](https://github.com/thisisgm/flea): a header with the path and the sort order, then three columns (the parent folder, the list, and a preview), and a footer with the counts, what just happened and the free space. There is no sidebar; colours are flea's quiet grey palette on your terminal's own background.

Nothing runs in the background. Folders are read when you open them, and thumbnails are made when a tile needs one.

- **The screen**: the places panel on the left, the list in the middle with a glyph, the name and the size on each row, and the preview on the right. Links show in cyan with where they point, programs in green. Alt+P hides or shows the preview, and the choice is remembered. The header has the path, the tabs, four view buttons (list, columns, grid, two panes; the one showing lit, a click switches like Ctrl+1–4; dropped first when the window is narrow) and the sort order.
- **Places panel**, in every view: PLACES (Applications when `/Applications` exists; Home; Desktop, Documents, Downloads, Music, Pictures, Movies and Projects when they exist; the Trash) and DEVICES (the system's own volume, which opens `/`; iCloud Drive when it is on; then the other mounted volumes). The place you are in is lifted. Tab (or Shift+Tab) moves the keyboard into it; `j`/`k` move, ⏎ or `l` opens a place and gives the keyboard back to the list; a click opens one. Ctrl+B hides or shows it, and the choice is remembered. It is read at start, on F5 and when the keyboard enters it.
- **Preview pane**: the name, kind, size and date, then a folder's items, a text file's lines with line numbers (code coloured the way flea colours it, by `bat` with its ANSI theme, once it is ready), a picture with its pixel size, an app's icon, or what several selected items add up to. Tab moves the keyboard into it to scroll. Space opens flea's Quick Look card over the window.
- **Four views**: Ctrl+1 the list; Ctrl+2 Miller columns from home down to the folder, then the preview; Ctrl+3 a grid of real thumbnails with two-line names, beside the preview (apps show their own icon, from the `.icns` file their `Info.plist` names, and in the list, the columns and the two panes too, in place of their glyph, where the terminal draws real pictures); Ctrl+4 two lists side by side, each under its own path. **The app opens in two panes**, both on the start folder with the left one active; Ctrl+4 (or `|`, Alt+1) goes to the one-pane view you used last, which is remembered (the list the first time).
- **Tabs**: `t` opens one in the folder you are in, `O` the folder under the cursor; `[` / `]` go round them, `1`–`9` jump to one, `{` / `}` move it, `w` closes it (the last one quits). Each tab keeps its own folder, view, panes, marks and history; the strip in the header shows once there are two, at most nine.
- **Colours**: truecolor when `COLORTERM` says so, the nearest of 256 colours otherwise, and none with `NO_COLOR` (the cursor row is then reverse video and marked rows underlined). `FILEFINDER_ICONS=plain` replaces the Nerd Font glyphs with plain marks.
- **Trash**: `~/.Trash`, with Restore and Empty Trash in its menu; reach it from the places panel, the menu or `:`.
- **File operations** that never overwrite: copy, cut, paste, duplicate, rename, new folder, compress, extract, edit in `$EDITOR`, move to the trash, and delete permanently after asking. Everything except a permanent delete can be undone.
- **Finder tags**: `T` opens flea's seven tags (Red, Orange, Yellow, Green, Blue, Purple, Gray); `1`–`7` turns one on, or off where every selected item already has it, and Remove All Tags takes the seven off. They are real Finder tags, kept in the `com.apple.metadata:_kMDItemUserTags` attribute exactly as Finder writes it, so Finder, Spotlight and `mdfind` see them, and tags of your own stay as they are. They show as coloured squares after the name (their digits without colour), and Find takes `tag:red` (any case; every tag named must be there, and the rest of the text is looked for as usual: `tag:red holiday`). Undo takes a change back exactly. A drive whose file system has no room for them says "This filesystem does not support file tags."
- **Bulk rename**: `r` with several items marked puts their names in `$EDITOR`, one a line, in a private file of the system's temporary folder. Change the lines, save and quit; the renames run together, so names can swap (`a` ↔ `b`) or go round, and one undo takes them all back. The same number of lines in the same order is needed; a line that is empty, holds `/`, is `.` or `..`, is too long, repeats another, or names something else already there refuses the lot with the line it is about, and nothing is renamed. Quitting the editor with an error renames nothing either.
- **Compress and Extract**, in the menu (`m`). Compress writes zip, tar, tar.gz, tar.bz2 or tar.xz (and 7z when [7-Zip](https://www.7-zip.org/) is installed as `7zz` or `7z`), named after the item without its extension, or after the folder several are in. Extract unpacks a zip, tar, tar.gz/.tgz, tar.bz2, tar.xz (or 7z) into a new folder beside it named after it without its ending. A name in use is never written over: the next free copy name is taken (`pack copy.tar.gz`, `pack copy 2.tar.gz`, `pack copy`). An archive holding a name from the root, one with `..`, or a link that leads out of its folder is refused with nothing written; one that is damaged or needs a password says so and leaves nothing behind; Escape stops either. Undo moves what was made to the Trash.
- **Two panes** with Ctrl+4, `|` or Alt+1 to move things between folders (`y`/`x` in one, `p` or `M` in the other); the same key goes back to the view you came from.
- **Filter and Find**, both typed on the bottom row. The filter (`/`) narrows the folder you are looking at as you type: part of a name in any case, or a `*` / `?` pattern; close matches (a letter or two off, depending on how long the text is) show only when nothing matches exactly. Tab widens it to every folder below. Find (`f` or Ctrl+F) walks the folders below and ranks what it finds: the whole name, then its start, the start of a word, the letters in order, and typos last. Tab looks in this folder, home, or everywhere; Ctrl+G looks inside files instead, through [ripgrep](https://github.com/BurntSushi/ripgrep) when it is installed. Go to a path with `:`.
- **Mouse friendly**, by flea's rules: click, double-click, right-click for the menu, Ctrl+click for one more, Shift+click for a range, the wheel to move.

## Requirements

- macOS.
- A Rust toolchain. With Homebrew: `brew install rustup && rustup default stable`. Homebrew's rustup is keg-only, so put it on your `PATH`, for example in `~/.zshrc`:

  ```sh
  export PATH="$(brew --prefix rustup)/bin:$PATH"
  ```

- A terminal with a graphics protocol for real thumbnails: Ghostty, kitty and WezTerm use the kitty protocol, and iTerm2 its own. Other terminals fall back to coarser half-block pictures.
- A terminal with the kitty keyboard protocol for chords such as Ctrl+1/2/3/4 and Ctrl+Shift+N. Ghostty, kitty, WezTerm and iTerm2 have it.
- [bat](https://github.com/sharkdp/bat) for coloured code in the preview (`brew install bat`). Without it, code shows plain.
- A [Nerd Font](https://www.nerdfonts.com/) for the icons. Without one, run with `FILEFINDER_ICONS=plain` for plain Unicode symbols.

## Installing

Straight from GitHub:

```sh
cargo install --git https://github.com/juancasanueva/filefinder-tui
```

Or from a clone:

```sh
git clone https://github.com/juancasanueva/filefinder-tui.git
cd filefinder-tui
cargo install --path .
```

Either way the binary lands in `~/.cargo/bin`, which needs to be on your `PATH`. Run the same command again to update.

## Running

```sh
filefinder-tui                          # opens the current folder
filefinder-tui ~/Pictures
filefinder-tui ~/Pictures/photo.jpg     # opens its folder with the file selected
```

From a clone, without installing: `cargo run --release -- ~/Pictures`.

## Keys

flea's own terminal keys. Digits are never counts.

| Key | Does |
| --- | --- |
| `j` / `k`, ↓ / ↑ | Down, up |
| `g` / `G`, Home / End | First, last |
| Ctrl+D / Ctrl+U, PgDn / PgUp | Page down, page up |
| `h`, ←, Backspace | The parent folder |
| `l`, → | Browse into the folder, or open the file |
| ⏎ | Open |
| `H` / `L` | Back, forward |
| Tab, Shift+Tab | The places panel, the list and the preview, or the places panel and the two panes |
| Escape | Close or go back: an error, the filter, Find (its walk first), the file work under way, the marks |
| `:`, Ctrl+L | Go to a place (Tab completes) |
| `/` | Filter this folder; in the field Tab adds its subfolders, ⏎ or ↓ keeps the filter, Escape clears it, Ctrl+A marks what it shows |
| `f`, Ctrl+F | Find below this folder; in the field Tab changes where (this folder, home, everywhere), ⏎ or ↓ moves into what it found |
| Ctrl+G | In Find's field: names or what files hold (needs `rg`) |
| `tag:red` | In Find's field: only what carries that tag |
| `o` | Show what Find found under the cursor in its own folder |
| Space | Quick Look, in the terminal (see below) |
| `.` | Show hidden files |
| `s` / `S` | Next sort key (name, size, modified, kind) / reverse the sort |
| `I` | Detail column: size, modified, mode (`drwx------`), owner |
| `T` | Finder tags: `1`–`7` turn a tag on or off, Remove All Tags |
| Alt+P | Show or hide the preview pane |
| Ctrl+B | Show or hide the places panel |
| Ctrl+R, F5 | Reload |
| `v` | Mark or unmark the item under the cursor |
| `J` / `K`, Shift+↓ / ↑ | Mark down, up |
| `V`, Ctrl+A | Invert the marks, mark everything |
| `y`, `x`, `p` (Ctrl+C / X / V) | Copy, cut, paste |
| `M` | Move here what was copied or cut |
| `U` | Duplicate |
| `r`, F2 | Rename; with several marked, all of them in `$EDITOR` |
| `N`, Ctrl+Shift+N | New folder |
| `dd`, Delete | Move to Trash |
| Shift+Delete | Delete permanently (a card asks first; Cancel is the default) |
| `z`, Ctrl+Z | Undo |
| Ctrl+Shift+Z, Ctrl+Y | Redo |
| `E` | Edit the file in `$EDITOR` (`vi` without one) |
| `Y`, Ctrl+Shift+C | Copy the folder's path, the item's path |
| `m`, Menu, Shift+F10 | The menu (see below) |
| Ctrl+T | Open a terminal here |
| Ctrl+1 / 2 / 3 | List, columns, grid |
| Ctrl+4, `\|`, Alt+1 | Two panes, or back to the view before (on a Spanish Mac keyboard `\|` is Option+1) |
| `+` / `-` | Bigger, smaller tiles |
| `t` | New tab, in the folder you are in |
| `O` | Open the folder under the cursor in a new tab |
| `w`, Ctrl+W | Close the tab (closing the last one quits) |
| `[` / `]` | Previous tab, next tab |
| `{` / `}` | Move the tab left, right |
| `1` … `9` | Go to that tab |
| `?` | The keyboard shortcuts, in a card that scrolls with `j` / `k` and the page keys |
| `q` | Quit |

Rename, Folder name and Go to ask on the bottom row. A paste that meets names already taken asks there too: Enter or `b` keeps both (`name copy.ext`), `r` replaces (the one there goes to the Trash, and undo brings it back), `s` skips, Escape cancels.

### The menu

`m` (or Menu, Shift+F10, a right click on an item) opens flea's menu under the item, at its name, or above it when there is no room below:

- Open, Open With › (Default app, and Quick Look in macOS's own window)
- Compress › (`.zip`, `.tar`, `.tar.gz`, `.tar.bz2`, `.tar.xz`, and `.7z` with 7-Zip installed), Extract (for an archive)
- Tags › (the seven Finder tags, Remove All Tags), Copy Path
- Rename (with several selected, all of them in `$EDITOR`), Duplicate
- Move to Trash, Delete Permanently…
- Show/Hide Hidden Files, Open Trash

In the Trash it offers Restore, Delete Permanently… and Empty Trash…. `j` / `k` and the arrows move, `l`, → or ⏎ choose or open a submenu, `h` or ← go back out of one, Escape closes. Each row does exactly what its key does. What can't apply is left out or dimmed.

### Quick Look

Space opens a card over the window with the item's name, one fact (a picture's size, a text's lines) and the picture, an app's icon, the text with line numbers (coloured like the preview's), or a folder's items. `j` / `k` and ↓ / ↑ move the list's cursor and the card follows; `+` / `-` zoom the picture and `0` fits it; PgDn / PgUp scroll the text; Escape or Space closes, and so does a click on its `×` or outside it. macOS's own Quick Look window is in the menu: Open With › Quick Look.

### The mouse

- A click puts the cursor on an item and clears the selection; a second click within 400 ms opens it (among what Find found, shows it in its folder, like `o`).
- Ctrl-click (or Cmd-click where the terminal sends it) marks or unmarks one item; Shift-click selects the range from the cursor.
- A right click opens the menu for the item, or for the selection when the item is in it.
- The wheel moves the cursor a row; over the preview, the shortcuts or Quick Look it scrolls them.
- A click on a tab's label shows that tab; one on a view button switches the view; one on a place opens it. The wheel over the places panel scrolls it.
- A click outside the menu or a card closes it; a card's buttons can be clicked, and moving over the menu's rows or a card's buttons puts the keyboard there.

The mouse is tracked as flea does: presses and drags, and every move only while a menu or a card is open. Select text in the terminal with its own modifier: Shift-drag in Ghostty, Option-drag in Terminal.

## Where it keeps things

- `~/.config/filefinder-tui/`: the chosen view, tile size, sort, detail column and whether the preview and the places panel show (`state`), and where trashed items came from (`trashed`).
- `~/Library/Caches/filefinder-tui/`: Quick Look thumbnails.
- The Trash is `~/.Trash`. Restore works for items this app moved there.

## Notes and limits

- Listing `~/.Trash` needs Full Disk Access for your terminal (System Settings → Privacy & Security). Moving things into it works without it.
- On FAT-formatted drives macOS keeps file attributes in `._name` files. A file `name` and a file of your own called `._name` can't sit side by side there, so FileFinder refuses that operation instead of letting macOS replace your file.
- Find stops after 500 matches. Everywhere means `/` on the system's volume and the home folder's, leaving out `/System`, `/private/var/vm`, `/Volumes`, `/dev` and `/cores`; links to folders are never followed. The filter's subfolders take in at most 20,000 items.
- Not included: copying and pasting with other apps through the system clipboard, Open With other apps than the default one, sharing, ejecting drives, drag and drop, and the mouse's back and forward buttons.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The plan, the specification and the log of decisions live in `odd/tasks/folio-tui.md`.
