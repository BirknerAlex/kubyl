#!/usr/bin/env bash
# Builds and signs the self-update manifest (kubyl_selfupdate) for one channel from the
# release's built artifacts. Run from the repo root after `dist/` (built by the `release` job's
# download-artifact + checksums step) has the platform files and SHA256SUMS.
#
# Needs `minisign` on PATH and the unencrypted secret key (repo secret UPDATE_SIGNING_KEY,
# `docs/RELEASING.md`) written to $MINISIGN_KEY_FILE.
set -euo pipefail

VERSION="${1:?Version required (e.g., 0.3.0)}"
CHANNEL="${2:-stable}"
DIST="${DIST:-dist}"
MINISIGN_KEY_FILE="${MINISIGN_KEY_FILE:?MINISIGN_KEY_FILE must point at the minisign secret key file}"

sha256_of() {
  awk -v f="$1" '$2 == f { print $1 }' "$DIST/SHA256SUMS"
}

MACOS_DMG="kubyl-$VERSION-macos-universal.dmg"
WIN_X64_ZIP="kubyl-$VERSION-windows-x86_64.zip"
WIN_ARM64_ZIP="kubyl-$VERSION-windows-aarch64.zip"
# build-linux's matrix.arch is amd64/arm64 (not x86_64/aarch64) — matches the real asset names
# release.yml produces (kubyl-$VERSION-linux-amd64.tar.gz etc).
LINUX_X64_TAR="kubyl-$VERSION-linux-amd64.tar.gz"
LINUX_ARM64_TAR="kubyl-$VERSION-linux-arm64.tar.gz"

BASE_URL="https://github.com/BirknerAlex/kubyl/releases/download/v$VERSION"
MANIFEST="updates-$CHANNEL.json"

for asset in "$MACOS_DMG" "$WIN_X64_ZIP" "$WIN_ARM64_ZIP" "$LINUX_X64_TAR" "$LINUX_ARM64_TAR"; do
  if [ -z "$(sha256_of "$asset")" ]; then
    echo "::error::no checksum for $asset in $DIST/SHA256SUMS"
    exit 1
  fi
done

cat > "$MANIFEST" <<JSON
{
  "version": "$VERSION",
  "channel": "$CHANNEL",
  "notes_url": "https://github.com/BirknerAlex/kubyl/releases/tag/v$VERSION",
  "platforms": {
    "macos-universal": { "url": "$BASE_URL/$MACOS_DMG", "sha256": "$(sha256_of "$MACOS_DMG")" },
    "windows-x86_64": { "url": "$BASE_URL/$WIN_X64_ZIP", "sha256": "$(sha256_of "$WIN_X64_ZIP")" },
    "windows-aarch64": { "url": "$BASE_URL/$WIN_ARM64_ZIP", "sha256": "$(sha256_of "$WIN_ARM64_ZIP")" },
    "linux-x86_64": { "url": "$BASE_URL/$LINUX_X64_TAR", "sha256": "$(sha256_of "$LINUX_X64_TAR")" },
    "linux-aarch64": { "url": "$BASE_URL/$LINUX_ARM64_TAR", "sha256": "$(sha256_of "$LINUX_ARM64_TAR")" }
  }
}
JSON

minisign -S -s "$MINISIGN_KEY_FILE" -m "$MANIFEST" -x "$MANIFEST.minisig" \
  -c "kubyl $VERSION ($CHANNEL)" -t "kubyl $VERSION ($CHANNEL)"

echo "Signed $MANIFEST -> $MANIFEST.minisig"
