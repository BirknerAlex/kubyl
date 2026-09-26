//! Reading and saving kubeconfig files. Blocking: run on a background thread.
//!
//! A save refuses to overwrite a file whose SHA-256 changed since it was loaded (another tool
//! wrote it), copies the old contents to a timestamped backup (mode 0600, the last N per file),
//! and replaces the file atomically (a temp file in the same folder, fsync, rename). The hash is
//! checked before the backup and again right before the rename; only a write in the instant
//! between that last check and the rename could still be lost (there is no compare-and-rename).
//! Symlinks (a dotfiles repo's `~/.kube/config`) are followed, so the link stays. New files and
//! files with inline credentials are 0600; others keep their mode.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// A file as it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    /// SHA-256 of the bytes, hex. `None`: the file doesn't exist.
    pub hash: Option<String>,
}

/// SHA-256 of `bytes` as hex.
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Reads a kubeconfig. A missing file is an empty snapshot without a hash.
pub fn read(path: &Path) -> std::io::Result<Snapshot> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let hash = hash(&bytes);
            let text = String::from_utf8(bytes).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "the file isn't UTF-8 text")
            })?;
            Ok(Snapshot {
                text,
                hash: Some(hash),
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Snapshot {
            text: String::new(),
            hash: None,
        }),
        Err(err) => Err(err),
    }
}

/// Where backups go and how many are kept per file.
#[derive(Clone, Debug)]
pub struct Backups {
    pub dir: PathBuf,
    pub keep: usize,
}

/// How to save.
#[derive(Clone, Debug)]
pub struct SaveOptions {
    /// The hash the file had when it was loaded; `None` for a new file (which must not exist).
    pub expected: Option<String>,
    pub backups: Option<Backups>,
    /// Write mode 0600 (new files and files with inline credentials).
    pub private: bool,
}

/// Why a save didn't happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    /// The file changed on disk since it was loaded (`current`: its hash now, `None` if it was
    /// deleted).
    Changed {
        current: Option<String>,
    },
    /// A new file would overwrite an existing one.
    Exists,
    Io(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Changed { current: Some(_) } => {
                f.write_str("the file changed on disk since it was opened")
            }
            SaveError::Changed { current: None } => {
                f.write_str("the file was deleted since it was opened")
            }
            SaveError::Exists => f.write_str("a file with that name already exists"),
            SaveError::Io(err) => f.write_str(err),
        }
    }
}

/// What a save did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The file that was written (the symlink's target for a symlink).
    pub written: PathBuf,
    pub hash: String,
    pub backup: Option<PathBuf>,
    /// The file's permission bits now (Unix).
    pub mode: Option<u32>,
}

fn io(err: std::io::Error) -> SaveError {
    SaveError::Io(err.to_string())
}

impl From<std::io::Error> for SaveError {
    fn from(err: std::io::Error) -> Self {
        io(err)
    }
}

/// The file's hash now; `None` if it doesn't exist.
fn hash_now(path: &Path) -> Result<Option<String>, SaveError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(hash(&bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io(err)),
    }
}

/// The file a write goes to: a symlink's target (so the link stays a link).
pub fn target(path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    }
}

/// Saves `text` to `path`. See the module docs.
pub fn save(path: &Path, text: &str, options: &SaveOptions) -> Result<Saved, SaveError> {
    let written = target(path);
    let current = match std::fs::read(&written) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(io(err)),
    };
    let current_hash = current.as_deref().map(hash);
    match (&options.expected, &current_hash) {
        (None, Some(_)) => return Err(SaveError::Exists),
        (Some(expected), current) if current.as_ref() != Some(expected) => {
            return Err(SaveError::Changed {
                current: current.clone(),
            });
        }
        _ => {}
    }
    let backup = match (&options.backups, &current) {
        (Some(backups), Some(bytes)) => Some(backup(&written, bytes, backups).map_err(io)?),
        _ => None,
    };
    let mode = mode_for(&written, current.is_none() || options.private);
    replace(&written, text.as_bytes(), mode, || {
        // Again right before the rename: another tool may have written the file meanwhile.
        match (hash_now(&written)?, &current_hash) {
            (now, expected) if now == *expected => Ok(()),
            (Some(_), None) => Err(SaveError::Exists),
            (now, _) => Err(SaveError::Changed { current: now }),
        }
    })?;
    Ok(Saved {
        hash: hash(text.as_bytes()),
        mode: current_mode(&written),
        written,
        backup,
    })
}

/// The mode a written file gets: 0600 when `private`, else the existing file's.
fn mode_for(path: &Path, private: bool) -> Option<u32> {
    if private {
        return Some(0o600);
    }
    current_mode(path)
}

#[cfg(unix)]
fn current_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn current_mode(_path: &Path) -> Option<u32> {
    None
}

/// Writes a temp file next to `path` and renames it over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    replace(path, bytes, mode, || Ok(()))
}

/// [`write_atomic`], with `check` run after the temp file is written and synced, right before
/// the rename; an error from it leaves `path` alone.
fn replace<E: From<std::io::Error>>(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    check: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kubeconfig".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{name}.kubyl-{}-{nanos}.tmp", std::process::id()));
    let result = (|| -> Result<(), E> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            // Start private; the final mode is set before the rename.
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        drop(file);
        check()?;
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Ok(dir) = std::fs::File::open(dir) {
            dir.sync_all().ok();
        }
        Ok(())
    })();
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

/// `<stem>-<hash of the path>-` : the prefix of a file's backups.
pub fn backup_prefix(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kubeconfig".into());
    let stem: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let path_hash = hash(path.to_string_lossy().as_bytes());
    format!("{stem}-{}-", &path_hash[..8])
}

/// Copies `bytes` (the file before the save) to the backup folder and prunes old backups.
fn backup(path: &Path, bytes: &[u8], backups: &Backups) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(&backups.dir)?;
    let prefix = backup_prefix(path);
    let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
    // Several saves in one second: counters only go up (pruning may have removed lower ones).
    let highest = backups_with_prefix(&backups.dir, &prefix)
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            let (s, n) = split_counter(&name[prefix.len()..]);
            (s == stamp).then_some(n.max(1))
        })
        .max();
    let mut target = match highest {
        None => backups.dir.join(format!("{prefix}{stamp}.yaml")),
        Some(n) => backups.dir.join(format!("{prefix}{stamp}-{}.yaml", n + 1)),
    };
    let mut n = highest.unwrap_or(1) + 2;
    while target.exists() {
        target = backups.dir.join(format!("{prefix}{stamp}-{n}.yaml"));
        n += 1;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&target)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    prune(&backups.dir, &prefix, backups.keep.max(1))?;
    Ok(target)
}

/// The backups of `path`, oldest first.
pub fn backups_of(dir: &Path, path: &Path) -> Vec<PathBuf> {
    backups_with_prefix(dir, &backup_prefix(&target(path)))
}

/// Backups starting with `prefix`, oldest first: timestamps sort by name, and `-2` suffixes
/// after the plain name of the same second.
fn backups_with_prefix(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
        })
        .collect();
    found.sort_by_key(|p| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        split_counter(&name[prefix.len()..])
    });
    found
}

fn split_counter(rest: &str) -> (String, u32) {
    let rest = rest.trim_end_matches(".yaml");
    // `20260926-140512` or `20260926-140512-3`.
    match rest.rsplit_once('-') {
        Some((stamp, n)) if stamp.contains('-') => (stamp.to_string(), n.parse().unwrap_or(0)),
        _ => (rest.to_string(), 0),
    }
}

fn prune(dir: &Path, prefix: &str, keep: usize) -> std::io::Result<()> {
    let found = backups_with_prefix(dir, prefix);
    let excess = found.len().saturating_sub(keep);
    for old in &found[..excess] {
        std::fs::remove_file(old)?;
    }
    Ok(())
}

/// The folder for Kubyl-owned kubeconfigs (pasted or created in Kubyl).
pub fn owned_dir() -> PathBuf {
    kubyl_settings::config_dir().join("kubeconfigs")
}

/// The folder for backups.
pub fn backup_dir() -> PathBuf {
    kubyl_settings::config_dir().join("kubeconfig-backups")
}

/// Whether Kubyl owns `path` (it's in `owned_dir`).
pub fn is_owned(path: &Path, owned_dir: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    path.parent()
        .is_some_and(|parent| canon(parent) == canon(owned_dir))
}

/// A free `<dir>/<name>.yaml` for a new Kubyl-owned kubeconfig.
pub fn new_owned_path(dir: &Path, name: &str) -> PathBuf {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches(['-', '.']);
    let stem = if stem.is_empty() { "kubeconfig" } else { stem };
    let mut path = dir.join(format!("{stem}.yaml"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.yaml"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(expected: Option<String>, dir: &Path, keep: usize) -> SaveOptions {
        SaveOptions {
            expected,
            backups: Some(Backups {
                dir: dir.join("backups"),
                keep,
            }),
            private: false,
        }
    }

    #[test]
    fn saves_atomically_with_backups_and_a_hash_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        // New file: must not exist yet; 0600.
        let saved = save(&path, "a: 1\n", &options(None, dir.path(), 2)).unwrap();
        assert_eq!(saved.backup, None);
        #[cfg(unix)]
        assert_eq!(saved.mode, Some(0o600));
        assert_eq!(
            save(&path, "x", &options(None, dir.path(), 2)),
            Err(SaveError::Exists)
        );

        // Existing file with a looser mode keeps it unless it gets credentials.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let snap = read(&path).unwrap();
        let saved = save(&path, "a: 2\n", &options(snap.hash.clone(), dir.path(), 2)).unwrap();
        #[cfg(unix)]
        assert_eq!(saved.mode, Some(0o644));
        let backup = saved.backup.unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "a: 1\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // A stale hash is refused and nothing changes.
        let stale = snap.hash.clone();
        assert!(matches!(
            save(&path, "a: 3\n", &options(stale, dir.path(), 2)),
            Err(SaveError::Changed { current: Some(_) })
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a: 2\n");

        // Private saves tighten the mode; only `keep` backups stay.
        for i in 3..9 {
            let snap = read(&path).unwrap();
            let mut opts = options(snap.hash, dir.path(), 2);
            opts.private = true;
            let saved = save(&path, &format!("a: {i}\n"), &opts).unwrap();
            if cfg!(unix) {
                assert_eq!(saved.mode, Some(0o600));
            }
        }
        let kept = backups_of(&dir.path().join("backups"), &path);
        assert_eq!(kept.len(), 2);
        assert_eq!(
            std::fs::read_to_string(kept.last().unwrap()).unwrap(),
            "a: 7\n"
        );
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "a: 6\n");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_write_while_saving_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        std::fs::write(&path, "a: 1\n").unwrap();
        // Another tool writes the file after the first check, while the temp file is written.
        let result: Result<(), SaveError> = replace(&path, b"a: 2\n", None, || {
            std::fs::write(&path, "a: 3\n").unwrap();
            match hash_now(&path)? {
                now if now == Some(hash(b"a: 1\n")) => Ok(()),
                now => Err(SaveError::Changed { current: now }),
            }
        });
        assert_eq!(
            result,
            Err(SaveError::Changed {
                current: Some(hash(b"a: 3\n"))
            })
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a: 3\n");
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-config");
        std::fs::write(&real, "a: 1\n").unwrap();
        let link = dir.path().join("config");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let snap = read(&link).unwrap();
        let saved = save(&link, "a: 2\n", &options(snap.hash, dir.path(), 3)).unwrap();
        assert_eq!(saved.written, std::fs::canonicalize(&real).unwrap());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "a: 2\n");
    }

    #[test]
    fn names_new_owned_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = new_owned_path(dir.path(), "kind dev");
        assert_eq!(path.file_name().unwrap(), "kind-dev.yaml");
        std::fs::write(&path, "").unwrap();
        assert_eq!(
            new_owned_path(dir.path(), "kind dev").file_name().unwrap(),
            "kind-dev-2.yaml"
        );
        assert!(is_owned(&path, dir.path()));
        assert!(!is_owned(Path::new("/etc/hosts"), dir.path()));
        assert_eq!(read(&dir.path().join("missing")).unwrap().hash, None);
    }
}
