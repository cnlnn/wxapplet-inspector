#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_icon="$root/assets/icon.svg"
png_dir="$root/assets/icons"
work="$(mktemp -d "${TMPDIR:-/tmp}/wxapplet-icons.XXXXXX")"
cleanup() { find "$work" -depth -delete 2>/dev/null || true; }
trap cleanup EXIT

for size in 16 32 48 64 128 256 512 1024; do
  rsvg-convert --width "$size" --height "$size" "$source_icon" \
    --output "$work/icon_${size}.png"
done

mkdir -p "$png_dir"
for size in 16 32 48 64 128 256 512; do
  install -m644 "$work/icon_${size}.png" "$png_dir/icon_${size}.png"
done
install -m644 "$work/icon_512.png" "$root/assets/icon.png"
python -c 'from PIL import Image; import sys; image=Image.open(sys.argv[1]); image.save(sys.argv[2], sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])' \
  "$work/icon_256.png" "$root/assets/icon.ico"
python -c 'from PIL import Image; import sys; image=Image.open(sys.argv[1]); image.save(sys.argv[2], sizes=[(16,16),(32,32),(64,64),(128,128),(256,256),(512,512)])' \
  "$work/icon_512.png" "$root/assets/icon.icns"

file "$root/assets/icon.png" "$root/assets/icon.ico" "$root/assets/icon.icns"
