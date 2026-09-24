use std::io::Write as _;
use std::path::{Path, PathBuf};

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
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}
