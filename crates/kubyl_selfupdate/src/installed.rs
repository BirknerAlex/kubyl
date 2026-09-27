//! Where Kubyl thinks it was installed from — self-update only replaces the executable when
//! nothing else already owns that job.

use std::path::Path;

/// How this running binary got onto the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// A manual download: the raw `.dmg`/`.zip`/`.tar.gz` extracted or dragged somewhere the
    /// user (not a package manager) controls. Self-update is safe here.
    Manual,
    /// Homebrew, apt/dpkg, dnf/rpm, pacman, winget or Flatpak. Self-update would fight the
    /// package manager's own bookkeeping (and, for Flatpak, can't write into `/app` anyway) —
    /// these show "update with <tool>" instead.
    PackageManager(&'static str),
}

impl InstallKind {
    pub fn can_self_update(self) -> bool {
        matches!(self, InstallKind::Manual)
    }

    /// What to tell the user instead of offering a self-update, when known.
    pub fn update_hint(self) -> Option<&'static str> {
        match self {
            InstallKind::Manual => None,
            InstallKind::PackageManager(tool) => Some(tool),
        }
    }
}

/// Detects the install kind from the running executable's real (symlink-resolved) path and,
/// on Linux, a couple of env vars package managers set. Best-effort: an unrecognized path
/// falls back to `Manual` — the safer default is to offer the update, not silently withhold it.
pub fn detect() -> InstallKind {
    if std::env::var_os("FLATPAK_ID").is_some() {
        return InstallKind::PackageManager("Flatpak (`flatpak update`)");
    }
    if std::env::var_os("SNAP").is_some() {
        return InstallKind::PackageManager("snap (`snap refresh`)");
    }
    let Ok(exe) = std::env::current_exe() else {
        return InstallKind::Manual;
    };
    let Ok(exe) = exe.canonicalize() else {
        return InstallKind::Manual;
    };
    detect_from_path(&exe)
}

fn detect_from_path(exe: &Path) -> InstallKind {
    let path = exe.to_string_lossy();

    if cfg!(target_os = "macos") && path.contains("/Caskroom/") {
        return InstallKind::PackageManager("Homebrew (`brew upgrade --cask kubyl`)");
    }
    if cfg!(target_os = "windows") && path.contains("\\WinGet\\Packages\\") {
        return InstallKind::PackageManager("winget (`winget upgrade Kubyl`)");
    }
    if cfg!(target_os = "linux") {
        // /usr and /opt are where .deb, .rpm and Arch packages put the binary (nfpm.yaml
        // installs to /usr/bin/kubyl); a manual tarball extract lands anywhere else, most
        // often under the user's home directory.
        if path.starts_with("/usr/") || path.starts_with("/opt/") {
            return InstallKind::PackageManager(
                "your Linux package manager (apt/dnf/pacman upgrade)",
            );
        }
    }
    InstallKind::Manual
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_extract_can_self_update() {
        let path = Path::new("/home/alex/apps/kubyl-linux-x86_64/kubyl");
        assert_eq!(detect_from_path(path), InstallKind::Manual);
        assert!(detect_from_path(path).can_self_update());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_homebrew_cask_install_skips_self_update() {
        let path = Path::new("/opt/homebrew/Caskroom/kubyl/1.0.0/Kubyl.app/Contents/MacOS/kubyl");
        let kind = detect_from_path(path);
        assert!(!kind.can_self_update());
        assert!(kind.update_hint().unwrap().contains("brew"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_deb_or_rpm_install_skips_self_update() {
        let path = Path::new("/usr/bin/kubyl");
        assert!(!detect_from_path(path).can_self_update());
    }
}
