#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
name="微信小程序缓存识别器"
app="$root/dist/$name.app"
case "$(uname -m)" in
  x86_64) arch="x86_64" ;;
  arm64) arch="arm64" ;;
  *) echo "unsupported macOS architecture: $(uname -m)" >&2; exit 1 ;;
esac
dmg="$root/dist/wxapplet-inspector-macos-$arch.dmg"

find "$app" -depth -delete 2>/dev/null || true
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m755 "$root/target/release/wxapplet-inspector" \
  "$app/Contents/MacOS/wxapplet-inspector"
install -m644 "$root/assets/icon.icns" "$app/Contents/Resources/icon.icns"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDisplayName</key><string>微信小程序缓存识别器</string>
  <key>CFBundleExecutable</key><string>wxapplet-inspector</string>
  <key>CFBundleIdentifier</key><string>local.wxapplet.inspector</string>
  <key>CFBundleName</key><string>微信小程序缓存识别器</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
hdiutil create -volname "$name" -srcfolder "$app" -ov -format UDZO "$dmg"
