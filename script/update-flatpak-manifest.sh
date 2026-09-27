#!/usr/bin/env bash
# Regenerates io.github.birkneralex.Kubyl.yml's `sources:` block for a release. The whole block
# is rewritten from scratch (rather than patched line-by-line) so a second run against a new
# version can't leave stale URLs or checksums behind, and it stays correct even if the manifest
# was hand-edited since the last run.
#
# Run from the repo root, on Linux (uses sha256sum) — publish-flathub.yml's only caller.
set -euo pipefail

VERSION="${1:?Version required (e.g., 0.2.5)}"
MANIFEST_FILE="io.github.birkneralex.Kubyl.yml"
METAINFO_FILE="io.github.birkneralex.Kubyl.metainfo.xml"
BASE_URL="https://github.com/BirknerAlex/kubyl/releases/download/v${VERSION}"
RAW_BASE="https://raw.githubusercontent.com/BirknerAlex/kubyl/v${VERSION}"

# build-linux's matrix.arch is amd64/arm64 (release.yml), and each archive's own top-level
# directory (kubyl-$VERSION-linux-$ARCH/) is what strip-components: 1 in the manifest flattens
# — so this only ever needs the plain filename, no directory-name guessing.
AMD64_TAR="kubyl-${VERSION}-linux-amd64.tar.gz"
ARM64_TAR="kubyl-${VERSION}-linux-arm64.tar.gz"

echo "Fetching SHA256SUMS for v${VERSION}..."
SUMS="$(curl -sL --fail "${BASE_URL}/SHA256SUMS")"

sha256_of_release_asset() {
  awk -v f="$1" '$2 == f { print $1 }' <<< "$SUMS"
}

AMD64_SHA="$(sha256_of_release_asset "$AMD64_TAR")"
ARM64_SHA="$(sha256_of_release_asset "$ARM64_TAR")"
if [ -z "$AMD64_SHA" ] || [ -z "$ARM64_SHA" ]; then
  echo "::error::missing checksum for $AMD64_TAR or $ARM64_TAR in SHA256SUMS"
  exit 1
fi

DESKTOP_SHA="$(sha256sum -- packaging/linux/kubyl.desktop | awk '{print $1}')"
ICON_512_SHA="$(sha256sum -- assets/logo/png/app-icon-512.png | awk '{print $1}')"
ICON_256_SHA="$(sha256sum -- assets/logo/png/app-icon-256.png | awk '{print $1}')"
ICON_128_SHA="$(sha256sum -- assets/logo/png/app-icon-128.png | awk '{print $1}')"
ICON_64_SHA="$(sha256sum -- assets/logo/png/app-icon-64.png | awk '{print $1}')"

echo "Updating $MANIFEST_FILE for v${VERSION}"

# Replace everything from "sources:" to end of file with the freshly generated block. The
# manifest always ends with the sources list (see the file itself), so this is safe.
sed -i '/^    sources:/,$d' "$MANIFEST_FILE"

cat >> "$MANIFEST_FILE" <<YAML
    sources:
      # Pre-built binaries from GitHub releases, one per arch (release.yml's build-linux matrix
      # produces amd64 and arm64 separately; each archive's staged directory name embeds the
      # version, so strip-components flattens it instead of hardcoding that path).
      - type: archive
        only-arches: [x86_64]
        url: ${BASE_URL}/${AMD64_TAR}
        sha256: ${AMD64_SHA}
        strip-components: 1
      - type: archive
        only-arches: [aarch64]
        url: ${BASE_URL}/${ARM64_TAR}
        sha256: ${ARM64_SHA}
        strip-components: 1
      # Desktop file and icons live in the kubyl repo, not this manifest's own submission repo
      # (Flathub only ever checks out the manifest + metainfo, not the whole upstream tree), so
      # these are fetched from the tagged release commit rather than a local \`path:\`.
      - type: file
        url: ${RAW_BASE}/packaging/linux/kubyl.desktop
        sha256: ${DESKTOP_SHA}
        dest-filename: io.github.birkneralex.Kubyl.desktop
      - type: file
        url: ${RAW_BASE}/assets/logo/png/app-icon-512.png
        sha256: ${ICON_512_SHA}
        dest-filename: app-icon-512.png
      - type: file
        url: ${RAW_BASE}/assets/logo/png/app-icon-256.png
        sha256: ${ICON_256_SHA}
        dest-filename: app-icon-256.png
      - type: file
        url: ${RAW_BASE}/assets/logo/png/app-icon-128.png
        sha256: ${ICON_128_SHA}
        dest-filename: app-icon-128.png
      - type: file
        url: ${RAW_BASE}/assets/logo/png/app-icon-64.png
        sha256: ${ICON_64_SHA}
        dest-filename: app-icon-64.png
      # Metainfo is a sibling of this manifest (not part of the kubyl repo tree Flathub can't
      # see) — copy it alongside io.github.birkneralex.Kubyl.yml when submitting.
      - type: file
        path: io.github.birkneralex.Kubyl.metainfo.xml
YAML

# Records a <release> per version for Flathub's linter, without duplicating one on a re-run.
if ! grep -q "version=\"${VERSION}\"" "$METAINFO_FILE"; then
  echo "Adding release v${VERSION} to $METAINFO_FILE"
  TODAY="$(date -u +%Y-%m-%d)"
  sed -i "s#<releases>#<releases>\n    <release version=\"${VERSION}\" date=\"${TODAY}\"/>#" "$METAINFO_FILE"
fi

echo "Manifest updated successfully"
