use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// `$KUBYL_CONFIG_DIR`, or `<platform config dir>/kubyl`.
///
/// Without a platform config dir (no `HOME`) this is a private per-user directory in the temp
/// dir, never a shared, predictable path that another user could pre-create or read.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KUBYL_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    match dirs::config_dir() {
        Some(dir) => dir.join("kubyl"),
        None => private_dir_in(&std::env::temp_dir()),
    }
}

/// A directory under `base` that only the current user can use: `kubyl-<user>` when that is
/// ours and mode 0700, else a fresh `kubyl-<user>-<pid>-<n>` created with mode 0700. If neither works the path is
/// unusable (every write fails) rather than an unverified directory.
fn private_dir_in(base: &Path) -> PathBuf {
    let user = ["USER", "USERNAME", "LOGNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok())
        .unwrap_or_default();
    let user: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    let name = format!("kubyl-{user}");
    let dir = base.join(&name);
    if create_private(&dir, true) {
        return dir;
    }
    // Someone else may have pre-created the pid-based name, so retry with an unguessable suffix.
    for attempt in 0..4u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = base.join(format!("{name}-{}-{nanos:x}{attempt}", std::process::id()));
        if create_private(&dir, false) {
            return dir;
        }
    }
    tracing::warn!(
        "could not create a private config dir in {}",
        base.display()
    );
    // An interior NUL makes every filesystem call on this path fail, so nothing gets written
    // into a directory we could not verify.
    base.join("kubyl-unavailable\0")
}

/// Creates `dir` with mode 0700. With `reuse`, an existing directory is accepted when we own it
/// (only the owner can chmod it) and it is not accessible to others.
fn create_private(dir: &Path, reuse: bool) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => true,
            Err(err) if reuse && err.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).is_ok()
                    && std::fs::symlink_metadata(dir)
                        .is_ok_and(|m| m.is_dir() && m.permissions().mode() & 0o077 == 0)
            }
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = reuse;
        std::fs::create_dir_all(dir).is_ok()
    }
}

/// Writes `contents` to `path` atomically (temp file + rename), creating parent directories.
/// The temp file name is unique per call, so concurrent writers never share it.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_file_name(name);
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

static SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_WRITTEN: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);
/// Number of live tickets: writes scheduled but not finished.
static PENDING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// A write scheduled on the executor. Create it on the thread that produces the contents and
/// move it into the task: tasks can run out of order, and the ticket keeps an older snapshot
/// from replacing a newer one. [`wait_for_writes`] waits until all tickets are dropped.
pub(crate) struct WriteTicket {
    seq: u64,
}

impl WriteTicket {
    pub(crate) fn new() -> Self {
        *PENDING.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Self {
            seq: SEQ.fetch_add(1, Ordering::SeqCst),
        }
    }

    /// Like [`write_atomic`], serialised with other writes, and skipped when a newer write to
    /// `path` already went through.
    pub(crate) fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        let mut guard = LAST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        let last = guard.get_or_insert_with(HashMap::new);
        if last.get(path).is_some_and(|&seq| seq > self.seq) {
            return Ok(());
        }
        write_atomic(path, contents)?;
        last.insert(path.to_path_buf(), self.seq);
        Ok(())
    }
}

impl Drop for WriteTicket {
    fn drop(&mut self) {
        let mut pending = PENDING.0.lock().unwrap_or_else(|e| e.into_inner());
        *pending = pending.saturating_sub(1);
        PENDING.1.notify_all();
    }
}

/// Blocks (up to `timeout`) until every scheduled write is done. Call off the UI thread.
pub(crate) fn wait_for_writes(timeout: Duration) {
    let pending = PENDING.0.lock().unwrap_or_else(|e| e.into_inner());
    let _ = PENDING
        .1
        .wait_timeout_while(pending, timeout, |pending| *pending > 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_writes_use_distinct_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::thread::scope(|scope| {
            for i in 0..16 {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        write_atomic(path, format!("{{\"n\":{i}}}").as_bytes()).unwrap();
                    }
                });
            }
        });
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&written).is_ok());
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "settings.json")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn older_write_does_not_overwrite_newer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let older = WriteTicket::new();
        let newer = WriteTicket::new();
        newer.write(&path, b"new").unwrap();
        older.write(&path, b"old").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn fallback_dir_is_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let base = tempfile::tempdir().unwrap();
        let dir = private_dir_in(base.path());
        assert!(dir.starts_with(base.path()));
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
        // A pre-existing, world-accessible dir of ours is tightened and reused.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(private_dir_in(base.path()), dir);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn unusable_fallback_never_returns_an_unverified_dir() {
        let base = tempfile::tempdir().unwrap();
        let missing = base.path().join("does-not-exist").join("nested");
        let dir = private_dir_in(&missing);
        assert!(write_atomic(&dir.join("state.json"), b"x").is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn waits_for_scheduled_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.json");
        let ticket = WriteTicket::new();
        let handle = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                ticket.write(&path, b"done").unwrap();
            })
        };
        // Other tests' tickets may be live too; they finish quickly.
        wait_for_writes(Duration::from_secs(10));
        assert_eq!(std::fs::read(&path).unwrap(), b"done");
        handle.join().unwrap();
    }
}
