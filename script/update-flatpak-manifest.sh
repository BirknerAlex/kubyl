#!/usr/bin/env bash
# Updates the Flatpak manifest with the latest release version and sha256
set -euo pipefail

VERSION="${1:?Version required (e.g., 0.2.5)}"
MANIFEST_FILE="io.github.birkneralex.Kubyl.yml"
BINARY_NAME="kubyl-v${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
RELEASE_URL="https://github.com/BirknerAlex/kubyl/releases/download/v${VERSION}/${BINARY_NAME}"

echo "Fetching sha256 from GitHub release..."
SHA256=$(curl -sL --fail "${RELEASE_URL}.sha256" | awk '{print $1}')

if [ -z "$SHA256" ]; then
  echo "::error::failed to fetch sha256 for ${BINARY_NAME}"
  exit 1
fi

echo "Updating manifest with version $VERSION and sha256 $SHA256"

# Update version in URL
sed -i.bak "s|releases/download/v[0-9.]*/${BINARY_NAME}|releases/download/v${VERSION}/${BINARY_NAME}|" "$MANIFEST_FILE" && rm -f "$MANIFEST_FILE.bak"

# Update sha256
sed -i.bak "s/sha256: placeholder_sha_will_be_updated_by_ci/sha256: ${SHA256}/" "$MANIFEST_FILE" && rm -f "$MANIFEST_FILE.bak"

echo "Manifest updated successfully"
