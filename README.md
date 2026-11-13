# FileFinder (TUI)

A keyboard-first terminal file manager for macOS, written in Rust with [ratatui](https://ratatui.rs/). It follows the looks and behavior of Omarchy's [folio](https://github.com/omacom/folio): one quiet window with breadcrumbs, a sidebar, a roomy grid with real previews, and a details pane.

Nothing runs in the background. Folders are read when you open them, and thumbnails are made when a tile needs one.

- **Three views**: a grid with real thumbnails, a list with permissions (`drwx------`), size and date, and Finder-style columns. Ctrl+1/2/3 switches; the choice is remembered.
- **Details pane**: a large preview, the kind and size, the modified date, picture dimensions, and the opening lines of text files. Space opens Quick Look.
- **Sidebar**: every folder in your home, pinned folders, Recents (from Spotlight), the Trash with Restore and Empty Trash, and mounted drives.
- **File operations** that never overwrite: copy, cut, paste, duplicate, rename, new folder, compress, move to the trash, and delete permanently after asking. Everything except a permanent delete can be undone.
- **Split** the window in two with `|` (or the ◫ icon in the top bar) to move things between folders; the same key or icon closes it again.
- **Search** this folder and below with `/`, or type a path with Ctrl+L.
- **Mouse friendly**: click, double-click, right-click for the menu, Shift+click for a range.

## Requirements

- macOS.
- A Rust toolchain. With Homebrew: `brew install rustup && rustup default stable`. Homebrew's rustup is keg-only, so put it on your `PATH`, for example in `~/.zshrc`:

  ```sh
  export PATH="$(brew --prefix rustup)/bin:$PATH"
  ```

- A terminal with a graphics protocol for real thumbnails: Ghostty, kitty and WezTerm use the kitty protocol, and iTerm2 its own. Other terminals fall back to coarser half-block pictures.
- A terminal with the kitty keyboard protocol for chords such as Ctrl+1/2/3, Ctrl+I and Ctrl+Shift+N. Ghostty, kitty, WezTerm and iTerm2 have it.
- A [Nerd Font](https://www.nerdfonts.com/) for the icons. Without one, run with `FILEFINDER_ICONS=plain` for plain Unicode symbols.

## Running

```sh
cargo run --release            # opens the current folder
cargo run --release -- ~/Pictures
cargo run --release -- ~/Pictures/photo.jpg   # opens its folder with the file selected
```

To install it on your `PATH`: `cargo install --path .`, then run `filefinder-tui`.

## Keys

| Key | Does |
| --- | --- |
| Arrows, ⏎ | Move, open |
| → / ← | In list and columns: into a folder / back out |
| Shift+arrows, Ctrl+A | Select a range, select everything |
| Space | Preview with Quick Look |
| Alt+⏎ | The menu: everything else, for files and for places in the sidebar |
| Backspace, Alt+← / Alt+→, Alt+Home | Up, back, forward, home |
| Ctrl+L | Type a path (Tab completes) |
| / | Search this folder and below |
| Ctrl+C / X / V | Copy, cut, paste |
| Ctrl+Shift+C | Copy the path as text |
| F2, Ctrl+Shift+D | Rename, duplicate |
| Ctrl+Shift+N | New folder |
| Delete (fn+⌫) or ⌘⌫ | Move to the trash (in the sidebar: unpin) |
| Shift+Delete | Delete permanently (asks first) |
| Ctrl+Z, Ctrl+Shift+Z or Ctrl+Y | Undo, redo |
| Ctrl+P | Pin or unpin the folder |
| `\|` | Split into two panes, or close the right one |
| Ctrl+Alt+N, Ctrl+W | Split into two panes, close the right one |
| Tab, Shift+Tab | Sidebar, panes and details |
| Ctrl+1 / 2 / 3 | Grid, list, columns |
| Ctrl+= / Ctrl+- / Ctrl+0 | Bigger, smaller, normal tiles |
| Ctrl+I | Show or hide the details pane |
| S, Shift+S | Sort: name, kind, newest, oldest, size |
| Ctrl+H | Show hidden files |
| F5 | Reload |
| Escape | Stop the file work under way, or dismiss what went wrong |
| ? | Every shortcut |
| Q | Quit |

With the mouse on, select text in the terminal with its own modifier: Shift-drag in Ghostty, Option-drag in Terminal.

## Where it keeps things

- `~/.config/filefinder-tui/`: the chosen view and tile size (`state`), pinned folders (`pinned`, one path per line), and where trashed items came from (`trashed`).
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
