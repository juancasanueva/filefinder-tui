//! Human-readable sizes, dates and natural ordering.

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;
use std::time::SystemTime;

use chrono::{DateTime, Local};

/// Decimal sizes, like Finder: `27.1 KB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub fn items(count: usize) -> String {
    match count {
        0 => "No items".to_string(),
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateStyle {
    /// `11/10/26 6:10 PM`
    MonthFirst,
    /// `10/11/26 18:10`
    DayFirst,
}

impl DateStyle {
    pub fn from_env() -> Self {
        let lang = ["LC_ALL", "LC_TIME", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty())
            .unwrap_or_default();
        Self::from_lang(&lang)
    }

    pub fn from_lang(lang: &str) -> Self {
        if lang.is_empty() || lang == "C" || lang == "POSIX" || lang.starts_with("en_US") {
            Self::MonthFirst
        } else {
            Self::DayFirst
        }
    }

    fn date(self) -> &'static str {
        match self {
            Self::MonthFirst => "%-m/%-d/%y",
            Self::DayFirst => "%-d/%-m/%y",
        }
    }

    fn time(self) -> &'static str {
        match self {
            Self::MonthFirst => "%-I:%M %p",
            Self::DayFirst => "%-H:%M",
        }
    }
}

/// Date and time, for the details pane.
pub fn date_time(time: SystemTime, style: DateStyle) -> String {
    let local: DateTime<Local> = time.into();
    format!(
        "{} {}",
        local.format(style.date()),
        local.format(style.time())
    )
}

/// The time for something changed today, the date otherwise; for tiles and rows.
pub fn short_date(time: SystemTime, now: SystemTime, style: DateStyle) -> String {
    let local: DateTime<Local> = time.into();
    let today: DateTime<Local> = now.into();
    if local.date_naive() == today.date_naive() {
        local.format(style.time()).to_string()
    } else {
        local.format(style.date()).to_string()
    }
}

/// The type and permission bits of a Unix mode the way `ls -l` writes them,
/// like `drwx------` or `-rwsr-xr-x`.
pub fn permissions(mode: u32) -> String {
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o010000 => 'p',
        0o140000 => 's',
        0o020000 => 'c',
        0o060000 => 'b',
        _ => '-',
    };
    // Read, write, execute for owner, group, others; the execute place also
    // carries setuid, setgid and sticky: lower case with execute, upper without.
    let triplets = [
        (0o400, 0o200, 0o100, 0o4000, 's'),
        (0o040, 0o020, 0o010, 0o2000, 's'),
        (0o004, 0o002, 0o001, 0o1000, 't'),
    ];
    let mut text = String::with_capacity(10);
    text.push(kind);
    for (read, write, execute, special, mark) in triplets {
        text.push(if mode & read != 0 { 'r' } else { '-' });
        text.push(if mode & write != 0 { 'w' } else { '-' });
        text.push(match (mode & execute != 0, mode & special != 0) {
            (true, true) => mark,
            (false, true) => mark.to_ascii_uppercase(),
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    text
}

/// Case-insensitive ordering where runs of digits compare as numbers,
/// so `photo 9` sorts before `photo 10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                match take_number(&mut left).cmp(&take_number(&mut right)) {
                    Ordering::Equal => {}
                    other => return other,
                }
            }
            (Some(x), Some(y)) => {
                let (x, y) = (lower(x), lower(y));
                if x != y {
                    return x.cmp(&y);
                }
                left.next();
                right.next();
            }
        }
    }
}

fn take_number(chars: &mut Peekable<Chars>) -> u128 {
    let mut number: u128 = 0;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
        number = number.saturating_mul(10).saturating_add(digit as u128);
        chars.next();
    }
    number
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> SystemTime {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .into()
    }

    #[test]
    fn sizes_are_decimal_with_one_decimal_place() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(27_100), "27.1 KB");
        assert_eq!(size(999_990), "1.0 MB");
        assert_eq!(size(5_300_000_000), "5.3 GB");
    }

    #[test]
    fn item_counts_read_naturally() {
        assert_eq!(items(0), "No items");
        assert_eq!(items(1), "1 item");
        assert_eq!(items(3), "3 items");
    }

    #[test]
    fn natural_order_compares_digit_runs_as_numbers() {
        let mut names = vec![
            "photo 10.jpg",
            "Photo 9.jpg",
            "photo 2.jpg",
            "alpha",
            "Beta",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec![
                "alpha",
                "Beta",
                "photo 2.jpg",
                "Photo 9.jpg",
                "photo 10.jpg"
            ]
        );
    }

    #[test]
    fn date_style_follows_the_language() {
        assert_eq!(DateStyle::from_lang("en_US.UTF-8"), DateStyle::MonthFirst);
        assert_eq!(DateStyle::from_lang(""), DateStyle::MonthFirst);
        assert_eq!(DateStyle::from_lang("es_ES.UTF-8"), DateStyle::DayFirst);
    }

    #[test]
    fn dates_render_in_both_styles() {
        let time = at(2026, 11, 10, 18, 10);
        assert_eq!(date_time(time, DateStyle::MonthFirst), "11/10/26 6:10 PM");
        assert_eq!(date_time(time, DateStyle::DayFirst), "10/11/26 18:10");
    }

    #[test]
    fn short_date_is_the_time_today_and_the_date_before() {
        let now = at(2026, 11, 10, 20, 0);
        let style = DateStyle::MonthFirst;
        assert_eq!(short_date(at(2026, 11, 10, 18, 10), now, style), "6:10 PM");
        assert_eq!(short_date(at(2026, 11, 9, 18, 10), now, style), "11/9/26");
    }

    #[test]
    fn permissions_read_like_ls() {
        assert_eq!(permissions(0o040700), "drwx------");
        assert_eq!(permissions(0o100644), "-rw-r--r--");
        assert_eq!(permissions(0o120755), "lrwxr-xr-x");
        assert_eq!(permissions(0o010600), "prw-------");
        assert_eq!(permissions(0o140755), "srwxr-xr-x");
        assert_eq!(permissions(0o020620), "crw--w----");
        assert_eq!(permissions(0o060640), "brw-r-----");
    }

    #[test]
    fn permissions_show_setuid_setgid_and_sticky_bits() {
        assert_eq!(permissions(0o104755), "-rwsr-xr-x");
        assert_eq!(permissions(0o104644), "-rwSr--r--");
        assert_eq!(permissions(0o102755), "-rwxr-sr-x");
        assert_eq!(permissions(0o102745), "-rwxr-Sr-x");
        assert_eq!(permissions(0o041777), "drwxrwxrwt");
        assert_eq!(permissions(0o041776), "drwxrwxrwT");
    }
}
