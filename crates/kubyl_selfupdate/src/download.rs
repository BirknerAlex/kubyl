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
}

fn client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .user_agent(concat!("kubyl/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("the reqwest client builder never fails with these options")
}

/// Downloads and verifies `channel`'s manifest. `Ok` only ever wraps a manifest whose
/// signature checked out against Kubyl's embedded release key.
pub async fn fetch_manifest(channel: Channel) -> Result<UpdateManifest, FetchError> {
    let http = client();
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
    Ok(verify::verify(&manifest_bytes, &signature_text)?)
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
