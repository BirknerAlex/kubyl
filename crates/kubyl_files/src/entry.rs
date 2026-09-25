//! A file or directory in a listing (local or in a container), and formatting helpers.

use jiff::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    /// Devices, sockets, pipes.
    Other,
}

/// One entry of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// Permission bits (`0o755`); the file type is in `kind`.
    pub mode: u32,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub modified: Option<Timestamp>,
    pub link_target: Option<String>,
    /// For symlinks: whether the target is a directory (navigable).
    pub link_to_dir: bool,
}

impl Entry {
    pub fn new(name: impl Into<String>, kind: EntryKind) -> Self {
        Self {
            name: name.into(),
            kind,
            size: 0,
            mode: 0,
            owner: None,
            group: None,
            modified: None,
            link_target: None,
            link_to_dir: false,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind == EntryKind::Dir || (self.kind == EntryKind::Symlink && self.link_to_dir)
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }

    /// `drwxr-xr-x`.
    pub fn mode_string(&self) -> String {
        mode_string(self.kind, self.mode)
    }
}

/// `drwxr-xr-x` for a kind and permission bits.
pub fn mode_string(kind: EntryKind, mode: u32) -> String {
    let mut out = String::with_capacity(10);
    out.push(match kind {
        EntryKind::Dir => 'd',
        EntryKind::Symlink => 'l',
        EntryKind::File => '-',
        EntryKind::Other => '?',
    });
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        let exec = bits & 1 != 0;
        // setuid/setgid/sticky.
        let special = match shift {
            6 => mode & 0o4000 != 0,
            3 => mode & 0o2000 != 0,
            _ => mode & 0o1000 != 0,
        };
        out.push(match (exec, special, shift) {
            (true, true, 0) => 't',
            (false, true, 0) => 'T',
            (true, true, _) => 's',
            (false, true, _) => 'S',
            (true, false, _) => 'x',
            (false, false, _) => '-',
        });
    }
    out
}

/// Sorts a listing: directories first, then by name (case-insensitive).
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir()
            .cmp(&a.is_dir())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// `844 B`, `2.1 KB`, `388 MB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Joins a POSIX directory and a name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// The parent of a POSIX path (`/` stays `/`).
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(ix) => trimmed[..ix].to_string(),
    }
}

/// The last component of a POSIX path.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Normalizes a POSIX path: collapses `//`, resolves `.` and `..`, keeps it absolute.
pub fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// The breadcrumb components of an absolute path: `/app/config` → `["/", "app", "config"]`
/// with the path each one leads to.
pub fn crumbs(path: &str) -> Vec<(String, String)> {
    let mut out = vec![("/".to_string(), "/".to_string())];
    let mut current = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        current.push('/');
        current.push_str(part);
        out.push((part.to_string(), current.clone()));
    }
    out
}

/// A name that doesn't collide with `taken`: `a.txt` → `a (1).txt`, `dir` → `dir (1)`.
pub fn keep_both_name(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(ix) if ix > 0 => (&name[..ix], &name[ix..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_strings() {
        assert_eq!(mode_string(EntryKind::Dir, 0o755), "drwxr-xr-x");
        assert_eq!(mode_string(EntryKind::File, 0o644), "-rw-r--r--");
        assert_eq!(mode_string(EntryKind::Dir, 0o1777), "drwxrwxrwt");
        assert_eq!(mode_string(EntryKind::File, 0o4755), "-rwsr-xr-x");
        assert_eq!(mode_string(EntryKind::Symlink, 0o777), "lrwxrwxrwx");
    }

    #[test]
    fn sorts_directories_first() {
        let mut entries = vec![
            Entry::new("b.txt", EntryKind::File),
            Entry::new("Zeta", EntryKind::Dir),
            Entry::new("a.txt", EntryKind::File),
            Entry::new("alpha", EntryKind::Dir),
        ];
        sort_entries(&mut entries);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Zeta", "a.txt", "b.txt"]);
    }

    #[test]
    fn sizes_and_paths() {
        assert_eq!(human_size(844), "844 B");
        assert_eq!(human_size(2150), "2.1 KB");
        assert_eq!(human_size(388 * 1024 * 1024), "388 MB");
        assert_eq!(join("/app", "config"), "/app/config");
        assert_eq!(join("/", "app"), "/app");
        assert_eq!(parent("/app/config"), "/app");
        assert_eq!(parent("/app"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(file_name("/app/config/"), "config");
        assert_eq!(normalize("/app//config/../logs/."), "/app/logs");
        assert_eq!(normalize("/.."), "/");
        let crumbs = crumbs("/app/config");
        assert_eq!(crumbs[2], ("config".into(), "/app/config".into()));
    }

    #[test]
    fn keep_both_names() {
        let taken = |n: &str| n == "a (1).txt";
        assert_eq!(keep_both_name("a.txt", &taken), "a (2).txt");
        assert_eq!(keep_both_name("certs", &|_| false), "certs (1)");
        assert_eq!(keep_both_name(".env", &|_| false), ".env (1)");
    }
}
