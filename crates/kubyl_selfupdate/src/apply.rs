//! Extracts a verified artifact and replaces the running executable with it.
//!
//! Only ever called with bytes [`crate::download::fetch_artifact`] already checked against the
//! signed manifest's sha256. `self_replace` handles the one truly platform-specific problem
//! (Windows refuses to overwrite a running .exe; it renames the old one aside first instead).
//!
//! Known limitation: on macOS this replaces `Kubyl.app/Contents/MacOS/kubyl` in place but
//! doesn't refresh `Info.plist`, the icon or the code signature inside the bundle from the new
//! `.dmg` — only the binary changes. Good enough for "restart to pick up the new version";
//! revisit if a release ever needs an Info.plist change to take effect without a fresh install.

#[cfg(any(target_os = "windows", all(test, target_os = "linux")))]
use std::io::Cursor;
use std::path::Path;

#[cfg(all(test, target_os = "linux"))]
use tempfile::TempDir;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("couldn't find the {0} in the downloaded archive")]
    NotFound(&'static str),
    #[error("couldn't read the downloaded archive: {0}")]
    Archive(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("couldn't determine the running executable's path: {0}")]
    CurrentExe(std::io::Error),
    #[error("couldn't replace the running executable: {0}")]
    Replace(std::io::Error),
}

/// Extracts the new `kubyl` binary from `archive_bytes` (the artifact this platform's manifest
/// entry pointed at — a `.tar.gz` on Linux, a `.zip` on Windows, a `.dmg` on macOS) and replaces
/// the currently running executable with it. Returns once the replacement is on disk; the
/// caller still needs to relaunch for it to take effect (`SelfUpdate::restart`).
pub fn apply(archive_bytes: &[u8]) -> Result<(), ApplyError> {
    let current_exe = std::env::current_exe().map_err(ApplyError::CurrentExe)?;
    let staging = tempfile::tempdir()?;
    let new_binary = extract_binary(archive_bytes, staging.path())?;
    self_replace::self_replace(&new_binary).map_err(ApplyError::Replace)?;
    let _ = current_exe; // kept for clarity of intent; self_replace targets argv[0]'s exe itself.
    Ok(())
}

#[cfg(target_os = "linux")]
fn extract_binary(archive_bytes: &[u8], staging: &Path) -> Result<std::path::PathBuf, ApplyError> {
    let decoder = flate2::read::GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path.file_name().and_then(|n| n.to_str()) == Some("kubyl") {
            let dest = staging.join("kubyl");
            entry.unpack(&dest)?;
            return Ok(dest);
        }
    }
    Err(ApplyError::NotFound("kubyl binary"))
}

#[cfg(target_os = "windows")]
fn extract_binary(archive_bytes: &[u8], staging: &Path) -> Result<std::path::PathBuf, ApplyError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive_bytes))
        .map_err(|err| ApplyError::Archive(err.to_string()))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|err| ApplyError::Archive(err.to_string()))?;
        if file.name() == "kubyl.exe" {
            let dest = staging.join("kubyl.exe");
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut file, &mut out)?;
            return Ok(dest);
        }
    }
    Err(ApplyError::NotFound("kubyl.exe binary"))
}

/// The `.dmg` is a disk image, not an archive `tar`/`zip` can read: mount it with `hdiutil`
/// (background thread, off the UI thread — same rule as any other blocking call), grab the new
/// binary out of the mounted `.app`, then unmount.
#[cfg(target_os = "macos")]
fn extract_binary(archive_bytes: &[u8], staging: &Path) -> Result<std::path::PathBuf, ApplyError> {
    use std::process::Command;

    let dmg_path = staging.join("kubyl.dmg");
    std::fs::write(&dmg_path, archive_bytes)?;
    let mount_point = staging.join("mnt");
    std::fs::create_dir_all(&mount_point)?;

    let attach = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount_point)
        .arg(&dmg_path)
        .output()?;
    if !attach.status.success() {
        return Err(ApplyError::Archive(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&attach.stderr)
        )));
    }

    let detach = |mount_point: &Path| {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet"])
            .arg(mount_point)
            .output();
    };

    let source = mount_point.join("Kubyl.app/Contents/MacOS/kubyl");
    if !source.exists() {
        detach(&mount_point);
        return Err(ApplyError::NotFound("Kubyl.app/Contents/MacOS/kubyl"));
    }
    let dest = staging.join("kubyl");
    let copied = std::fs::copy(&source, &dest);
    detach(&mount_point);
    copied?;
    Ok(dest)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn make_tarball(binary_name: &str, contents: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let encoder = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, binary_name, Cursor::new(contents))
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        buf
    }

    #[test]
    fn extracts_the_named_binary_from_a_tarball() {
        let tarball = make_tarball("kubyl-0.3.0-linux-x86_64/kubyl", b"fake binary contents");
        let staging = TempDir::new().unwrap();
        let extracted = extract_binary(&tarball, staging.path()).unwrap();
        assert_eq!(std::fs::read(extracted).unwrap(), b"fake binary contents");
    }

    #[test]
    fn errors_when_no_kubyl_binary_is_present() {
        let tarball = make_tarball("kubyl-0.3.0-linux-x86_64/LICENSE-MIT", b"license text");
        let staging = TempDir::new().unwrap();
        assert!(matches!(
            extract_binary(&tarball, staging.path()),
            Err(ApplyError::NotFound(_))
        ));
    }
}
