# folio-tui

Objective: a macOS terminal file manager in Rust + ratatui that looks and works like Omarchy's folio (`omacom/folio`, C++/Qt Quick).
Branch: `feat/folio-tui`. Delivery strategy: `exception-ok` (greenfield bootstrap, no remote and no PR target yet; work-unit commits stay reviewable slices).
Test runner: `cargo test` (toolchain from Homebrew `rustup`; `export PATH="$(brew --prefix rustup)/bin:$PATH"`). Lint: `cargo clippy --all-targets -- -D warnings`.

## Specs

- **S1** (request, verbatim): "I want to implement a file manager for MacOS that is done in Rust using the TUI library https://ratatui.rs/ and that is similar in looks and functionality to Omarchy's folio https://github.com/omacom/folio"
- **S2** Looks, taken from folio's `docs/grid.png`, `docs/columns.png` and the fallback palette in `src/theme.cpp`: a top bar with breadcrumbs (`~ / Pictures / Lisbon 2026`, last segment bright) and view / sort / hidden / details toggles; a sidebar with dim upper-case section labels and the current place highlighted; the file area; a details pane with a large preview, the name, `JPEG image · 27.1 KB`, `Modified`, `Dimensions` and an accent `Open` button. Palette: accent `#FFD60A`, background `#16161a`, dark_background `#111114`, lighter_background `#232329`, foreground `#c8c8d0`, bright_foreground `#ececf1`. Selection is shown as color on the item itself.
- **S3** (folio README): "**Three views**: a grid with real previews, a list, and Finder-style columns. Ctrl+1/2/3 switches." Grid tiles show size (or item count) and date under the name. Folders first, natural sort. Sort by "Name", "Kind", "Newest", "Oldest", "Size".
- **S4** (folio README): "**Space previews** pictures, video, audio, PDFs, text and Markdown at full size, in their own window." The details pane shows "large preview, the system's type name, size, modified, dimensions, opening lines of text files, Open".
- **S5** (folio README): "**The sidebar** holds everything in your home folder, pinned folders (shared with GTK file dialogs), Recents, Trash with restore, your drives, and the machines on your tailnet, which open over SFTP."
- **S6** (folio README): "**Right-click** (or Alt+Enter) to open with another app, rename, duplicate, compress, share by Dropbox link, Taildrop or LocalSend, and delete. Copy and paste work with other apps, and nothing is ever overwritten." Delete means trash; file operations can be undone.
- **S7** (folio README): "**Split** the window in two with Ctrl+Alt+N to move things between folders." Ctrl+W closes the right pane.
- **S8** Keys (folio README table, verbatim): "Arrows, ⏎ — Move, open"; "Space — Preview"; "Backspace, Alt+← / Alt+→ — Up, back, forward"; "Ctrl+L — Type a path"; "/ — Search here and below"; "Ctrl+C / X / V — Copy, cut, paste"; "F2, Delete — Rename, move to the trash"; "Ctrl+Shift+N — New folder"; "Ctrl+P — Pin the folder"; "Ctrl+N, Ctrl+Alt+N — New window, split"; "Tab, Shift+Tab — Sidebar, panes and details"; "? — Every other shortcut".
- **S9** Other shortcuts (folio `src/HelpOverlay.qml`, verbatim): "→ / ← — In list and columns: into a folder / back out"; "Shift+arrows — Select a range"; "Ctrl+A — Select everything"; "Delete in the sidebar — Unpin the folder"; "Alt+Home — Go home"; "Ctrl+= / Ctrl+- / Ctrl+0 — Bigger, smaller or normal tiles"; "Ctrl+I — Show or hide the details pane"; "Ctrl+Shift+C — Copy the path as text"; "F2 / Ctrl+Shift+D — Rename / duplicate"; "Shift+Delete — Delete permanently (asks first)"; "Ctrl+Z / Ctrl+Shift+Z — Undo / redo the last change to files (or Ctrl+Y)"; "Escape — Stop the file work under way, or dismiss what went wrong"; "S / Shift+S — Sort: name, kind, newest, oldest, size"; "Ctrl+H — Show hidden files"; "F5 — Reload"; "? — Show these shortcuts"; "Q — Close the window".

Assumptions (mine, not the user's words; macOS and terminal adaptations of the Linux/Qt original):

- **A1** "Real previews" use terminal graphics through `ratatui-image` (kitty protocol in Ghostty/kitty/WezTerm, sixel or iTerm2 elsewhere, half-blocks as the fallback). Non-image thumbnails come from Quick Look (`qlmanage -t`), the macOS counterpart of installed thumbnailers.
- **A2** Space opens Quick Look (`qlmanage -p`), the macOS "own window" preview. Enter opens with `open`.
- **A3** Sidebar on macOS: Home folders, Pinned (stored in `~/.config/filefinder-tui/pinned`), Recents (Spotlight `mdfind`), Trash (`~/.Trash`), Drives (`/Volumes`). Hidden means dotfiles plus the macOS `UF_HIDDEN` flag.
- **A4** Out of scope: plugins, tailnet/SFTP, Dropbox/Taildrop/LocalSend sharing, translations, "New window" (a TUI lives in one terminal window), drag and drop, rubber-band selection, Omarchy theme following.
- **A5** Mac keyboards have no forward Delete key: fn+⌫ sends Delete; ⌘⌫ is also accepted when the terminal forwards it. Ghostty sends Alt+←/→ as Alt+B/Alt+F by default, so both spellings are accepted.
- **A6** Ctrl+1/2/3, Ctrl+I, Ctrl+Shift+letter chords are only distinguishable in terminals with the kitty keyboard protocol (Ghostty, kitty, WezTerm, iTerm2); the app enables it when available.
- **A7** Dates follow `LANG`: `en_US` shows `11/10/26 6:10 PM`, anything else `10/11/26 18:10`. Sizes are decimal (1000-based) like Finder.

## Tasks

- [x] **T1** S1 S2 S8 — scaffold, theme, folder model, shell (top bar, Home sidebar, list view, details text), navigation and history, sort (S / Shift+S), reload (F5), help overlay, quit. Route: inline. Commit: f3910ce (34 tests green, clippy clean, smoke-run in tmux)
- [ ] **T2** S3 S9 — grid and columns views, Ctrl+1/2/3, sort, hidden files, reload, details toggle, tile sizes. Route: delegated (context backstop). Commit: —
- [ ] **T3** S3 S4 — thumbnails and details preview (ratatui-image, Quick Look thumbnails), text opening lines, Space preview. Route: delegated (context backstop). Commit: —
- [ ] **T4** S6 S8 S9 — file operations: copy/cut/paste without overwriting, rename, duplicate, new folder, trash, delete permanently, copy path, undo/redo, status band. Route: delegated (context backstop). Commit: —
- [ ] **T5** S5 S8 S9 — sidebar Pinned / Recents / Trash / Drives, Tab focus cycle, pin and unpin. Route: delegated (context backstop). Commit: —
- [ ] **T6** S8 — Ctrl+L path entry with Tab completion, `/` search here and below. Route: delegated (context backstop). Commit: —
- [ ] **T7** S7 S9 — split panes, multi-select (Shift+arrows, Ctrl+A). Route: delegated (context backstop). Commit: —
- [ ] **T8** S6 — context menu (Alt+Enter), compress, mouse (click, double-click, scroll, right-click). Route: delegated (context backstop). Commit: —

Route evidence: T1 inline. From T2 on, one bounded writer per task, one at a time in the same tree: Writer trigger (b), the long-session context backstop (the parent session passed roughly 150k tokens during T1). Tasks share `src/app.rs`, so they never run in parallel; the parent owns git.

## Log

- **L1** (2026-11-13, user, verbatim): "I want to implement a file manager for MacOS that is done in Rust using the TUI library https://ratatui.rs/ and that is similar in looks and functionality to Omarchy's folio https://github.com/omacom/folio"
- **L2** (2026-11-13) Explored folio: README, `plans/folio.md`, `docs/*.png`, `src/HelpOverlay.qml`, `src/theme.cpp`, `src/SortMenu.qml`, `src/i18n/en.json`. It is C++/Qt Quick for Linux, so this is a port of looks and behavior, not of code.
- **L3** (2026-11-13) No Rust toolchain was installed; installed `rustup` with Homebrew (keg-only, not on PATH) and `stable` (rustc 1.100.0). Dependencies: ratatui 0.30.2, ratatui-image 11.1.0 (without the `chafa` feature, which needs a system library), image 0.25, trash 5.3, chrono 0.4.
- **L4** (2026-11-13) Forecast: about 4,000 authored lines across T1–T8, far above one 400-line slice; strategy `exception-ok` recorded in the header, one commit per task.
- **L5** (2026-11-13) T1 done in f3910ce. Native assess over 7fcee92..f3910ce: risk medium, review not due (`under_budget`).
- **L6** (2026-11-13) Trash probe on this Mac, terminal without Full Disk Access: moving a file into `~/.Trash` works, `stat` and moving a known path back out work, listing `~/.Trash` is refused. Decision for T4/T5: trash is a rename into `~/.Trash/<unique name>` (undo is the rename back, no `trash` crate), and the sidebar Trash shows "Permission was refused." with a Full Disk Access hint when it can't be listed. The probe file was moved back and removed.
- **L7** (2026-11-13) Decisions carried by T1 that later tasks must keep: `System` trait in `src/system.rs` is the only port to the outside (tests use `app::testing::FakeSystem`); `App::handle_key` is the public interface tests drive; `ui::testing::screen` renders to text; `model` and `format` hold no terminal types; the home folder and start folder are injected through `app::Config`.
