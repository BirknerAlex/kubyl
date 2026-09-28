use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// `$KUBYL_CONFIG_DIR`, or `<platform config dir>/kubyl`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KUBYL_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("kubyl")
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

/// A write scheduled on the executor. Create it on the thread that produces the contents and
/// move it into the task: tasks can run out of order, and the ticket keeps an older snapshot
/// from replacing a newer one.
pub(crate) struct WriteTicket {
    seq: u64,
}

impl WriteTicket {
    pub(crate) fn new() -> Self {
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
}
