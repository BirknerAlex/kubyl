//! Listing a directory in a container through exec, with whatever the image has:
//!
//! 1. GNU `find -printf` (NUL-separated, exact types, sizes, modes and timestamps);
//! 2. a `stat -c` loop in `sh` (busybox and coreutils);
//! 3. GNU `ls -la --time-style=full-iso`;
//! 4. busybox `ls -lae` (or plain `ls -la`, minute precision).
//!
//! The commands pass the directory as a positional argument (`sh -c '…' sh DIR`), never
//! spliced into the script, so any file name is safe. The parsers are pure and unit-tested.

use jiff::Timestamp;

use crate::entry::{Entry, EntryKind};

/// How to list directories in a container, from the capability probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingMethod {
    FindPrintf,
    StatLoop,
    LsFullIso,
    Ls,
}

const FIND_SCRIPT: &str =
    "find \"$1\" -mindepth 1 -maxdepth 1 -printf '%y\\t%Y\\t%s\\t%m\\t%u\\t%g\\t%T@\\t%l\\t%f\\0'";

const STAT_SCRIPT: &str = r#"cd -- "$1" || exit 2
for f in * .[!.]* ..?*; do
  [ -e "$f" ] || [ -L "$f" ] || continue
  s=$(stat -c '%F	%s	%a	%U	%G	%Y' -- "$f" 2>/dev/null) || continue
  t=''; d=0
  if [ -L "$f" ]; then t=$(readlink -- "$f"); [ -d "$f" ] && d=1; fi
  printf '%s\t%s\t%s\t%s\0' "$s" "$d" "$t" "$f"
done"#;

/// The exec command that lists `dir` with `method`.
pub fn command(method: ListingMethod, dir: &str) -> Vec<String> {
    let script = match method {
        ListingMethod::FindPrintf => FIND_SCRIPT,
        ListingMethod::StatLoop => STAT_SCRIPT,
        ListingMethod::LsFullIso => "ls -la --time-style=full-iso -- \"$1\"",
        ListingMethod::Ls => "ls -lae -- \"$1\" 2>/dev/null || ls -la -- \"$1\"",
    };
    vec![
        "sh".into(),
        "-c".into(),
        script.into(),
        "sh".into(),
        dir.into(),
    ]
}

/// Parses the output of `method`.
pub fn parse(method: ListingMethod, output: &[u8]) -> Vec<Entry> {
    let text = String::from_utf8_lossy(output);
    match method {
        ListingMethod::FindPrintf => parse_find(&text),
        ListingMethod::StatLoop => parse_stat(&text),
        ListingMethod::LsFullIso | ListingMethod::Ls => parse_ls(&text),
    }
}

fn find_kind(c: &str) -> EntryKind {
    match c {
        "d" => EntryKind::Dir,
        "f" => EntryKind::File,
        "l" => EntryKind::Symlink,
        _ => EntryKind::Other,
    }
}

fn epoch(seconds: &str) -> Option<Timestamp> {
    let (secs, frac) = seconds.split_once('.').unwrap_or((seconds, "0"));
    let secs: i64 = secs.parse().ok()?;
    let mut nanos_text: String = frac.chars().take(9).collect();
    while nanos_text.len() < 9 {
        nanos_text.push('0');
    }
    let nanos: i32 = nanos_text.parse().unwrap_or(0);
    Timestamp::new(secs, nanos).ok()
}

/// `%y\t%Y\t%s\t%m\t%u\t%g\t%T@\t%l\t%f\0` records.
pub fn parse_find(text: &str) -> Vec<Entry> {
    text.split('\0')
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.splitn(9, '\t').collect();
            let [
                kind,
                target_kind,
                size,
                mode,
                owner,
                group,
                mtime,
                link,
                name,
            ] = fields.as_slice()
            else {
                return None;
            };
            let kind = find_kind(kind);
            Some(Entry {
                name: name.to_string(),
                kind,
                size: size.parse().unwrap_or(0),
                mode: u32::from_str_radix(mode, 8).unwrap_or(0),
                owner: Some(owner.to_string()),
                group: Some(group.to_string()),
                modified: epoch(mtime),
                link_target: (kind == EntryKind::Symlink).then(|| link.to_string()),
                link_to_dir: kind == EntryKind::Symlink && *target_kind == "d",
            })
        })
        .collect()
}

fn stat_kind(description: &str) -> EntryKind {
    match description {
        "directory" => EntryKind::Dir,
        "symbolic link" => EntryKind::Symlink,
        d if d.contains("regular") => EntryKind::File,
        _ => EntryKind::Other,
    }
}

/// `%F\t%s\t%a\t%U\t%G\t%Y\t<dir flag>\t<link target>\t<name>\0` records.
pub fn parse_stat(text: &str) -> Vec<Entry> {
    text.split('\0')
        .map(|r| r.trim_start_matches('\n'))
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.splitn(9, '\t').collect();
            let [kind, size, mode, owner, group, mtime, dir, link, name] = fields.as_slice() else {
                return None;
            };
            let kind = stat_kind(kind);
            Some(Entry {
                name: name.to_string(),
                kind,
                size: size.parse().unwrap_or(0),
                mode: u32::from_str_radix(mode, 8).unwrap_or(0),
                owner: Some(owner.to_string()),
                group: Some(group.to_string()),
                modified: epoch(mtime),
                link_target: (kind == EntryKind::Symlink).then(|| link.to_string()),
                link_to_dir: kind == EntryKind::Symlink && *dir == "1",
            })
        })
        .collect()
}

/// `drwxr-xr-x` → kind and permission bits.
pub fn parse_mode_string(mode: &str) -> Option<(EntryKind, u32)> {
    let bytes = mode.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let kind = match bytes[0] {
        b'd' => EntryKind::Dir,
        b'-' => EntryKind::File,
        b'l' => EntryKind::Symlink,
        b'c' | b'b' | b'p' | b's' => EntryKind::Other,
        _ => return None,
    };
    let mut bits = 0u32;
    for (i, &c) in bytes[1..10].iter().enumerate() {
        let bit = 8 - i as u32;
        let set = match i % 3 {
            0 => c == b'r',
            1 => c == b'w',
            _ => matches!(c, b'x' | b's' | b't'),
        };
        if set {
            bits |= 1 << bit;
        }
        match (i, c) {
            (2, b's' | b'S') => bits |= 0o4000,
            (5, b's' | b'S') => bits |= 0o2000,
            (8, b't' | b'T') => bits |= 0o1000,
            _ => {}
        }
    }
    Some((kind, bits))
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `ls -la` output: GNU `--time-style=full-iso`, busybox `-e` (`Wed Sep 25 10:42:17 2026`) or
/// the default (`Sep 25 10:42` / `Sep 25  2025`).
pub fn parse_ls(text: &str) -> Vec<Entry> {
    let now = Timestamp::now();
    text.lines()
        .filter(|l| !l.starts_with("total "))
        .filter_map(|line| parse_ls_line(line, now))
        .filter(|e| e.name != "." && e.name != "..")
        .collect()
}

fn parse_ls_line(line: &str, now: Timestamp) -> Option<Entry> {
    let mut rest = line;
    let mut next = || -> Option<&str> {
        rest = rest.trim_start();
        let end = rest.find(' ').unwrap_or(rest.len());
        let (word, tail) = rest.split_at(end);
        rest = tail;
        (!word.is_empty()).then_some(word)
    };
    let (kind, mode) = parse_mode_string(next()?)?;
    let _links = next()?;
    let owner = next()?.to_string();
    let group = next()?.to_string();
    let size_word = next()?;
    // Devices show `major, minor` instead of a size.
    let size = if size_word.ends_with(',') {
        next()?;
        0
    } else {
        size_word.parse().unwrap_or(0)
    };
    let first = next()?;
    let modified = if first.len() == 10 && first.as_bytes()[4] == b'-' {
        // full-iso: `2026-09-24 10:42:17.123456789 +0000`
        let time = next()?;
        let zone = next()?;
        let text = format!("{first} {time} {zone}");
        jiff::fmt::strtime::parse("%Y-%m-%d %H:%M:%S%.f %z", &text)
            .ok()
            .and_then(|t| t.to_timestamp().ok())
    } else if first.len() == 3 && MONTHS.contains(&first) {
        // `Sep 25 10:42` or `Sep 25  2025`
        let day = next()?;
        let time_or_year = next()?;
        let year = now.to_zoned(jiff::tz::TimeZone::UTC).year();
        let text = if time_or_year.contains(':') {
            format!("{year} {first} {day} {time_or_year}")
        } else {
            format!("{time_or_year} {first} {day} 00:00")
        };
        jiff::fmt::strtime::parse("%Y %b %d %H:%M", &text)
            .ok()
            .and_then(|t| t.to_datetime().ok())
            .and_then(|dt| dt.to_zoned(jiff::tz::TimeZone::UTC).ok())
            .map(|z| z.timestamp())
    } else {
        // busybox -e: `Wed Sep 25 10:42:17 2026`
        let month = next()?;
        let day = next()?;
        let time = next()?;
        let year = next()?;
        let text = format!("{year} {month} {day} {time}");
        jiff::fmt::strtime::parse("%Y %b %d %H:%M:%S", &text)
            .ok()
            .and_then(|t| t.to_datetime().ok())
            .and_then(|dt| dt.to_zoned(jiff::tz::TimeZone::UTC).ok())
            .map(|z| z.timestamp())
    };
    let name_part = rest.strip_prefix(' ').unwrap_or(rest);
    let (name, link_target) = if kind == EntryKind::Symlink {
        match name_part.split_once(" -> ") {
            Some((name, target)) => (name.to_string(), Some(target.to_string())),
            None => (name_part.to_string(), None),
        }
    } else {
        (name_part.to_string(), None)
    };
    if name.is_empty() {
        return None;
    }
    Some(Entry {
        name,
        kind,
        size,
        mode,
        owner: Some(owner),
        group: Some(group),
        modified,
        link_target,
        link_to_dir: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_find_printf_records() {
        let out = "d\td\t4096\t755\tapp\tapp\t1727260937.5\t\tcerts\0\
                   f\tf\t6250\t644\tapp\tapp\t1727260937.0\t\tapplication.yaml\0\
                   l\td\t10\t777\troot\troot\t1727260937.0\t/etc/ssl\tssl\0\
                   f\tf\t3\t600\tapp\tapp\t1727260937.0\t\twith\ttab\0";
        let entries = parse_find(out);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].kind, EntryKind::Dir);
        assert_eq!(entries[0].mode, 0o755);
        assert_eq!(entries[1].size, 6250);
        assert_eq!(
            entries[1].modified.unwrap().to_string(),
            "2024-09-25T10:42:17Z"
        );
        assert!(entries[2].is_dir());
        assert_eq!(entries[2].link_target.as_deref(), Some("/etc/ssl"));
        assert_eq!(entries[3].name, "with\ttab");
    }

    #[test]
    fn parses_stat_loop_records() {
        let out = "directory\t4096\t755\troot\troot\t1727260937\t0\t\tetc\0\
                   regular empty file\t0\t644\tapp\tapp\t1727260937\t0\t\t.env\0\
                   symbolic link\t7\t777\troot\troot\t1727260937\t1\tusr/lib\tlib\0";
        let entries = parse_stat(out);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].kind, EntryKind::File);
        assert!(entries[1].is_hidden());
        assert!(entries[2].is_dir());
        assert_eq!(entries[2].link_target.as_deref(), Some("usr/lib"));
    }

    #[test]
    fn parses_gnu_ls_full_iso() {
        let out = "total 12\n\
drwxr-xr-x 2 app app 4096 2024-09-25 10:42:17.123456789 +0000 .\n\
drwxr-xr-x 3 app app 4096 2024-09-25 10:42:17.123456789 +0000 ..\n\
-rw-r--r-- 1 app app  612 2024-09-25 10:42:17.000000000 +0200 overrides yaml.txt\n\
lrwxrwxrwx 1 root root  12 2024-09-25 10:42:17.000000000 +0000 current -> releases/v2\n";
        let entries = parse_ls(out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "overrides yaml.txt");
        assert_eq!(entries[0].size, 612);
        assert_eq!(
            entries[0].modified.unwrap().to_string(),
            "2024-09-25T08:42:17Z"
        );
        assert_eq!(entries[1].kind, EntryKind::Symlink);
        assert_eq!(entries[1].name, "current");
        assert_eq!(entries[1].link_target.as_deref(), Some("releases/v2"));
    }

    #[test]
    fn parses_busybox_ls() {
        let out = "-rw-r--r--    1 root     root          1234 Wed Sep 25 10:42:17 2024 notes.md\n\
crw-rw-rw-    1 root     root        1,   3 Wed Sep 25 10:42:17 2024 null\n";
        let entries = parse_ls(out);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "notes.md");
        assert_eq!(entries[0].size, 1234);
        assert_eq!(
            entries[0].modified.unwrap().to_string(),
            "2024-09-25T10:42:17Z"
        );
        assert_eq!(entries[1].kind, EntryKind::Other);
        let short = parse_ls("drwxrwxrwt    2 root root 40 Sep 25  2023 tmp\n");
        assert_eq!(short[0].mode, 0o1777);
        assert!(short[0].modified.is_some());
    }

    #[test]
    fn mode_strings_round_trip() {
        assert_eq!(
            parse_mode_string("drwxr-xr-x"),
            Some((EntryKind::Dir, 0o755))
        );
        assert_eq!(
            parse_mode_string("-rwsr-x---"),
            Some((EntryKind::File, 0o4750))
        );
        assert_eq!(
            parse_mode_string("drwxrwxrwt"),
            Some((EntryKind::Dir, 0o1777))
        );
        assert_eq!(parse_mode_string("total"), None);
    }

    #[test]
    fn commands_pass_the_directory_as_an_argument() {
        let cmd = command(ListingMethod::StatLoop, "/app/it's here");
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[4], "/app/it's here");
        assert!(!cmd[2].contains("it's"));
    }
}
