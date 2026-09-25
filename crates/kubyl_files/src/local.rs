//! The local side: directory listings, hashes and bookmarks. Blocking I/O; callers run it on
//! the background executor.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::entry::{Entry, EntryKind, sort_entries};

/// Lists a local directory, sorted (directories first).
pub fn list(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(item.path()) else {
            continue;
        };
        let kind = if meta.is_symlink() {
            EntryKind::Symlink
        } else if meta.is_dir() {
            EntryKind::Dir
        } else if meta.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        let mut entry = Entry::new(name, kind);
        entry.size = if kind == EntryKind::Dir {
            0
        } else {
            meta.len()
        };
        entry.modified = meta
            .modified()
            .ok()
            .and_then(|t| jiff::Timestamp::try_from(t).ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            entry.mode = meta.permissions().mode() & 0o7777;
        }
        if kind == EntryKind::Symlink {
            entry.link_target = std::fs::read_link(item.path())
                .ok()
                .map(|p| p.display().to_string());
            entry.link_to_dir = std::fs::metadata(item.path()).is_ok_and(|m| m.is_dir());
        }
        entries.push(entry);
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of every regular file under `base/name`, keyed by path relative to `base` (with
/// `/` separators), like [`crate::remote::RemoteTarget::sha256_tree`].
pub fn sha256_tree(base: &Path, name: &str) -> std::io::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    let root = base.join(name);
    let mut stack = vec![root];
    while let Some(path) = stack.pop() {
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            for item in std::fs::read_dir(&path)? {
                stack.push(item?.path());
            }
        } else if meta.is_file() {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            out.insert(relative, sha256_file(&path)?);
        }
    }
    Ok(out)
}

/// Total size of files under `path`.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        std::fs::read_dir(path)
            .map(|items| {
                items
                    .filter_map(Result::ok)
                    .map(|i| tree_size(&i.path()))
                    .sum()
            })
            .unwrap_or(0)
    } else {
        meta.len()
    }
}

/// Quick links in the local pane: Home, Downloads, Desktop, Documents, plus the user's own.
pub fn bookmarks(extra: &[String]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for (label, dir) in [
        ("Home", dirs::home_dir()),
        ("Downloads", dirs::download_dir()),
        ("Desktop", dirs::desktop_dir()),
        ("Documents", dirs::document_dir()),
    ] {
        if let Some(dir) = dir {
            out.push((label.to_string(), dir));
        }
    }
    for path in extra {
        let path = PathBuf::from(expand_home(path));
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        out.push((label, path));
    }
    out
}

/// `~/x` → `$HOME/x`.
pub fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ => path.to_string(),
    }
}

/// `~/Downloads/debug` for display.
pub fn display(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_hashes_a_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("certs")).unwrap();
        std::fs::write(dir.path().join("certs/ca.pem"), b"abc").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"").unwrap();
        let entries = list(dir.path()).unwrap();
        assert_eq!(entries[0].name, "certs");
        assert!(entries[0].is_dir());
        assert_eq!(entries[1].name, "b.txt");
        let hashes = sha256_tree(dir.path(), "certs").unwrap();
        assert_eq!(
            hashes["certs/ca.pem"],
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(tree_size(&dir.path().join("certs")), 3);
        assert_eq!(
            sha256_file(&dir.path().join("b.txt")).unwrap(),
            sha256_bytes(b"")
        );
    }
}
