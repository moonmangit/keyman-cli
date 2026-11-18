#!/usr/bin/env bash
# Builds Keyman.app (release, with its icon) into target/release/bundle/.
#   scripts/bundle-macos.sh            build for this Mac
#   scripts/bundle-macos.sh --universal  Apple silicon + Intel in one app
#   scripts/bundle-macos.sh --target x86_64-apple-darwin  one target (the release builds each in its own job)
# Signs ad hoc; set KEYMAN_SIGN_IDENTITY to sign with a Developer ID instead.
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
out=target/release/bundle/Keyman.app

if [[ "${1:-}" == "--universal" ]]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  bin=$(mktemp -d)/keyman-app
  lipo -create -output "$bin" \
    target/aarch64-apple-darwin/release/keyman-app target/x86_64-apple-darwin/release/keyman-app
elif [[ "${1:-}" == "--target" ]]; then
  rustup target add "$2" >/dev/null
  cargo build --locked --release --target "$2"
  bin=target/$2/release/keyman-app
else
  cargo build --release
  bin=target/release/keyman-app
fi

rm -rf "$out"
mkdir -p "$out/Contents/MacOS" "$out/Contents/Resources"
cp "$bin" "$out/Contents/MacOS/Keyman"
cp assets/keyman.icns "$out/Contents/Resources/keyman.icns"
cat > "$out/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Keyman</string>
  <key>CFBundleDisplayName</key><string>Keyman</string>
  <key>CFBundleIdentifier</key><string>dev.keyman.app</string>
  <key>CFBundleExecutable</key><string>Keyman</string>
  <key>CFBundleIconFile</key><string>keyman</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
codesign --force --deep --sign "${KEYMAN_SIGN_IDENTITY:--}" "$out" >/dev/null
echo "Built $out"
