#!/usr/bin/env bash
# Builds kubyl-$VERSION-linux-<amd64|arm64>.flatpak from an already-built release binary.
#
# The bundle is published to silo (an OSTree remote), not Flathub. It carries a runtime-repo
# pointer to Flathub so `flatpak install kubyl-*.flatpak` and installs from silo's remote fetch
# the GNOME runtime from there.
#
# Needs: flatpak, flatpak-builder, and the Flathub remote (added here, per-user).
#
# Usage: build-flatpak.sh <version> <x86_64|aarch64> <path to the built kubyl binary>
set -euo pipefail

VERSION="${1:?version required}"
ARCH="${2:?arch required (x86_64 or aarch64)}"
BIN_PATH="${3:?path to the built kubyl binary required}"

APP_ID=io.github.birkneralex.Kubyl
case "$ARCH" in
  x86_64) NFPM_ARCH=amd64 ;;
  aarch64) NFPM_ARCH=arm64 ;;
  *)
    echo "::error::unknown arch '$ARCH' (expected x86_64 or aarch64)"
    exit 1
    ;;
esac

SRC="packaging/linux/flatpak"
OUT="$PWD/kubyl-$VERSION-linux-$NFPM_ARCH.flatpak"
WORK="$(mktemp -d)"
STAGE="$WORK/stage"
mkdir -p "$STAGE"

# --- Stage the sources the manifest refers to ---
install -Dm755 "$BIN_PATH" "$STAGE/kubyl"
# Flatpak requires the desktop file and icons to be named after the app id.
sed "s/^Icon=.*/Icon=$APP_ID/" packaging/linux/kubyl.desktop > "$STAGE/$APP_ID.desktop"
for size in 16 32 48 64 128 256 512; do
  install -Dm644 "assets/logo/png/app-icon-${size}.png" "$STAGE/app-icon-${size}.png"
done
RELEASE_DATE="$(date -u +%F)"
awk -v v="$VERSION" -v d="$RELEASE_DATE" '
  /<!-- script\/build-flatpak.sh/ { printf "  <releases>\n    <release version=\"%s\" date=\"%s\"/>\n  </releases>\n", v, d; next }
  { print }
' "$SRC/$APP_ID.metainfo.xml" > "$STAGE/$APP_ID.metainfo.xml"
cp "$SRC/$APP_ID.yml" "$WORK/$APP_ID.yml"

# --- Build, export and bundle ---
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
(
  cd "$WORK"
  flatpak-builder --user --force-clean --disable-rofiles-fuse \
    --install-deps-from=flathub \
    --arch="$ARCH" --default-branch=stable \
    --repo=repo build "$APP_ID.yml"
  # Fail the release here, not on a user's machine, if the runtime lacks a library the binary
  # links (GTK 3 or WebKitGTK 4.1 are the ones a runtime bump can drop).
  if ! linked="$(flatpak-builder --run build "$APP_ID.yml" ldd /app/bin/kubyl 2>&1)"; then
    echo "::error::could not inspect the binary's libraries inside the runtime:"
    echo "$linked"
    exit 1
  fi
  if grep -q 'not found' <<< "$linked"; then
    echo "::error::the runtime is missing libraries kubyl links:"
    grep 'not found' <<< "$linked"
    exit 1
  fi
  flatpak build-bundle --arch="$ARCH" \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    repo "$OUT" "$APP_ID" stable
)
echo "built $OUT"
