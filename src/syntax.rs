//! Code coloured the way flea colours it: `bat`, with its ANSI theme, over
//! the preview's own lines, its colours read back as six roles. Made off the
//! UI thread; until it is ready, and without `bat`, the text shows plain.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

/// A piece of a line in one colour: its role (0 the foreground, then 1 red,
/// 2 green, 3 yellow, 4 blue, 5 magenta, 6 cyan) and its text.
pub type Run = (u8, String);

/// One line of text as runs.
pub type Runs = Vec<Run>;

/// The port that colours text. It is called on a worker thread, never while
/// drawing.
pub trait Colourer: Send + Sync {
    /// `bat`'s coloured output for `text`, its language told by the file
    /// name `name`; `None` when there is none.
    fn colour(&self, name: &str, text: &str) -> Option<String>;
}

/// `bat`, the way flea runs it.
pub struct Bat;

/// flea's flags: no user config, the ANSI theme, colours only.
const BAT_FLAGS: [&str; 7] = [
    "--no-config",
    "--no-custom-assets",
    "--theme=ansi",
    "--color=always",
    "--style=plain",
    "--paging=never",
    "--wrap=never",
];

/// How long `bat` gets before it is given up on.
const PATIENCE: Duration = Duration::from_secs(2);

impl Colourer for Bat {
    fn colour(&self, name: &str, text: &str) -> Option<String> {
        let mut child = Command::new("bat")
            .args(BAT_FLAGS)
            .arg(format!("--file-name={name}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // The preview's few kilobytes fit the pipes: no reader thread needed.
        child.stdin.take()?.write_all(text.as_bytes()).ok()?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let mut output = String::new();
        child.stdout.take()?.read_to_string(&mut output).ok()?;
        Some(output)
    }
}

/// The lines in `output`, each as runs: SGR 31–36 and 91–96 colour what
/// follows, a reset or the default and grey foregrounds end it, and anything
/// else (bold, backgrounds, extended colours) is left out.
pub fn runs(output: &str) -> Vec<Runs> {
    output.split('\n').map(line_runs).collect()
}

fn line_runs(line: &str) -> Runs {
    let mut runs: Runs = Vec::new();
    let mut role = 0;
    let mut rest = line;
    while !rest.is_empty() {
        let (text, after) = match rest.find('\x1b') {
            Some(at) => rest.split_at(at),
            None => (rest, ""),
        };
        push(&mut runs, role, text);
        rest = after;
        let Some(sequence) = rest.strip_prefix("\x1b[") else {
            // A lone escape, or one that isn't SGR: dropped with what it says.
            rest = rest.get(1..).unwrap_or_default();
            continue;
        };
        let end = sequence.find(|c: char| !c.is_ascii_digit() && c != ';');
        let Some(end) = end else { break };
        let (codes, after) = sequence.split_at(end);
        let ends = after.chars().next().unwrap_or('m');
        rest = &after[ends.len_utf8()..];
        if ends != 'm' {
            continue;
        }
        let mut codes = codes.split(';').map(|code| code.parse::<u8>().unwrap_or(0));
        while let Some(code) = codes.next() {
            match code {
                31..=36 => role = code - 30,
                91..=96 => role = code - 90,
                0 | 30 | 37 | 39 | 90 | 97 => role = 0,
                // An extended colour: its own numbers are no codes.
                38 | 48 => {
                    let skip = if codes.next() == Some(5) { 1 } else { 3 };
                    codes.by_ref().take(skip).for_each(drop);
                    if code == 38 {
                        role = 0;
                    }
                }
                _ => {}
            }
        }
    }
    runs
}

/// Adds text to the last run when it is the same colour.
fn push(runs: &mut Runs, role: u8, text: &str) {
    if text.is_empty() {
        return;
    }
    match runs.last_mut() {
        Some((last, run)) if *last == role => run.push_str(text),
        _ => runs.push((role, text.to_string())),
    }
}

/// flea's terminal rule: a magenta run of nothing but operators is drawn in
/// the foreground, so `=` and `->` don't shout like keywords.
pub fn calm_operators(line: &mut Runs) {
    let operator = |c: char| "=+-*%<>!&|^~?:".contains(c);
    let runs = std::mem::take(line);
    for (role, text) in runs {
        let calm = role == 5 && text.chars().all(operator);
        push(line, if calm { 0 } else { role }, &text);
    }
}

/// The lines `shown` coloured from `bat`'s output for them: a line keeps its
/// colours only when they spell it exactly, and shows plain otherwise.
pub fn coloured(output: &str, shown: &[String]) -> Vec<Runs> {
    let mut lines = runs(output).into_iter();
    shown
        .iter()
        .map(|text| {
            let mut runs = lines.next().unwrap_or_default();
            let spelled: String = runs.iter().map(|(_, run)| run.as_str()).collect();
            if spelled != *text {
                runs = vec![(0, text.clone())];
            }
            calm_operators(&mut runs);
            runs.retain(|(_, run)| !run.is_empty());
            runs
        })
        .collect()
}

/// Which file's opening lines, as it was then.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    modified: Option<SystemTime>,
}

struct Job {
    key: Key,
    name: String,
    lines: Vec<String>,
}

#[derive(Default)]
struct Slot {
    job: Mutex<(Option<Job>, bool)>,
    ready: Condvar,
}

/// Colours the opening lines of one file at a time on a worker thread; the
/// newest file asked for replaces one still waiting.
pub struct Highlighter {
    slot: Arc<Slot>,
    results: Receiver<(Key, Vec<Runs>)>,
    asked: Option<Key>,
    ready: Option<(Key, Vec<Runs>)>,
}

impl Highlighter {
    pub fn new(colourer: Arc<dyn Colourer>) -> Self {
        let slot = Arc::new(Slot::default());
        let (sender, results) = mpsc::channel();
        let shared = slot.clone();
        // Without its thread the service still answers: nothing is ever ready.
        let _ = thread::Builder::new()
            .name("syntax".to_string())
            .spawn(move || work(&shared, colourer.as_ref(), &sender));
        Self {
            slot,
            results,
            asked: None,
            ready: None,
        }
    }

    /// The opening lines of the file at `path` coloured, when they are
    /// ready; asked for when they aren't.
    pub fn get(
        &mut self,
        path: &Path,
        modified: Option<SystemTime>,
        lines: &[String],
    ) -> Option<&[Runs]> {
        let key = Key {
            path: path.to_path_buf(),
            modified,
        };
        if self.ready.as_ref().is_some_and(|(ready, _)| *ready == key) {
            return self.ready.as_ref().map(|(_, lines)| lines.as_slice());
        }
        if self.asked.as_ref() != Some(&key) {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let mut job = self.slot.job.lock().unwrap_or_else(PoisonError::into_inner);
            job.0 = Some(Job {
                key: key.clone(),
                name,
                lines: lines.to_vec(),
            });
            self.slot.ready.notify_all();
            self.asked = Some(key);
        }
        None
    }

    /// Takes in what the worker finished.
    pub fn collect(&mut self) {
        while let Ok((key, lines)) = self.results.try_recv() {
            if self.asked.as_ref() == Some(&key) {
                self.asked = None;
            }
            self.ready = Some((key, lines));
        }
    }

    /// Whether what was asked for has come back.
    #[cfg(test)]
    pub fn is_idle(&self) -> bool {
        self.asked.is_none()
    }
}

impl Drop for Highlighter {
    fn drop(&mut self) {
        let mut job = self.slot.job.lock().unwrap_or_else(PoisonError::into_inner);
        job.1 = true;
        self.slot.ready.notify_all();
    }
}

fn work(slot: &Slot, colourer: &dyn Colourer, results: &Sender<(Key, Vec<Runs>)>) {
    loop {
        let job = {
            let mut job = slot.job.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if job.1 {
                    return;
                }
                if let Some(next) = job.0.take() {
                    break next;
                }
                job = slot.ready.wait(job).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let plain = || {
            job.lines
                .iter()
                .map(|line| vec![(0, line.clone())])
                .collect()
        };
        let lines = match colourer.colour(&job.name, &job.lines.join("\n")) {
            Some(output) => coloured(&output, &job.lines),
            None => plain(),
        };
        if results.send((job.key, lines)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// Colours like `bat` would, in a few strokes: `fn` magenta, words in
    /// double quotes green, `=` magenta; everything else plain.
    #[derive(Default)]
    pub struct FakeColourer {
        pub asked: Mutex<Vec<String>>,
    }

    impl Colourer for FakeColourer {
        fn colour(&self, name: &str, text: &str) -> Option<String> {
            self.asked.lock().unwrap().push(name.to_string());
            let lines: Vec<String> = text
                .split('\n')
                .map(|line| {
                    let mut out = String::new();
                    let mut quoted = false;
                    for (index, word) in line.split(' ').enumerate() {
                        if index > 0 {
                            out.push(' ');
                        }
                        if word == "fn" || word == "=" {
                            out.push_str(&format!("\x1b[35m{word}\x1b[0m"));
                        } else if word.starts_with('"') || quoted {
                            quoted = !word.ends_with('"') || word == "\"";
                            out.push_str(&format!("\x1b[32m{word}\x1b[0m"));
                        } else {
                            out.push_str(word);
                        }
                    }
                    out
                })
                .collect();
            Some(lines.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeColourer;
    use super::*;

    fn line(runs: &[(u8, &str)]) -> Runs {
        runs.iter()
            .map(|(role, text)| (*role, text.to_string()))
            .collect()
    }

    #[test]
    fn bat_s_colours_become_roles_and_everything_else_is_left_out() {
        let output = "\x1b[35mfn\x1b[0m \x1b[34mmain\x1b[0m() {\n\
                      \x1b[1;92m\"hi\"\x1b[39m;\x1b[38;5;208mx\x1b[0m\x1b[48;2;1;2;3m!\x1b[0m\n\
                      \n\
                      \x1b[31mred\x1b[30mblack\x1b[96mcyan\x1b[90mgrey";
        assert_eq!(
            runs(output),
            [
                line(&[(5, "fn"), (0, " "), (4, "main"), (0, "() {")]),
                // Bold, an extended colour and a background say nothing.
                line(&[(2, "\"hi\""), (0, ";x!")]),
                line(&[]),
                line(&[(1, "red"), (0, "black"), (6, "cyan"), (0, "grey")]),
            ]
        );
        // A sequence that isn't a colour, ending in any character, says nothing.
        assert_eq!(runs("a\x1b[2éb\x1b"), [line(&[(0, "ab")])]);
    }

    #[test]
    fn a_magenta_run_of_only_operators_is_drawn_in_the_foreground() {
        let mut runs = line(&[(5, "let"), (0, " x "), (5, "="), (0, " "), (5, "->")]);
        calm_operators(&mut runs);
        assert_eq!(runs, line(&[(5, "let"), (0, " x = ->")]));
        // Not magenta, or not only operators: left as it is.
        let mut runs = line(&[(2, "="), (5, "=x")]);
        calm_operators(&mut runs);
        assert_eq!(runs, line(&[(2, "="), (5, "=x")]));
    }

    #[test]
    fn a_line_keeps_its_colours_only_when_they_spell_it() {
        let shown = [
            "fn main".to_string(),
            "same".to_string(),
            "lost".to_string(),
        ];
        let output = "\x1b[35mfn\x1b[0m main\nchanged";
        assert_eq!(
            coloured(output, &shown),
            [
                line(&[(5, "fn"), (0, " main")]),
                line(&[(0, "same")]),
                line(&[(0, "lost")]),
            ]
        );
    }

    #[test]
    fn bat_colours_rust_with_its_ansi_theme_when_it_is_installed() {
        let Some(output) = Bat.colour("main.rs", "fn main() {\n    let s = \"hi\";\n}") else {
            eprintln!("bat isn't installed: nothing to check");
            return;
        };
        let lines = coloured(&output, &["fn main() {".to_string()]);
        assert_eq!(lines[0][0], (5, "fn".to_string()), "{output:?}");
        // A name it doesn't know: plain text, nothing coloured.
        let plain = Bat.colour("notes.unknown-kind", "fn main").unwrap();
        assert!(runs(&plain).iter().flatten().all(|(role, _)| *role == 0));
    }

    #[test]
    fn the_highlighter_colours_off_the_ui_thread_and_keeps_the_newest_file() {
        let colourer = Arc::new(FakeColourer::default());
        let mut highlighter = Highlighter::new(colourer.clone());
        let lines = vec!["fn main".to_string(), "let s = \"hi\"".to_string()];
        let path = Path::new("/code/main.rs");
        assert!(highlighter.get(path, None, &lines).is_none(), "not yet");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !highlighter.is_idle() {
            assert!(Instant::now() < deadline, "never coloured");
            highlighter.collect();
            thread::sleep(Duration::from_millis(2));
        }
        let got = highlighter.get(path, None, &lines).expect("ready");
        assert_eq!(got[0], line(&[(5, "fn"), (0, " main")]));
        assert_eq!(
            got[1],
            line(&[(0, "let s = "), (2, "\"hi\"")]),
            "the lone `=` is calmed"
        );
        assert_eq!(*colourer.asked.lock().unwrap(), ["main.rs"]);
        // The same file changed since: asked again, nothing shown meanwhile.
        let later = Some(SystemTime::UNIX_EPOCH);
        assert!(highlighter.get(path, later, &lines).is_none());
    }
}
