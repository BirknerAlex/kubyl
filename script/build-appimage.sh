#!/usr/bin/env bash
# Builds kubyl-$VERSION-linux-<amd64|arm64>.AppImage from an already-built release binary.
#
# Uses linuxdeploy + its GTK plugin — the same tool tauri-bundler uses in production for wry
# (kubyl_webview's underlying webview crate) apps on Linux. webkit2gtk's subprocess helpers
# (WebKitWebProcess, WebKitNetworkProcess) aren't found by linuxdeploy's or the GTK plugin's
# dependency scan, since nothing directly links them — they get copied in by hand below, the
# same fix tauri-bundler applies for the same underlying webview library.
#
# Pinned to the linuxdeploy commit and AppRun tauri-bundler validates in production (not
# linuxdeploy's own "continuous" build, which has shipped broken AppImage output before).
#
# Usage: build-appimage.sh <version> <x86_64|aarch64> <path to the built kubyl binary>
set -euo pipefail

VERSION="${1:?version required}"
ARCH="${2:?arch required (x86_64 or aarch64)}"
BIN_PATH="${3:?path to the built kubyl binary required}"

case "$ARCH" in
  x86_64) NFPM_ARCH=amd64 ;;
  aarch64) NFPM_ARCH=arm64 ;;
  *)
    echo "::error::unknown arch '$ARCH' (expected x86_64 or aarch64)"
    exit 1
    ;;
esac

WORK="$(mktemp -d)"
APPDIR="$WORK/AppDir"
TOOLS="$WORK/tools"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$TOOLS"

# --- Stage the AppDir ---
install -Dm755 "$BIN_PATH" "$APPDIR/usr/bin/kubyl"
install -Dm644 packaging/linux/kubyl.desktop "$APPDIR/usr/share/applications/kubyl.desktop"
for size in 16 32 48 64 128 256 512 1024; do
  src="assets/logo/png/app-icon-${size}.png"
  [ -f "$src" ] || continue
  install -Dm644 "$src" "$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps/kubyl.png"
done

for dir in "/usr/lib/$ARCH-linux-gnu" /usr/lib64 /usr/lib /usr/libexec; do
  for file in WebKitNetworkProcess WebKitWebProcess injected-bundle/libwebkit2gtkinjectedbundle.so; do
    source="$dir/webkit2gtk-4.1/$file"
    if [ -f "$source" ]; then
      install -Dm755 "$source" "$APPDIR/${source#/}"
    fi
  done
done

# --- Fetch the build tools ---
LINUXDEPLOY="$TOOLS/linuxdeploy-$ARCH.AppImage"
curl -sSL --fail -o "$LINUXDEPLOY" \
  "https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-07333c6/linuxdeploy-$ARCH.AppImage"
chmod +x "$LINUXDEPLOY"
# Zeroes the AppImage type-2 magic bytes in this *tool*, not kubyl's output — otherwise a
# desktop's AppImage integration daemon tries to "integrate" linuxdeploy itself while it's only
# used transiently here to build kubyl's AppImage (same patch tauri-bundler applies).
dd if=/dev/zero bs=1 count=3 seek=8 conv=notrunc of="$LINUXDEPLOY" 2>/dev/null

APPRUN="$TOOLS/AppRun-$ARCH"
curl -sSL --fail -o "$APPRUN" \
  "https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-$ARCH"
chmod +x "$APPRUN"
cp "$APPRUN" "$APPDIR/AppRun"

GTK_PLUGIN="$TOOLS/linuxdeploy-plugin-gtk.sh"
curl -sSL --fail -o "$GTK_PLUGIN" \
  "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
chmod +x "$GTK_PLUGIN"

# --- Top-level AppDir icon + desktop symlinks linuxdeploy's AppImage output plugin expects ---
cp "$APPDIR/usr/share/icons/hicolor/512x512/apps/kubyl.png" "$APPDIR/kubyl.png"
ln -sf kubyl.png "$APPDIR/.DirIcon"
ln -sf usr/share/applications/kubyl.desktop "$APPDIR/kubyl.desktop"

FINAL_PATH="$(pwd)/kubyl-$VERSION-linux-$NFPM_ARCH.AppImage"
OUTPUT="$FINAL_PATH" ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 PATH="$TOOLS:$PATH" \
  "$LINUXDEPLOY" --appimage-extract-and-run --appdir "$APPDIR" --plugin gtk --output appimage

if [ ! -f "$FINAL_PATH" ]; then
  echo "::error::linuxdeploy didn't produce $FINAL_PATH"
  exit 1
fi
echo "Built $(basename "$FINAL_PATH")"
