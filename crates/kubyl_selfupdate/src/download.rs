//! Fetches the update manifest and, once a newer version is confirmed, the platform artifact.

use openidconnect::reqwest;
use sha2::{Digest, Sha256};

use crate::manifest::{Channel, PlatformArtifact, UpdateManifest};
use crate::verify::{self, VerifyError};

/// Where CI publishes release assets. `gh release create` (`.github/workflows/release.yml`)
/// uploads the manifest and its `.minisig` next to the platform artifacts, so `/latest/download`
/// always resolves to the newest published channel file without Kubyl needing to know a tag.
const RELEASES_BASE: &str = "https://github.com/BirknerAlex/kubyl/releases/latest/download";

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("couldn't reach the update server: {0}")]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("the downloaded artifact's checksum doesn't match the signed manifest")]
    ChecksumMismatch,
    #[error("this build's platform isn't in the update manifest")]
    NoArtifactForPlatform,
    #[error("the signed manifest is for the {found:?} channel, not {expected:?}")]
    WrongChannel { expected: Channel, found: Channel },
}

fn client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .user_agent(concat!("kubyl/", env!("CARGO_PKG_VERSION")))
        // Without a timeout, a stalled request leaves the service stuck in Checking or
        // Downloading forever — start_poll and check() both skip re-checking in those states.
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(15 * 60))
        .build()
        .expect("the reqwest client builder never fails with these options")
}

/// The manifest channels to try for `requested`, in order. CI publishes only the stable
/// manifest today, so Preview falls back to it while no `updates-preview.json` exists.
fn manifest_candidates(requested: Channel) -> &'static [Channel] {
    match requested {
        Channel::Stable => &[Channel::Stable],
        Channel::Preview => &[Channel::Preview, Channel::Stable],
    }
}

/// A verified manifest must be the one published for the channel it was fetched from: a stale
/// or misplaced (but validly signed) manifest of another channel is refused.
fn check_channel(
    manifest: UpdateManifest,
    expected: Channel,
) -> Result<UpdateManifest, FetchError> {
    if manifest.channel != expected {
        return Err(FetchError::WrongChannel {
            expected,
            found: manifest.channel,
        });
    }
    Ok(manifest)
}

/// Downloads and verifies the manifest for `channel` (see [`manifest_candidates`]). `Ok` only
/// ever wraps a manifest whose signature checked out against Kubyl's embedded release key.
pub async fn fetch_manifest(channel: Channel) -> Result<UpdateManifest, FetchError> {
    let http = client();
    let candidates = manifest_candidates(channel);
    for (ix, candidate) in candidates.iter().enumerate() {
        match fetch_signed(&http, *candidate).await {
            Err(FetchError::Network(err))
                if err.status() == Some(reqwest::StatusCode::NOT_FOUND)
                    && ix + 1 < candidates.len() => {}
            result => return result,
        }
    }
    unreachable!("manifest_candidates is never empty and the last candidate returns")
}

async fn fetch_signed(
    http: &reqwest::Client,
    channel: Channel,
) -> Result<UpdateManifest, FetchError> {
    let manifest_bytes = http
        .get(format!("{RELEASES_BASE}/{}", channel.manifest_file()))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let signature_text = http
        .get(format!("{RELEASES_BASE}/{}", channel.signature_file()))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    check_channel(verify::verify(&manifest_bytes, &signature_text)?, channel)
}

/// Downloads this build's platform artifact from `manifest` and checks it against the sha256
/// the (already-verified) manifest recorded for it.
pub async fn fetch_artifact(manifest: &UpdateManifest) -> Result<Vec<u8>, FetchError> {
    let PlatformArtifact { url, sha256 } = manifest
        .artifact()
        .ok_or(FetchError::NoArtifactForPlatform)?;
    let bytes = client()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();
    let digest = Sha256::digest(&bytes);
    if hex::encode(digest) != sha256.to_lowercase() {
        return Err(FetchError::ChecksumMismatch);
    }
    Ok(bytes)
}

/// Minimal hex encoding — `sha2`'s digest is fixed-size bytes, and pulling in a whole crate
/// just for `bytes -> lowercase hex` isn't worth it.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(channel: Channel) -> UpdateManifest {
        UpdateManifest {
            version: "1.0.0".parse().unwrap(),
            channel,
            notes_url: String::new(),
            platforms: Default::default(),
        }
    }

    #[test]
    fn a_manifest_of_another_channel_is_refused() {
        assert!(check_channel(manifest(Channel::Stable), Channel::Stable).is_ok());
        assert!(matches!(
            check_channel(manifest(Channel::Stable), Channel::Preview),
            Err(FetchError::WrongChannel {
                expected: Channel::Preview,
                found: Channel::Stable
            })
        ));
    }

    #[test]
    fn preview_falls_back_to_stable_but_not_the_other_way() {
        assert_eq!(manifest_candidates(Channel::Stable), [Channel::Stable]);
        assert_eq!(
            manifest_candidates(Channel::Preview),
            [Channel::Preview, Channel::Stable]
        );
    }

    #[test]
    fn hex_encodes_like_sha256sum() {
        let digest = Sha256::digest(b"kubyl");
        // Cross-checked against `printf kubyl | sha256sum`.
        assert_eq!(
            hex::encode(digest),
            "ac572c09e0edbce58c9db790a398b0fdcd8b63783f00c916a5b8336ea958f077"
        );
    }
}
