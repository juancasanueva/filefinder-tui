# filefinder-tui

A keyboard-first terminal file manager for macOS, written in Rust with [ratatui](https://ratatui.rs/). It follows the terminal interface of [flea](https://github.com/thisisgm/flea): a header with the path and the sort order, then three columns (the parent folder, the list, and a preview), and a footer with the counts, what just happened and the free space. There is no sidebar; colours are flea's quiet grey palette on your terminal's own background.

Nothing runs in the background. Folders are read when you open them, and thumbnails are made when a tile needs one.

- **The screen**: the parent folder's items on the left (the folder you are in lifted), the list in the middle with a glyph, the name and the size on each row, and the preview on the right. Links show in cyan with where they point, programs in green. Alt+P hides or shows the preview, and the choice is remembered.
- **Preview pane**: the name, kind, size and date, then a folder's items, a text file's lines with line numbers, a picture with its pixel size, or what several selected items add up to. Tab moves the keyboard into it to scroll. Space opens Quick Look.
- **Other views**: Finder-style columns and a grid with real thumbnails. Ctrl+1/2/3 switches between the list, the columns and the grid; the choice is remembered.
- **Colours**: truecolor when `COLORTERM` says so, the nearest of 256 colours otherwise, and none with `NO_COLOR` (the cursor row is then reverse video and marked rows underlined). `FILEFINDER_ICONS=plain` replaces the Nerd Font glyphs with plain marks.
- **Trash**: `~/.Trash`, with Restore and Empty Trash in its menu; reach it with `:`.
- **File operations** that never overwrite: copy, cut, paste, duplicate, rename, new folder, compress, edit in `$EDITOR`, move to the trash, and delete permanently after asking. Everything except a permanent delete can be undone.
- **Split** the window in two with Ctrl+4, `|` or Alt+1 to move things between folders; the same key closes it again.
- **Search** this folder and below with `/`, or go to a path with `:`.
- **Mouse friendly**: click, double-click, right-click for the menu, Shift+click for a range.

## Requirements

- macOS.
- A Rust toolchain. With Homebrew: `brew install rustup && rustup default stable`. Homebrew's rustup is keg-only, so put it on your `PATH`, for example in `~/.zshrc`:

  ```sh
  export PATH="$(brew --prefix rustup)/bin:$PATH"
  ```

- A terminal with a graphics protocol for real thumbnails: Ghostty, kitty and WezTerm use the kitty protocol, and iTerm2 its own. Other terminals fall back to coarser half-block pictures.
- A terminal with the kitty keyboard protocol for chords such as Ctrl+1/2/3/4 and Ctrl+Shift+N. Ghostty, kitty, WezTerm and iTerm2 have it.
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
| Tab | The list and the preview, or the two panes |
| Escape | Close or go back: an error, the search, the file work under way, the marks |
| `:`, Ctrl+L | Go to a place (Tab completes) |
| `/`, `f`, Ctrl+F | Search this folder and below |
| Space | Quick Look |
| `.` | Show hidden files |
| `s` / `S` | Next sort key (name, size, modified, kind) / reverse the sort |
| `I` | Detail column: size, modified, mode (`drwx------`), owner |
| Alt+P | Show or hide the preview pane |
| Ctrl+R, F5 | Reload |
| `v` | Mark or unmark the item under the cursor |
| `J` / `K`, Shift+↓ / ↑ | Mark down, up |
| `V`, Ctrl+A | Invert the marks, mark everything |
| `y`, `x`, `p` (Ctrl+C / X / V) | Copy, cut, paste |
| `M` | Move here what was copied or cut |
| `U` | Duplicate |
| `r`, F2 | Rename |
| `N`, Ctrl+Shift+N | New folder |
| `dd`, Delete | Move to Trash |
| Shift+Delete | Delete permanently (asks first) |
| `z`, Ctrl+Z | Undo |
| Ctrl+Shift+Z, Ctrl+Y | Redo |
| `E` | Edit the file in `$EDITOR` (`vi` without one) |
| `Y`, Ctrl+Shift+C | Copy the folder's path, the item's path |
| `m`, Menu, Shift+F10 | The menu |
| Ctrl+T | Open a terminal here |
| Ctrl+1 / 2 / 3 | List, columns, grid |
| Ctrl+4, `\|`, Alt+1 | Two panes, or back to one (on a Spanish Mac keyboard `\|` is Option+1) |
| `+` / `-` | Bigger, smaller tiles |
| `?` | Every shortcut |
| `q`, `w`, Ctrl+W | Quit |

Rename, Folder name and Go to ask on the bottom row. A paste that meets names already taken asks there too: Enter or `b` keeps both (`name copy.ext`), `r` replaces (the one there goes to the Trash, and undo brings it back), `s` skips, Escape cancels.

With the mouse on, select text in the terminal with its own modifier: Shift-drag in Ghostty, Option-drag in Terminal.

## Where it keeps things

- `~/.config/filefinder-tui/`: the chosen view, tile size, sort, detail column and whether the preview shows (`state`), and where trashed items came from (`trashed`).
- `~/Library/Caches/filefinder-tui/`: Quick Look thumbnails.
- The Trash is `~/.Trash`. Restore works for items this app moved there.

## Notes and limits

- Listing `~/.Trash` needs Full Disk Access for your terminal (System Settings → Privacy & Security). Moving things into it works without it.
- On FAT-formatted drives macOS keeps file attributes in `._name` files. A file `name` and a file of your own called `._name` can't sit side by side there, so FileFinder refuses that operation instead of letting macOS replace your file.
- Not included: copying and pasting with other apps through the system clipboard, Open With, sharing, ejecting drives, drag and drop, and the mouse's back and forward buttons.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The plan, the specification and the log of decisions live in `odd/tasks/folio-tui.md`.
