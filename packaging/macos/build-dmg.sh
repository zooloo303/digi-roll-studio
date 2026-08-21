#!/usr/bin/env bash
#
# Build "Digi-Roll Studio.app" and wrap it in a .dmg for release.
#
# Run from anywhere; it works on the repo it lives in. Needs macOS on Apple
# Silicon and nothing that is not already on a stock system: cargo, iconutil,
# hdiutil, codesign.
#
#   packaging/macos/build-dmg.sh            build into dist/
#   packaging/macos/build-dmg.sh --no-build reuse the existing release binary
#
# The output name is not cosmetic. The download page at digi-roll/studio picks
# the macOS asset out of the GitHub release by matching /\.(dmg)$/ on the asset
# name, so an asset that is not a .dmg leaves the button pointing at the
# releases page instead of at the file.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

APP_NAME="Digi-Roll Studio"
BUNDLE_ID="io.github.zooloo303.digi-roll-studio"
# The Elektron boxes make this a music app; the category is what Launchpad and
# the Finder's "Kind" column sort on.
CATEGORY="public.app-category.music"
# Apple Silicon only, matching what the page advertises, so the floor is the
# first macOS that ran on it.
MIN_MACOS="11.0"
TARGET="aarch64-apple-darwin"
CARGO_BIN="digi_roll_studio"

VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
           | sed -n 's/^version *= *"\([^"]*\)".*/\1/p' | head -1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

DIST="$ROOT/dist"
APP="$DIST/$APP_NAME.app"
DMG="$DIST/Digi-Roll-Studio-$VERSION-macOS-AppleSilicon.dmg"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

if [ "$(uname -m)" != "arm64" ]; then
  echo "This builds the Apple Silicon download; run it on an arm64 Mac." >&2
  exit 1
fi

# ---------------------------------------------------------------- the binary
if [ "${1:-}" != "--no-build" ]; then
  say "cargo build --release --target $TARGET"
  cargo build --release --target "$TARGET" -p "$CARGO_BIN"
fi

BIN="target/$TARGET/release/$CARGO_BIN"
[ -x "$BIN" ] || { echo "$BIN is missing; run without --no-build" >&2; exit 1; }

# A universal binary would be the other choice here; this asserts we did not
# quietly produce the x86_64 one under Rosetta and call it Apple Silicon.
#
# Captured into a variable rather than piped into `grep -q`: with `pipefail` set,
# grep exiting on its first match can hand the writer a SIGPIPE and fail the
# whole pipeline on success. That bites once per script and is invisible when the
# output happens to be short enough to buffer.
ARCH_INFO="$(file "$BIN")"
case "$ARCH_INFO" in
  *arm64*) ;;
  *) echo "$BIN is not arm64: $ARCH_INFO" >&2; exit 1 ;;
esac

# ------------------------------------------------------------------ the icon
# The recipe is icons/README.md's, kept here so the shipped .icns cannot drift
# from the committed PNGs: 16 through 512, each also standing in as the @2x of
# the size below it.
say "iconutil: AppIcon.icns"
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
cp icons/mac/icon_16x16.png     "$ICONSET/icon_16x16.png"
cp icons/mac/icon_32x32.png     "$ICONSET/icon_16x16@2x.png"
cp icons/mac/icon_32x32.png     "$ICONSET/icon_32x32.png"
cp icons/mac/icon_64x64.png     "$ICONSET/icon_32x32@2x.png"
cp icons/mac/icon_128x128.png   "$ICONSET/icon_128x128.png"
cp icons/mac/icon_256x256.png   "$ICONSET/icon_128x128@2x.png"
cp icons/mac/icon_256x256.png   "$ICONSET/icon_256x256.png"
cp icons/mac/icon_512x512.png   "$ICONSET/icon_256x256@2x.png"
cp icons/mac/icon_512x512.png   "$ICONSET/icon_512x512.png"
cp icons/mac/icon_1024x1024.png "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$(dirname "$ICONSET")/AppIcon.icns"
ICNS="$(dirname "$ICONSET")/AppIcon.icns"

# ---------------------------------------------------------------- the bundle
say "assembling $APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Named for the app rather than for the cargo target, because this is the name
# Activity Monitor and the crash reporter show.
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"
chmod +x "$APP/Contents/MacOS/$APP_NAME"
cp "$ICNS" "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleInfoDictionaryVersion</key>	<string>6.0</string>
	<key>CFBundlePackageType</key>			<string>APPL</string>
	<key>CFBundleSignature</key>			<string>????</string>
	<key>CFBundleName</key>				<string>$APP_NAME</string>
	<key>CFBundleDisplayName</key>			<string>$APP_NAME</string>
	<key>CFBundleIdentifier</key>			<string>$BUNDLE_ID</string>
	<key>CFBundleExecutable</key>			<string>$APP_NAME</string>
	<key>CFBundleIconFile</key>			<string>AppIcon</string>
	<key>CFBundleShortVersionString</key>		<string>$VERSION</string>
	<key>CFBundleVersion</key>			<string>$VERSION</string>
	<key>LSMinimumSystemVersion</key>		<string>$MIN_MACOS</string>
	<key>LSApplicationCategoryType</key>		<string>$CATEGORY</string>
	<!-- Without this the window is drawn at 1x and scaled up, which on a
	     Retina display makes every line in the piano roll soft. -->
	<key>NSHighResolutionCapable</key>		<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>	<true/>
</dict>
</plist>
PLIST

# Classic-Mac vestige that the Finder still reads; four bytes, and cheap
# insurance against an "unknown kind" bundle.
printf 'APPL????' > "$APP/Contents/PkgInfo"

# ------------------------------------------------------------------- signing
# THIS MUST BE THE LAST THING DONE TO THE BUNDLE.
#
# The linker ad-hoc signs the binary cargo produces, so `target/.../$CARGO_BIN`
# arrives here already signed. Copying it into a bundle and then writing an
# Info.plist and an icon next to it invalidates that signature: the signature
# covers a code directory that includes the bundle's resources, and on arm64
# macOS refuses to launch a binary whose signature does not verify. The symptom
# is not a Gatekeeper prompt that can be clicked through — it is
# "the application is damaged and can't be opened", which the install page
# documents as an edge case and which would otherwise be what every single
# person who downloads this sees.
#
# Re-signing ad-hoc (`--sign -`) puts that back. It is not a Developer ID and it
# is not notarisation, so the page's "Open Anyway" walkthrough still applies —
# that is the expected first-run path, and it is a different thing from damaged.
say "codesign (ad-hoc, --force --deep)"
codesign --force --deep --sign - "$APP"

# Verify rather than trust: `--strict` complains about exactly the structural
# problems a hand-assembled bundle is prone to.
codesign --verify --deep --strict --verbose=2 "$APP"

# Also assert the signature really does cover the bundle: a signature with no
# sealed resources would verify here and still break on launch, which is the
# whole failure this step exists to prevent.
SIG_INFO="$(codesign --display --verbose=4 "$APP" 2>&1)"
case "$SIG_INFO" in
  *"Signature=adhoc"*) ;;
  *) echo "expected an ad-hoc signature, got:" >&2; echo "$SIG_INFO" >&2; exit 1 ;;
esac
case "$SIG_INFO" in
  *"Sealed Resources version"*) ;;
  *) echo "the signature seals no resources; the icon and plist are uncovered" >&2; exit 1 ;;
esac

# --------------------------------------------------------------------- the dmg
# A staging folder rather than the .app directly, so the mounted volume has the
# Applications alias the install page tells people to drag onto.
say "hdiutil: $(basename "$DMG")"
STAGE="$(mktemp -d)/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

rm -f "$DMG"
hdiutil create \
  -volname "$APP_NAME" \
  -srcfolder "$STAGE" \
  -fs HFS+ \
  -format UDZO \
  -quiet \
  "$DMG"

# The signature has to survive the round trip through the image, because what
# the user launches is the copy that came out of it.
say "verifying the signature inside the image"
MOUNT="$(mktemp -d)"
hdiutil attach "$DMG" -mountpoint "$MOUNT" -nobrowse -quiet
codesign --verify --deep --strict "$MOUNT/$APP_NAME.app"
hdiutil detach "$MOUNT" -quiet

say "done"
printf '  %s\n  %s\n' "$DMG" "$(du -h "$DMG" | cut -f1) compressed"
