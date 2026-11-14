//! The function key bar: one row under the footer while two panes show,
//! F2 and F5 to F10 spread evenly across it, each the key lit and what it
//! does muted. One that can't run now is all muted. A click on one does
//! what its key does.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::fit;
use crate::app::{App, FnKey, HitMap, Spot};
use crate::theme;

/// Draws the bar on its row: the row cut in seven, each command in the
/// middle of its part, and the whole part a spot for the mouse.
pub fn draw(frame: &mut Frame, app: &App, area: Rect, hits: &mut HitMap) {
    let count = FnKey::ALL.len() as u16;
    for (index, command) in (0..count).zip(FnKey::ALL) {
        let slot = slot(area, index, count);
        let key = format!("F{}", command.number());
        // A cell of air at the end, so a cut label never runs into the next.
        let room = (slot.width as usize).saturating_sub(1);
        let text = fit(&format!("{key} {}", command.label()), room);
        let before = room.saturating_sub(text.width()).div_ceil(2);
        let lit = if app.can_run(command) {
            theme::fg(theme::ACCENT)
        } else {
            theme::muted()
        };
        let keys = key.chars().count();
        let line = Line::from(vec![
            Span::raw(" ".repeat(before)),
            Span::styled(text.chars().take(keys).collect::<String>(), lit),
            Span::styled(text.chars().skip(keys).collect::<String>(), theme::muted()),
        ]);
        frame.render_widget(Paragraph::new(line), slot);
        hits.add(slot, Spot::FnKey(command));
    }
}

/// The part `index` of `count` equal parts of the row, side by side.
/// Worked out in `u32`, so no width is too wide for it.
fn slot(area: Rect, index: u16, count: u16) -> Rect {
    let width = u32::from(area.width);
    let edge = |index: u16| width * u32::from(index) / u32::from(count);
    let (from, to) = (edge(index), edge(index + 1));
    // Never more than the width, which is a u16.
    let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
    Rect::new(area.x + narrow(from), area.y, narrow(to - from), 1)
}

#[cfg(test)]
mod tests {
    use crate::app::testing::{ctrl, finish_work, fixture, key, press, split};
    use crate::app::{Asking, FnKey, Spot};
    use crate::theme;
    use crate::ui::testing::{buffer, find, screen};
    use ratatui::crossterm::event::{
        KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    const LABELS: [&str; 7] = [
        "F2 Rename",
        "F5 Copy",
        "F6 Move",
        "F7 Mkdir",
        "F8 Delete",
        "F9 New MD",
        "F10 New TXT",
    ];

    /// Where each label starts on the bottom row of a 120 by 30 frame.
    fn starts(app: &mut crate::app::App) -> Vec<Option<u16>> {
        let drawn = buffer(app, 120, 30);
        LABELS
            .iter()
            .map(|label| {
                find(&drawn, label, 0..120)
                    .filter(|(_, y)| *y == 29)
                    .map(|(x, _)| x)
            })
            .collect()
    }

    fn left_click(app: &mut crate::app::App, at: (u16, u16)) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.0,
            row: at.1,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn on_a_very_wide_window_the_parts_cover_the_row_side_by_side() {
        let area = ratatui::layout::Rect::new(0, 3, 10_000, 1);
        let parts: Vec<_> = (0..7).map(|index| super::slot(area, index, 7)).collect();
        assert_eq!(parts[0].x, 0);
        for pair in parts.windows(2) {
            assert_eq!(pair[0].x + pair[0].width, pair[1].x);
        }
        assert_eq!(parts[6].x + parts[6].width, 10_000);
        for part in &parts {
            assert_eq!((part.y, part.height), (3, 1));
            assert!((1428..=1429).contains(&part.width), "{part:?}");
        }
    }

    #[test]
    fn on_a_narrow_window_the_items_keep_a_cell_apart() {
        let mut f = fixture();
        split(&mut f.app);
        let drawn = buffer(&mut f.app, 60, 30);
        let y = 29;
        // The last cell of every item's share stays blank, cut or not.
        for index in 1..7u16 {
            let x = 60 * index / 7;
            assert_eq!(drawn[(x - 1, y)].symbol(), " ", "before item {index}");
        }
        assert_eq!(drawn[(59, y)].symbol(), " ", "after the last");
    }

    #[test]
    fn the_bar_shows_only_in_two_panes_with_its_seven_items_in_order() {
        let mut f = fixture();
        // One pane: the footer on the last row and no bar.
        let one = screen(&mut f.app, 120, 30);
        assert!(one[29].contains("? keys"), "{}", one[29]);
        assert!(one.iter().all(|row| !row.contains("F5 Copy")));

        split(&mut f.app);
        let drawn = buffer(&mut f.app, 120, 30);
        let rows = screen(&mut f.app, 120, 30);
        // The footer one row up, over the bar; the rule over the footer.
        assert!(rows[28].contains("? keys"), "{}", rows[28]);
        assert!(rows[27].starts_with('─'), "{}", rows[27]);
        let xs = starts(&mut f.app);
        for (index, (label, x)) in LABELS.iter().zip(&xs).enumerate() {
            let x = x.unwrap_or_else(|| panic!("{label} on the bottom row: {}", rows[29]));
            // Each in its seventh of the width.
            let slot = (120 * index as u16 / 7)..(120 * (index as u16 + 1) / 7);
            assert!(slot.contains(&x), "{label} at {x}, not in {slot:?}");
            // The key lit, the label muted.
            let keys = label.split(' ').next().unwrap().len() as u16;
            assert_eq!(drawn[(x, 29)].fg, theme::ink(theme::ACCENT), "{label}");
            assert_eq!(drawn[(x + keys - 1, 29)].fg, theme::ink(theme::ACCENT));
            assert_eq!(drawn[(x + keys + 1, 29)].fg, theme::MUTED, "{label}");
        }
        assert!(xs.windows(2).all(|pair| pair[0] < pair[1]), "in order");
        assert_eq!(FnKey::ALL.map(FnKey::label)[6], "New TXT");

        // Back to one pane, and in the other views: no bar, no row taken.
        press(&mut f.app, &[KeyCode::Char('|')]);
        for view in ['1', '2', '3'] {
            f.app.handle_key(ctrl(KeyCode::Char(view)));
            let rows = screen(&mut f.app, 120, 30);
            assert!(rows[29].contains("? keys"), "view {view}: {}", rows[29]);
            assert!(
                rows.iter().all(|row| !row.contains("F5 Copy")),
                "view {view}"
            );
        }
    }

    #[test]
    fn an_item_whose_command_cannot_run_is_drawn_all_muted() {
        let mut f = fixture();
        split(&mut f.app);
        // The right pane in the empty Documents: nothing to rename, copy,
        // move or delete; folders and files can still be made.
        press(&mut f.app, &[KeyCode::Enter]);
        assert!(f.app.pane().entries.is_empty());
        let drawn = buffer(&mut f.app, 120, 30);
        let xs = starts(&mut f.app);
        let lit = |x: Option<u16>| drawn[(x.unwrap(), 29)].fg == theme::ink(theme::ACCENT);
        let muted = |x: Option<u16>| drawn[(x.unwrap(), 29)].fg == theme::MUTED;
        for (index, label) in LABELS.iter().enumerate() {
            let can = matches!(index, 3 | 5 | 6);
            assert_eq!(lit(xs[index]), can, "{label}");
            assert_eq!(muted(xs[index]), !can, "{label}");
        }
        // Several marked: rename takes one item.
        press(&mut f.app, &[KeyCode::BackTab]);
        press(
            &mut f.app,
            &[KeyCode::Char('v'), KeyCode::Down, KeyCode::Char('v')],
        );
        let drawn = buffer(&mut f.app, 120, 30);
        let xs = starts(&mut f.app);
        assert_eq!(drawn[(xs[0].unwrap(), 29)].fg, theme::MUTED, "F2");
        assert_eq!(drawn[(xs[1].unwrap(), 29)].fg, theme::ink(theme::ACCENT));
        // While a prompt is open nothing runs: every item muted.
        press(&mut f.app, &[KeyCode::F(7)]);
        assert!(f.app.prompt.is_some());
        let drawn = buffer(&mut f.app, 120, 30);
        let bar: Vec<_> = (0..120).map(|x| drawn[(x, 29)].fg).collect();
        assert!(bar.iter().all(|fg| *fg != theme::ink(theme::ACCENT)));
        assert!(FnKey::ALL.iter().all(|command| !f.app.can_run(*command)));
    }

    /// Every item on the bottom row of a 120 by 30 frame drawn all muted:
    /// its key in the colour of its label (both dimmed under a card), and
    /// nothing on the row in the accent colour.
    fn assert_every_item_muted(app: &mut crate::app::App, open: &str) {
        let drawn = buffer(app, 120, 30);
        let row: String = (0..120).map(|x| drawn[(x, 29)].symbol()).collect();
        let accent = theme::ink(theme::ACCENT);
        assert!((0..120).all(|x| drawn[(x, 29)].fg != accent), "{open}");
        for label in LABELS {
            let x = row
                .find(label)
                .unwrap_or_else(|| panic!("{open}: {label} on the bottom row: {row}"));
            let muted = drawn[(x as u16 + label.len() as u16 - 1, 29)].fg;
            for dx in 0..label.len() {
                let fg = drawn[(x as u16 + dx as u16, 29)].fg;
                assert_eq!(fg, muted, "{open}: {label}, cell {dx}");
            }
        }
    }

    /// A left click in the middle of an item, as the last frame drew it.
    fn click_item(app: &mut crate::app::App, command: FnKey) {
        buffer(app, 120, 30);
        let rect = app.hits.rect_of(&Spot::FnKey(command)).expect("on the bar");
        left_click(app, (rect.x + rect.width / 2, rect.y));
    }

    #[test]
    fn while_a_card_or_quick_look_is_open_every_item_is_muted_and_clicks_run_nothing() {
        let mut f = fixture();
        split(&mut f.app);
        let documents = f.home.join("Documents");
        // The left pane on home, on notes.txt; the right one on Documents.
        press(
            &mut f.app,
            &[KeyCode::Enter, KeyCode::BackTab, KeyCode::End],
        );
        assert_eq!(f.app.panes()[1].cwd, documents);
        let notes = f.home.join("notes.txt");
        // Home, Documents and the Trash (made on first use) as they are.
        let on_disk = || {
            let folders = [&f.home, &documents, &f.trash];
            let mut names: Vec<_> = (folders.iter())
                .flat_map(|folder| std::fs::read_dir(folder).into_iter().flatten())
                .map(|item| item.unwrap().path())
                .collect();
            names.sort();
            names
        };
        let before = on_disk();
        // Nothing open: every command can run, so every key is lit.
        assert!(FnKey::ALL.iter().all(|command| f.app.can_run(*command)));
        let drawn = buffer(&mut f.app, 120, 30);
        for x in starts(&mut f.app) {
            assert_eq!(drawn[(x.unwrap(), 29)].fg, theme::ink(theme::ACCENT));
        }

        // Shift+Delete's card, asking to delete notes.txt for good.
        f.app
            .handle_key(crate::app::testing::shift(KeyCode::Delete));
        assert!(matches!(
            f.app.dialog,
            Some(crate::app::Dialog::Delete { .. })
        ));
        assert!(FnKey::ALL.iter().all(|command| !f.app.can_run(*command)));
        assert_every_item_muted(&mut f.app, "the card");
        // A click on F8 only closes the card, as one outside it does.
        click_item(&mut f.app, FnKey::Trash);
        assert!(f.app.dialog.is_none());
        assert!(f.app.prompt.is_none() && f.app.question.is_none());
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(), before);
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello");
        // The card again: a click on F5 copies nothing either.
        f.app
            .handle_key(crate::app::testing::shift(KeyCode::Delete));
        assert!(f.app.dialog.is_some());
        click_item(&mut f.app, FnKey::Copy);
        assert!(f.app.dialog.is_none());
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(), before);

        // Quick Look, in the terminal (the setting's default).
        press(&mut f.app, &[KeyCode::Char(' ')]);
        assert!(f.app.look.is_some());
        assert!(FnKey::ALL.iter().all(|command| !f.app.can_run(*command)));
        assert_every_item_muted(&mut f.app, "Quick Look");
        // A click on F9 only closes it, as one outside it does.
        click_item(&mut f.app, FnKey::NewMarkdown);
        assert!(f.app.look.is_none());
        assert!(f.app.prompt.is_none());
        assert_eq!(on_disk(), before);
        press(&mut f.app, &[KeyCode::Char(' ')]);
        click_item(&mut f.app, FnKey::Move);
        assert!(f.app.look.is_none());
        assert_eq!(f.app.working(), None);
        assert_eq!(on_disk(), before);
        // The cursor stayed on notes.txt, and the bar is lit again.
        assert_eq!(
            f.app.pane().selected().map(|entry| entry.name.as_str()),
            Some("notes.txt")
        );
        let drawn = buffer(&mut f.app, 120, 30);
        for x in starts(&mut f.app) {
            assert_eq!(drawn[(x.unwrap(), 29)].fg, theme::ink(theme::ACCENT));
        }
    }

    #[test]
    fn a_click_on_an_item_does_what_its_key_does() {
        let mut f = fixture();
        split(&mut f.app);
        let xs = starts(&mut f.app);
        // F7: the folder name prompt.
        left_click(&mut f.app, (xs[3].unwrap(), 29));
        let asking = f.app.prompt.as_ref().map(|prompt| prompt.asking.clone());
        assert_eq!(asking, Some(Asking::NewFolder(f.home.clone())));
        // A click while the prompt is open does nothing.
        buffer(&mut f.app, 120, 30);
        left_click(&mut f.app, (xs[5].unwrap(), 29));
        assert!(!f.home.join("new.md").exists());
        press(&mut f.app, &[KeyCode::Esc]);

        // F9: an empty new.md and its rename prompt.
        buffer(&mut f.app, 120, 30);
        left_click(&mut f.app, (xs[5].unwrap() + 4, 29));
        assert!(f.home.join("new.md").is_file());
        let asking = f.app.prompt.as_ref().map(|prompt| prompt.asking.clone());
        assert_eq!(asking, Some(Asking::Rename(f.home.join("new.md"))));
        press(&mut f.app, &[KeyCode::Esc]);

        // F5 on the cursor's item, into the other pane's folder.
        press(&mut f.app, &[KeyCode::BackTab, KeyCode::Enter]);
        let documents = f.home.join("Documents");
        assert_eq!(f.app.panes()[0].cwd, documents);
        press(&mut f.app, &[KeyCode::Tab]);
        f.app.handle_key(key(KeyCode::End));
        buffer(&mut f.app, 120, 30);
        let rect = f.app.hits.rect_of(&Spot::FnKey(FnKey::Copy)).expect("F5");
        left_click(&mut f.app, (rect.x, rect.y));
        finish_work(&mut f.app);
        assert_eq!(
            std::fs::read_to_string(documents.join("notes.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            f.app.panes()[0].selected().map(|e| e.name.as_str()),
            Some("notes.txt")
        );
    }
}
