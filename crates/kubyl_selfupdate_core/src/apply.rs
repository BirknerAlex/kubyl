//! Extracts a verified artifact and replaces the running executable with it.
//!
//! Only ever called with bytes [`crate::download::fetch_artifact`] already checked against the
//! signed manifest's sha256. `self_replace` handles the one truly platform-specific problem
//! on Linux and Windows (Windows refuses to overwrite a running .exe; it renames the old one
//! aside first instead).
//!
//! macOS is its own case: swapping just `Kubyl.app/Contents/MacOS/kubyl` would leave the
//! bundle's `_CodeSignature/CodeResources` sealing the *old* binary's hash, so the hardened
//! runtime would refuse to launch it. Instead the whole `Kubyl.app` is staged from the mounted
//! `.dmg`, verified with `codesign --verify --deep --strict` before it ever touches the
//! installed copy, then swapped in as a directory rename (the running process keeps its open
//! file handle to the old bundle's now-unlinked files until it exits, same as `self_replace`'s
//! trick for a single executable).

use std::path::{Path, PathBuf};

#[cfg(any(target_os = "windows", all(test, target_os = "linux")))]
use std::io::Cursor;

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
    #[error("the staged app bundle failed code signature verification: {0}")]
    SignatureCheck(String),
}

/// Extracts the new version from `archive_bytes` (the artifact this platform's manifest entry
/// pointed at — a `.tar.gz` on Linux, a `.zip` on Windows, a `.dmg` on macOS) and replaces the
/// installed copy with it. Returns once the replacement is on disk; the caller still needs to
/// relaunch for it to take effect (`SelfUpdate::restart`).
#[cfg(target_os = "macos")]
pub fn apply(archive_bytes: &[u8]) -> Result<(), ApplyError> {
    let current_exe = std::env::current_exe().map_err(ApplyError::CurrentExe)?;
    let bundle_path = app_bundle_root(&current_exe)?;
    let staging = tempfile::tempdir()?;
    let staged_bundle = stage_bundle_from_dmg(archive_bytes, staging.path())?;
    verify_signature(&staged_bundle)?;
    swap_bundle(&staged_bundle, &bundle_path)
}

#[cfg(not(target_os = "macos"))]
pub fn apply(archive_bytes: &[u8]) -> Result<(), ApplyError> {
    let staging = tempfile::tempdir()?;
    let new_binary = extract_binary(archive_bytes, staging.path())?;
    self_replace::self_replace(&new_binary).map_err(ApplyError::Replace)
}

#[cfg(target_os = "linux")]
fn extract_binary(archive_bytes: &[u8], staging: &Path) -> Result<PathBuf, ApplyError> {
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
fn extract_binary(archive_bytes: &[u8], staging: &Path) -> Result<PathBuf, ApplyError> {
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

/// `current_exe` is `.../Kubyl.app/Contents/MacOS/kubyl`; the bundle root is three levels up.
#[cfg(target_os = "macos")]
fn app_bundle_root(current_exe: &Path) -> Result<PathBuf, ApplyError> {
    let bundle = current_exe
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| ApplyError::Archive("couldn't locate the app bundle root".into()))?;
    if bundle.extension().and_then(|e| e.to_str()) != Some("app") {
        return Err(ApplyError::Archive(
            "the running executable isn't inside a .app bundle".into(),
        ));
    }
    Ok(bundle.to_path_buf())
}

/// The `.dmg` is a disk image, not an archive `tar`/`zip` can read: mount it with `hdiutil`
/// (this whole function runs on a background thread, off the UI thread — same rule as any
/// other blocking call), copy the whole `Kubyl.app` out with `ditto` (preserves the resource
/// forks and extended attributes a plain recursive copy could drop), then unmount.
#[cfg(target_os = "macos")]
fn stage_bundle_from_dmg(archive_bytes: &[u8], staging: &Path) -> Result<PathBuf, ApplyError> {
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

    let source = mount_point.join("Kubyl.app");
    if !source.exists() {
        detach(&mount_point);
        return Err(ApplyError::NotFound("Kubyl.app"));
    }
    let dest = staging.join("Kubyl.app");
    let copied = Command::new("ditto").arg(&source).arg(&dest).output();
    detach(&mount_point);
    let copied = copied?;
    if !copied.status.success() {
        return Err(ApplyError::Archive(format!(
            "ditto failed: {}",
            String::from_utf8_lossy(&copied.stderr)
        )));
    }
    Ok(dest)
}

/// Apple Team ID Kubyl releases are signed with (`packaging/macos/entitlements.plist`).
#[cfg(any(target_os = "macos", test))]
const TEAM_ID: &str = "7RJB3ZHA5M";
#[cfg(any(target_os = "macos", test))]
const BUNDLE_ID: &str = "io.github.birkneralex.Kubyl";

/// The code-signing requirement a downloaded bundle must satisfy: a Developer ID Application
/// certificate of our team, and our bundle identifier. `codesign --verify` alone accepts any
/// validly signed bundle, including one signed by someone else.
#[cfg(any(target_os = "macos", test))]
fn signing_requirement() -> String {
    format!(
        "anchor apple generic and identifier \"{BUNDLE_ID}\" \
         and certificate 1[field.1.2.840.113635.100.6.2.6] \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] \
         and certificate leaf[subject.OU] = \"{TEAM_ID}\""
    )
}

/// Refuses to install a bundle whose signature doesn't check out — a mid-transfer truncation
/// or a `ditto` mishap should fail loudly here, not surface as "Kubyl won't launch" after the
/// swap already happened.
#[cfg(target_os = "macos")]
fn verify_signature(bundle: &Path) -> Result<(), ApplyError> {
    let verify = std::process::Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(format!("-R={}", signing_requirement()))
        .arg(bundle)
        .output()?;
    if !verify.status.success() {
        return Err(ApplyError::SignatureCheck(
            String::from_utf8_lossy(&verify.stderr).into_owned(),
        ));
    }
    Ok(())
}

/// Moves `installed` aside, moves `staged` into its place, then removes the old one — a
/// directory rename, which Unix allows even while the old bundle's executable is the one
/// currently running this code (it stays open by inode until the process exits). Restores the
/// backup on failure so a botched swap never leaves the user without any app at all.
#[cfg(target_os = "macos")]
fn swap_bundle(staged: &Path, installed: &Path) -> Result<(), ApplyError> {
    let backup = installed.with_extension("app.bak");
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(installed, &backup)?;
    if let Err(err) = std::fs::rename(staged, installed) {
        let _ = std::fs::rename(&backup, installed);
        return Err(err.into());
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

#[cfg(test)]
mod requirement_tests {
    use super::*;

    #[test]
    fn requirement_pins_team_and_bundle_id() {
        let req = signing_requirement();
        assert!(req.contains("certificate leaf[subject.OU] = \"7RJB3ZHA5M\""));
        assert!(req.contains("identifier \"io.github.birkneralex.Kubyl\""));
        assert!(req.starts_with("anchor apple generic and"));
        // One line of tokens: no stray line breaks or continuation backslashes.
        assert!(!req.contains('\n') && !req.contains('\\'));
    }

    #[test]
    fn team_id_matches_the_entitlements() {
        let plist = include_str!("../../../packaging/macos/entitlements.plist");
        assert!(plist.contains(&format!("<string>{TEAM_ID}</string>")));
        assert!(plist.contains(&format!("{TEAM_ID}.{BUNDLE_ID}")));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn finds_the_bundle_root_three_levels_up() {
        let exe = Path::new("/Applications/Kubyl.app/Contents/MacOS/kubyl");
        assert_eq!(
            app_bundle_root(exe).unwrap(),
            Path::new("/Applications/Kubyl.app")
        );
    }

    #[test]
    fn refuses_an_executable_outside_any_app_bundle() {
        let exe = Path::new("/home/alex/apps/kubyl-linux-x86_64/kubyl");
        assert!(app_bundle_root(exe).is_err());
    }
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
