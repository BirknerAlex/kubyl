//! The signed update manifest CI publishes per channel, and the platform key this build looks
//! up in it.

use std::collections::BTreeMap;

use semver::Version;
use serde::{Deserialize, Serialize};

/// An update channel. Each has its own manifest (`updates-<channel>.json`) and its own
/// `settings.json` opt-in (`kubyl_selfupdate::settings::SelfUpdateSettings::channel`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Preview,
}

impl Channel {
    /// The manifest file name CI publishes as a release asset for this channel.
    pub fn manifest_file(self) -> &'static str {
        match self {
            Channel::Stable => "updates-stable.json",
            Channel::Preview => "updates-preview.json",
        }
    }

    pub fn signature_file(self) -> String {
        format!("{}.minisig", self.manifest_file())
    }
}

/// A signed, parsed update manifest for one channel.
#[derive(Clone, Debug, Deserialize)]
pub struct UpdateManifest {
    pub version: Version,
    pub channel: Channel,
    /// Where "what's new" sends the user (a GitHub Release page).
    pub notes_url: String,
    /// Keyed by [`platform_key`] (`"macos-universal"`, `"windows-x86_64"`, …).
    pub platforms: BTreeMap<String, PlatformArtifact>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PlatformArtifact {
    pub url: String,
    pub sha256: String,
}

impl UpdateManifest {
    /// The artifact for this build's platform, if the manifest lists one.
    pub fn artifact(&self) -> Option<&PlatformArtifact> {
        self.platforms.get(platform_key()?)
    }
}

/// This build's key into [`UpdateManifest::platforms`], or `None` on a platform/arch Kubyl
/// doesn't publish release artifacts for (self-update just never finds anything to offer).
pub fn platform_key() -> Option<&'static str> {
    if cfg!(all(target_os = "macos")) {
        Some("macos-universal")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Some("windows-aarch64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_matches_this_build() {
        // Every CI runner target has a key; this just guards against the arm running the test
        // suite (macOS or Linux, x86_64 or aarch64) ever falling through to `None`.
        assert!(platform_key().is_some());
    }

    #[test]
    fn parses_a_manifest() {
        let json = r#"{
            "version": "1.2.3",
            "channel": "stable",
            "notes_url": "https://github.com/BirknerAlex/kubyl/releases/tag/v1.2.3",
            "platforms": {
                "macos-universal": {
                    "url": "https://example.com/kubyl.dmg",
                    "sha256": "deadbeef"
                }
            }
        }"#;
        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, Version::new(1, 2, 3));
        assert_eq!(manifest.channel, Channel::Stable);
        assert!(manifest.platforms.contains_key("macos-universal"));
    }
}
