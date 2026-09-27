//! Proves `verify()` accepts output actually produced by `script/sign-update-manifest.sh`
//! against the real embedded release key — not just the throwaway key used in
//! `src/verify.rs`'s unit tests. The fixtures are a fixed, never-shipped `v0.3.0` manifest;
//! this test only checks the signature format and key match, not that v0.3.0 is a real release.

use kubyl_selfupdate::manifest::Channel;
use kubyl_selfupdate::verify::verify;

#[test]
fn verifies_a_manifest_signed_by_the_real_release_key() {
    let manifest_bytes = include_bytes!("fixtures/updates-stable.json");
    let signature_text = include_str!("fixtures/updates-stable.json.minisig");

    let manifest = verify(manifest_bytes, signature_text).expect("signature must verify");

    assert_eq!(manifest.version.to_string(), "0.3.0");
    assert_eq!(manifest.channel, Channel::Stable);
    assert!(manifest.platforms.contains_key("macos-universal"));
    assert!(manifest.platforms.contains_key("windows-x86_64"));
    assert!(manifest.platforms.contains_key("windows-aarch64"));
    assert!(manifest.platforms.contains_key("linux-x86_64"));
    assert!(manifest.platforms.contains_key("linux-aarch64"));
}

#[test]
fn a_flipped_byte_in_the_manifest_fails_verification() {
    let mut manifest_bytes = include_bytes!("fixtures/updates-stable.json").to_vec();
    // Flip one byte in the version string: the signature was computed over the original
    // bytes, so it must no longer match.
    let pos = manifest_bytes
        .iter()
        .position(|&b| b == b'0')
        .expect("a digit exists in the fixture");
    manifest_bytes[pos] = b'9';
    let signature_text = include_str!("fixtures/updates-stable.json.minisig");

    assert!(verify(&manifest_bytes, signature_text).is_err());
}
