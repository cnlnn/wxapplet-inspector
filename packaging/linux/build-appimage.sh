#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binary="$root/target/release/wxapplet-inspector"
output="$root/dist/微信小程序缓存识别器_0.1.0_amd64.AppImage"
staged="$root/dist/.wxapplet-inspector.$$.AppImage"

if [[ ! -x "$binary" ]]; then
  cargo build --manifest-path "$root/Cargo.toml" --release
fi

if ldd "$binary" | grep -Eiq 'webkit|javascriptcore|gtk|libsoup|gstreamer'; then
  echo "release binary contains a forbidden WebView/GTK dependency" >&2
  exit 1
fi

appdir="$(mktemp -d "${TMPDIR:-/tmp}/wxapplet-appdir.XXXXXX")"
cleanup() {
  find "$appdir" -depth -delete 2>/dev/null || true
  [[ ! -e "$staged" ]] || unlink "$staged"
}
trap cleanup EXIT

install -Dm755 "$binary" "$appdir/usr/bin/wxapplet-inspector"
install -Dm755 "$root/packaging/linux/AppRun" "$appdir/AppRun"
install -Dm644 "$root/packaging/linux/wxapplet-inspector.desktop" \
  "$appdir/usr/share/applications/wxapplet-inspector.desktop"
install -Dm644 "$root/assets/icon.png" "$appdir/wxapplet-inspector.png"
for size in 16 32 48 64 128 256 512; do
  install -Dm644 "$root/assets/icons/icon_${size}.png" \
    "$appdir/usr/share/icons/hicolor/${size}x${size}/apps/wxapplet-inspector.png"
done
ln -s wxapplet-inspector.png "$appdir/.DirIcon"
ln -s usr/share/applications/wxapplet-inspector.desktop \
  "$appdir/wxapplet-inspector.desktop"

mkdir -p "$root/dist" "${XDG_CACHE_HOME:-$HOME/.cache}/wxapplet-inspector"
tool="${APPIMAGETOOL:-}"
if [[ -z "$tool" ]] && command -v appimagetool >/dev/null 2>&1; then
  tool="$(command -v appimagetool)"
fi
if [[ -z "$tool" ]]; then
  tool="${XDG_CACHE_HOME:-$HOME/.cache}/wxapplet-inspector/appimagetool-x86_64.AppImage"
  if [[ ! -x "$tool" ]]; then
    partial="$tool.part"
    curl -fL --retry 3 \
      https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage \
      -o "$partial"
    chmod +x "$partial"
    mv "$partial" "$tool"
  fi
fi

ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" "$appdir" "$staged"
size="$(stat -c %s "$staged")"
limit=$((15 * 1024 * 1024))
if (( size > limit )); then
  echo "AppImage exceeds 15 MiB: $size bytes" >&2
  exit 1
fi
mv -f "$staged" "$output"
printf '%s\n' "$output"
printf 'size=%s bytes\n' "$size"
sha256sum "$output"
