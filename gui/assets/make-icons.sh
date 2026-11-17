#!/usr/bin/env bash
# Regenerates every app icon from icon.svg. Needs macOS (swift, sips, iconutil) and ImageMagick.
#   keyman.icns          macOS bundle icon (Contents/Resources)
#   keyman.ico           Windows executable icon (embedded by build.rs)
#   icon-macos.png       runtime Dock icon on macOS (rune::set_app_icon)
#   icon-windows.png     runtime window/taskbar icon on Windows (rune::set_app_icon)
set -euo pipefail
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

swift render-svg.swift icon.svg "$work/master.png" 1024

# macOS: Apple's grid already leaves the margin, so every size is the whole canvas.
set_dir="$work/keyman.iconset"
mkdir "$set_dir"
for size in 16 32 128 256 512; do
  sips -z $size $size "$work/master.png" --out "$set_dir/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$work/master.png" --out "$set_dir/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$set_dir" -o keyman.icns
sips -z 512 512 "$work/master.png" --out icon-macos.png >/dev/null

# Windows: icons fill more of their square, so crop most of the margin away first.
magick "$work/master.png" -crop 888x888+68+68 +repage "$work/windows.png"
magick "$work/windows.png" -define icon:auto-resize=256,128,64,48,32,24,16 keyman.ico
magick "$work/windows.png" -resize 256x256 icon-windows.png

echo "wrote keyman.icns keyman.ico icon-macos.png icon-windows.png"
