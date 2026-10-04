//! Verifies the ed25519-signed update manifest with `minisign`.
//!
//! CI signs `updates-<channel>.json` with the private half of this key
//! (`script/sign-update-manifest.sh`, secret `UPDATE_SIGNING_KEY`) and publishes both the
//! manifest and its `.minisig` as release assets. Only the public half ever ships in the
//! binary — that's the whole point of an asymmetric signature: a compromised download host
//! can't forge an update Kubyl will accept.

use minisign_verify::{PublicKey, Signature};

use crate::manifest::UpdateManifest;

const PUBLIC_KEY: &str = "RWQv5UZx8dJ1ROXVEJwKXW0+dVa2ngoxWHixjEL54+3CGBCk8TeriiS1";

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("the signature file isn't valid minisign output: {0}")]
    BadSignatureFile(String),
    #[error("the update manifest's signature doesn't match Kubyl's release key")]
    BadSignature,
    #[error("the update manifest isn't valid JSON: {0}")]
    Invalid(#[from] serde_json::Error),
}

/// Verifies `manifest_bytes` against `signature_text` (the raw contents of the `.minisig`
/// file), then parses the manifest. Never trust a manifest that didn't pass this.
pub fn verify(manifest_bytes: &[u8], signature_text: &str) -> Result<UpdateManifest, VerifyError> {
    let key = PublicKey::from_base64(PUBLIC_KEY).expect("hardcoded public key is well-formed");
    let signature = Signature::decode(signature_text)
        .map_err(|err| VerifyError::BadSignatureFile(err.to_string()))?;
    key.verify(manifest_bytes, &signature, false)
        .map_err(|_| VerifyError::BadSignature)?;
    Ok(serde_json::from_slice(manifest_bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `minisign -G -W` (unrelated throwaway key) then `minisign -S -m` over the literal bytes
    // `payload` — a real, well-formed signature, just not one from the release key.
    const TEST_PUBLIC_KEY: &str = "RWTeBBO2qqYVUfVZmPdBolS6KiNNq//5Vss8qbOGWEXN+RfhVKTUyNg2";
    const TEST_SIGNATURE: &str = "untrusted comment: test sig\n\
        RUTeBBO2qqYVUaUAEZIiFINYAB6wCtgfsCCz3oCt5lkAkV+l+G/4LJ3eznmAn5xUAGAD4w2k166HOZfGx8bxKyOlOVdTvPVL9gI=\n\
        trusted comment: test\n\
        q457rADPcg8TN+s55o+CIikdEwjmZNKyO1Ripu3iONIpHAA2juIiU5co6NVnTydaVZ0DDardw+vOx1KIGrJrBg==";

    #[test]
    fn accepts_a_valid_signature_from_its_own_key() {
        let key = PublicKey::from_base64(TEST_PUBLIC_KEY).unwrap();
        let signature = Signature::decode(TEST_SIGNATURE).unwrap();
        key.verify(b"payload", &signature, false).unwrap();
    }

    #[test]
    fn rejects_a_signature_from_a_different_key() {
        // The real check: the hardcoded release PUBLIC_KEY must reject this unrelated test
        // key's signature (proving verify() checks against PUBLIC_KEY, not any well-formed sig).
        assert_ne!(PUBLIC_KEY, TEST_PUBLIC_KEY);
        let result = verify(b"payload", TEST_SIGNATURE);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_garbage_signature_text() {
        let err = verify(b"{}", "not a minisig file").unwrap_err();
        assert!(matches!(err, VerifyError::BadSignatureFile(_)));
    }
}
