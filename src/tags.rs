//! Finder tags: the seven colours flea names, kept where macOS keeps them,
//! in the `com.apple.metadata:_kMDItemUserTags` attribute, so that Finder
//! shows the same tags. No terminal types in here.
//!
//! The attribute holds a binary property list: an array of strings, each a
//! tag's name, then for a colour a newline and Finder's number for it
//! (`"Red\n6"`). Tags this app doesn't know are kept as they are.

use std::io;
use std::path::Path;

/// flea's seven tags, in menu order, which is also the order of keys 1 to 7.
pub const NAMES: [&str; 7] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"];

/// Finder's colour number for each of the seven, in the same order.
const FINDER_COLOURS: [u8; 7] = [6, 7, 5, 2, 4, 3, 1];

/// Where macOS keeps an item's tags.
pub const ATTRIBUTE: &str = "com.apple.metadata:_kMDItemUserTags";

/// What is said when the disk can't keep tags, in flea's words.
pub const UNSUPPORTED: &str = "This filesystem does not support file tags.";

/// The name of a stored tag: what comes before its colour number.
pub fn name(label: &str) -> &str {
    label.split('\n').next().unwrap_or_default()
}

/// Which of the seven a tag's name is, whatever its case.
pub fn index_of(name: &str) -> Option<usize> {
    let name = name.to_lowercase();
    NAMES.iter().position(|known| known.to_lowercase() == name)
}

/// The seven an item carries, one bit each, bit 0 for Red.
pub fn mask(labels: &[String]) -> u8 {
    labels
        .iter()
        .filter_map(|label| index_of(name(label)))
        .fold(0, |mask, index| mask | 1 << index)
}

/// One of the seven as Finder stores it: `"Red\n6"`.
pub fn label(index: usize) -> String {
    format!("{}\n{}", NAMES[index], FINDER_COLOURS[index])
}

/// The tags with one of the seven added at the end, when it isn't there.
pub fn with(labels: &[String], index: usize) -> Vec<String> {
    let mut labels = labels.to_vec();
    if mask(&labels) & 1 << index == 0 {
        labels.push(label(index));
    }
    labels
}

/// The tags without one of the seven; the rest as they were.
pub fn without(labels: &[String], index: usize) -> Vec<String> {
    let kept = labels
        .iter()
        .filter(|label| index_of(name(label)) != Some(index));
    kept.cloned().collect()
}

/// The tags without any of the seven: only the ones this app doesn't know.
pub fn without_known(labels: &[String]) -> Vec<String> {
    let kept = labels
        .iter()
        .filter(|label| index_of(name(label)).is_none());
    kept.cloned().collect()
}

/// Whether an item's tags hold every name asked for, whatever the case.
pub fn carries(labels: &[String], wanted: &[String]) -> bool {
    wanted.iter().all(|want| {
        let want = want.to_lowercase();
        labels
            .iter()
            .any(|label| name(label).to_lowercase() == want)
    })
}

/// Find's text cut into the tags it asks for (`tag:red`, any case, in
/// small letters) and the rest, the words in their order:
/// `"TAG:red holiday"` asks for `red` and `holiday`.
pub fn split_query(query: &str) -> (Vec<String>, String) {
    let mut tags = Vec::new();
    let mut words = Vec::new();
    for word in query.split_whitespace() {
        let asked = word
            .get(..4)
            .filter(|head| head.eq_ignore_ascii_case("tag:"))
            .map(|_| &word[4..])
            .filter(|name| !name.is_empty());
        match asked {
            Some(name) => tags.push(name.to_lowercase()),
            None => words.push(word),
        }
    }
    (tags, words.join(" "))
}

/// The tags in a property list as macOS writes them; `None` for anything
/// that isn't an array of strings, which is then never written over.
pub fn decode(bytes: &[u8]) -> Option<Vec<String>> {
    const HEADER: &[u8] = b"bplist00";
    if bytes.len() < HEADER.len() + 32 || !bytes.starts_with(HEADER) {
        return None;
    }
    let trailer = &bytes[bytes.len() - 32..];
    let (offset_size, ref_size) = (usize::from(trailer[6]), usize::from(trailer[7]));
    let objects = number(&trailer[8..16])?;
    let top = number(&trailer[16..24])?;
    let table = number(&trailer[24..32])?;
    let offset_of = |object: usize| {
        let at = table.checked_add(object.checked_mul(offset_size)?)?;
        number(bytes.get(at..at.checked_add(offset_size)?)?)
    };
    let (kind, count, start) = object(bytes, offset_of(top)?)?;
    if kind != 0xA {
        return None;
    }
    let mut labels = Vec::with_capacity(count.min(1024));
    for index in 0..count {
        let at = start.checked_add(index.checked_mul(ref_size)?)?;
        let reference = number(bytes.get(at..at.checked_add(ref_size)?)?)?;
        if reference >= objects {
            return None;
        }
        let (kind, length, start) = object(bytes, offset_of(reference)?)?;
        let text = match kind {
            // ASCII, a byte a character.
            0x5 => {
                let text = bytes.get(start..start.checked_add(length)?)?;
                String::from_utf8(text.to_vec())
                    .ok()
                    .filter(|text| text.is_ascii())?
            }
            // UTF-16, big end first.
            0x6 => {
                let text = bytes.get(start..start.checked_add(length.checked_mul(2)?)?)?;
                let units: Vec<u16> = (text.as_chunks::<2>().0.iter())
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect();
                String::from_utf16(&units).ok()?
            }
            _ => return None,
        };
        labels.push(text);
    }
    Some(labels)
}

/// An object's kind, its count and where what follows its marker starts.
fn object(bytes: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let marker = *bytes.get(at)?;
    let (kind, count) = (marker >> 4, marker & 0xF);
    if count != 0xF {
        return Some((kind, usize::from(count), at + 1));
    }
    // A count of 15 or more follows as an integer object.
    let integer = *bytes.get(at + 1)?;
    if integer >> 4 != 0x1 || integer & 0xF > 3 {
        return None;
    }
    let size = 1usize << (integer & 0xF);
    let count = number(bytes.get(at + 2..at + 2 + size)?)?;
    Some((kind, count, at + 2 + size))
}

/// A big-endian number of up to eight bytes.
fn number(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    let value = bytes
        .iter()
        .fold(0u64, |value, byte| value << 8 | u64::from(*byte));
    usize::try_from(value).ok()
}

/// The property list macOS writes for these tags.
pub fn encode(labels: &[String]) -> Vec<u8> {
    let objects = labels.len() + 1;
    let ref_size = size_for(objects);
    let mut out = b"bplist00".to_vec();
    let mut offsets = vec![out.len()];
    marker(&mut out, 0xA, labels.len());
    for reference in 1..objects {
        out.extend_from_slice(&(reference as u64).to_be_bytes()[8 - ref_size..]);
    }
    for label in labels {
        offsets.push(out.len());
        if label.is_ascii() {
            marker(&mut out, 0x5, label.len());
            out.extend_from_slice(label.as_bytes());
        } else {
            let units: Vec<u16> = label.encode_utf16().collect();
            marker(&mut out, 0x6, units.len());
            units
                .iter()
                .for_each(|unit| out.extend_from_slice(&unit.to_be_bytes()));
        }
    }
    let table = out.len();
    let offset_size = size_for(table);
    for offset in offsets {
        out.extend_from_slice(&(offset as u64).to_be_bytes()[8 - offset_size..]);
    }
    out.extend_from_slice(&[0; 6]);
    out.push(offset_size as u8);
    out.push(ref_size as u8);
    out.extend_from_slice(&(objects as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(&(table as u64).to_be_bytes());
    out
}

/// How many bytes a number up to `largest` takes: 1, 2, 4 or 8.
fn size_for(largest: usize) -> usize {
    match largest {
        0..=0xFF => 1,
        0x100..=0xFFFF => 2,
        0x1_0000..=0xFFFF_FFFF => 4,
        _ => 8,
    }
}

/// An object's marker: its kind, and its count in it or after it.
fn marker(out: &mut Vec<u8>, kind: u8, count: usize) {
    if count < 15 {
        out.push(kind << 4 | count as u8);
        return;
    }
    out.push(kind << 4 | 0xF);
    let size = size_for(count);
    out.push(0x10 | size.trailing_zeros() as u8);
    out.extend_from_slice(&(count as u64).to_be_bytes()[8 - size..]);
}

/// The attribute as it is on an item, link or not; `None` when it has none.
pub fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    xattr::get(path)
}

/// Puts the attribute on an item, or takes it off with `None`.
pub fn write(path: &Path, value: Option<&[u8]>) -> io::Result<()> {
    #[cfg(test)]
    if testing::is_pretend(path) {
        return Err(io::Error::from_raw_os_error(45));
    }
    match value {
        Some(value) => xattr::set(path, value),
        None => xattr::remove(path),
    }
}

/// The tags an item carries; none when they can't be read.
pub fn labels(path: &Path) -> Vec<String> {
    read(path)
        .ok()
        .flatten()
        .and_then(|value| decode(&value))
        .unwrap_or_default()
}

/// Whether an error says the disk can't keep the attribute at all.
pub fn is_unsupported(error: &io::Error) -> bool {
    // `ENOTSUP` and `EOPNOTSUPP`, which macOS numbers apart.
    matches!(error.raw_os_error(), Some(45 | 102)) || error.kind() == io::ErrorKind::Unsupported
}

/// The calls that read and write an attribute, never through a link.
#[cfg(target_os = "macos")]
mod xattr {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c_path(path: &Path) -> io::Result<CString> {
        CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
    }

    fn c_name() -> CString {
        CString::new(super::ATTRIBUTE).expect("no NUL in the name")
    }

    pub fn get(path: &Path) -> io::Result<Option<Vec<u8>>> {
        let (path, name) = (c_path(path)?, c_name());
        loop {
            // SAFETY: both strings end in a NUL; a null buffer asks for the size.
            let size = unsafe {
                libc::getxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            };
            if size < 0 {
                let error = io::Error::last_os_error();
                return match error.raw_os_error() {
                    Some(libc::ENOATTR) => Ok(None),
                    _ => Err(error),
                };
            }
            let mut value = vec![0u8; size as usize];
            // SAFETY: `value` has room for `value.len()` bytes.
            let read = unsafe {
                libc::getxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            };
            if read >= 0 {
                value.truncate(read as usize);
                return Ok(Some(value));
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                // It grew in the meantime: ask again.
                Some(libc::ERANGE) => continue,
                Some(libc::ENOATTR) => return Ok(None),
                _ => return Err(error),
            }
        }
    }

    pub fn set(path: &Path, value: &[u8]) -> io::Result<()> {
        let (path, name) = (c_path(path)?, c_name());
        // SAFETY: both strings end in a NUL and `value` is `value.len()` bytes.
        let done = unsafe {
            libc::setxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if done == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        let (path, name) = (c_path(path)?, c_name());
        // SAFETY: both strings end in a NUL.
        let done = unsafe { libc::removexattr(path.as_ptr(), name.as_ptr(), libc::XATTR_NOFOLLOW) };
        let error = io::Error::last_os_error();
        match done {
            0 => Ok(()),
            // Already gone is what was asked for.
            _ if error.raw_os_error() == Some(libc::ENOATTR) => Ok(()),
            _ => Err(error),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod xattr {
    use std::io;
    use std::path::Path;

    pub fn get(_path: &Path) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    pub fn set(_path: &Path, _value: &[u8]) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub fn remove(_path: &Path) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
pub mod testing {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static PRETEND: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    /// Makes every item in a folder answer as on a disk with no room for
    /// attributes: writing one fails with `ENOTSUP`, as such a disk's does.
    pub fn pretend_unsupported(folder: &Path) {
        PRETEND.with(|folders| folders.borrow_mut().push(folder.to_path_buf()));
    }

    pub(super) fn is_pretend(path: &Path) -> bool {
        PRETEND.with(|folders| {
            folders
                .borrow()
                .iter()
                .any(|folder| path.starts_with(folder))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// What Foundation itself wrote when asked to tag a file with the seven
    /// and a tag of the user's own, in this order (read with `xattr -px`).
    const FOUNDATION: &str = "62706C6973743030A80102030405060708555265640A36584F72616E67650A37\
        5859656C6C6F770A3557477265656E0A3256426C75650A3458507572706C650A\
        3356477261790A3158437573746F6D0A30081117202931384148000000000000\
        0101000000000000000900000000000000000000000000000051";
    /// And for `Red` and a name outside ASCII, `Café ☕`.
    const UNICODE: &str = "62706C6973743030A20102555265640A366800430061006600E900202615000A\
        0030080B11000000000000010100000000000000030000000000000000000000\
        0000000022";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
            .collect()
    }

    fn strings(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|label| label.to_string()).collect()
    }

    #[test]
    fn finder_numbers_the_seven_colours_as_foundation_writes_them() {
        let all: Vec<String> = (0..7).map(label).collect();
        let expected = [
            "Red\n6",
            "Orange\n7",
            "Yellow\n5",
            "Green\n2",
            "Blue\n4",
            "Purple\n3",
            "Gray\n1",
        ];
        assert_eq!(all, expected);
        let mut with_custom = all.clone();
        with_custom.push("Custom\n0".to_string());
        assert_eq!(decode(&bytes(FOUNDATION)), Some(with_custom.clone()));
        // Written back, it is byte for byte what Foundation wrote.
        assert_eq!(encode(&with_custom), bytes(FOUNDATION));
        let unicode = strings(&["Red\n6", "Café ☕\n0"]);
        assert_eq!(decode(&bytes(UNICODE)), Some(unicode.clone()));
        assert_eq!(encode(&unicode), bytes(UNICODE));
    }

    #[test]
    fn long_lists_and_long_names_go_round_and_anything_else_is_not_tags() {
        let many: Vec<String> = (0..300)
            .map(|n| format!("tag {n} {}", "x".repeat(n)))
            .collect();
        assert_eq!(decode(&encode(&many)), Some(many));
        assert_eq!(decode(&encode(&[])), Some(Vec::new()));
        assert_eq!(decode(b"Red,Blue"), None);
        assert_eq!(decode(&bytes(FOUNDATION)[..60]), None);
        // A property list that is a string, not an array.
        let mut string = encode(&strings(&["Red\n6"]));
        string[8] = 0x50;
        assert_eq!(decode(&string), None);
    }

    #[test]
    fn a_tag_is_one_of_the_seven_by_its_name_whatever_its_case_or_number() {
        let labels = strings(&["red\n6", "Work", "Blue\n0", "Custom\n0"]);
        assert_eq!(mask(&labels), 0b1_0001);
        assert_eq!(name("Red\n6"), "Red");
        assert_eq!(name("Work"), "Work");
        assert_eq!(index_of("GRAY"), Some(6));
        assert_eq!(index_of("Work"), None);
    }

    #[test]
    fn adding_and_removing_one_of_the_seven_keeps_the_others_untouched() {
        let labels = strings(&["Work", "Blue\n4", "Custom\n0"]);
        assert_eq!(
            with(&labels, 0),
            strings(&["Work", "Blue\n4", "Custom\n0", "Red\n6"])
        );
        // Already there: nothing changes.
        assert_eq!(with(&labels, 4), labels);
        assert_eq!(without(&labels, 4), strings(&["Work", "Custom\n0"]));
        assert_eq!(without(&labels, 0), labels);
        let all = strings(&["Red\n6", "Work", "Gray\n1"]);
        assert_eq!(without_known(&all), strings(&["Work"]));
    }

    #[test]
    fn find_takes_tag_terms_whatever_their_case_and_keeps_the_other_words() {
        assert_eq!(
            split_query("TAG:red holiday"),
            (vec!["red".to_string()], "holiday".to_string())
        );
        assert_eq!(
            split_query("snaps tag:Blue  tag:work 2024"),
            (strings(&["blue", "work"]), "snaps 2024".to_string())
        );
        assert_eq!(split_query("bench"), (Vec::new(), "bench".to_string()));
        // `tag:` alone names no tag: it is a word like any other.
        assert_eq!(split_query("tag:"), (Vec::new(), "tag:".to_string()));
        let labels = strings(&["Red\n6", "Work"]);
        assert!(carries(&labels, &strings(&["red", "work"])));
        assert!(!carries(&labels, &strings(&["red", "blue"])));
        assert!(carries(&labels, &[]));
    }

    #[test]
    fn the_attribute_is_read_written_and_taken_off_an_item_and_a_link_itself() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        fs::write(&file, "hello").unwrap();
        assert_eq!(read(&file).unwrap(), None);
        assert!(labels(&file).is_empty());
        let value = encode(&strings(&["Red\n6", "Custom\n0"]));
        write(&file, Some(&value)).unwrap();
        assert_eq!(read(&file).unwrap(), Some(value));
        assert_eq!(labels(&file), strings(&["Red\n6", "Custom\n0"]));
        // The file itself is untouched.
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");

        // A link is tagged itself, not what it points at.
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(read(&link).unwrap(), None);
        write(&link, Some(&encode(&strings(&["Blue\n4"])))).unwrap();
        assert_eq!(labels(&link), strings(&["Blue\n4"]));
        assert_eq!(labels(&file), strings(&["Red\n6", "Custom\n0"]));

        write(&file, None).unwrap();
        assert_eq!(read(&file).unwrap(), None);
        // Taking off what isn't there is no error.
        write(&file, None).unwrap();
        let gone = dir.path().join("gone");
        assert_eq!(read(&gone).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_disk_that_cannot_keep_the_attribute_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        fs::write(&file, "hello").unwrap();
        testing::pretend_unsupported(dir.path());
        let error = write(&file, Some(&encode(&strings(&["Red\n6"])))).unwrap_err();
        assert!(is_unsupported(&error));
        assert_eq!(read(&file).unwrap(), None);
        // macOS has two numbers for it.
        assert!(is_unsupported(&io::Error::from_raw_os_error(45)));
        assert!(is_unsupported(&io::Error::from_raw_os_error(102)));
        assert!(!is_unsupported(&io::Error::from_raw_os_error(13)));
    }
}
