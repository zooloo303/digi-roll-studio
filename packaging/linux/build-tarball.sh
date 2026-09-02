#!/usr/bin/env bash
#
# Build the portable Linux download: the release binary, its desktop entry, its
# icon and an installer that puts the three where a desktop will find them.
#
#   packaging/linux/build-tarball.sh            build into dist/
#   packaging/linux/build-tarball.sh --no-build reuse the existing release binary
#
# This is the download for *every* distro. `build-pkg.sh` next to it builds the
# Arch package, which is the better install where pacman is what you have —
# it declares its runtime dependencies, and this cannot.
#
# The output name is not cosmetic, for the same reason `build-dmg.sh` says so:
# the download page picks each asset out of the GitHub release by matching its
# name. `-Linux-x86_64` also has to stay clear of the *macOS* matcher, which
# takes any `.tar.gz` whose name says mac, darwin, apple, arm64 or universal.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

CARGO_BIN="digi_roll_studio"

VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
           | sed -n 's/^version *= *"\([^"]*\)".*/\1/p' | head -1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

DIST="$ROOT/dist"
STAGE_NAME="Digi-Roll-Studio-$VERSION-Linux-x86_64"
STAGE="$DIST/$STAGE_NAME"
TARBALL="$DIST/$STAGE_NAME.tar.gz"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

if [ "$(uname -s)" != "Linux" ]; then
  echo "This builds the Linux download; run it on Linux." >&2
  exit 1
fi

# ---------------------------------------------------------------- the binary
if [ "${1:-}" != "--no-build" ]; then
  say "cargo build --release -p $CARGO_BIN"
  cargo build --release -p "$CARGO_BIN"
fi

BIN="target/release/$CARGO_BIN"
[ -x "$BIN" ] || { echo "$BIN is missing; run without --no-build" >&2; exit 1; }

# The one library the build links. Everything else eframe and wgpu want is
# dlopen'd at runtime, which is why INSTALL.md has to name them by hand — a
# missing one is a crash inside dlopen at launch, with nothing in `ldd` to
# suggest what was wanted. `ldd` is the check that this stayed true.
if command -v ldd >/dev/null; then
  say "linked libraries"
  ldd "$BIN" | sed 's/^/    /'
  if ldd "$BIN" | grep -q "not found"; then
    echo "the binary has unresolved libraries on the machine that built it" >&2
    exit 1
  fi
fi

# ---------------------------------------------------------------- the payload
say "staging $STAGE_NAME"
rm -rf "$STAGE" "$TARBALL"
mkdir -p "$STAGE"

install -m755 "$BIN" "$STAGE/$CARGO_BIN"
install -m644 packaging/linux/digi-roll-studio.desktop "$STAGE/digi-roll-studio.desktop"
install -m644 icons/windows/icon-256.png "$STAGE/digi-roll-studio.png"
install -m644 LICENSE "$STAGE/LICENSE"
install -m644 CREDITS.md "$STAGE/CREDITS.md"

# Unpacking to a folder full of files and a .desktop entry that names a binary
# on `$PATH` is not an install, so the tarball carries the four `install`
# lines rather than describing them. Per-user, no root: this app needs nothing
# outside `$HOME`, and asking for a password to copy a sequencer into place is
# not a trade anyone should take.
cat > "$STAGE/install.sh" <<'INSTALLER'
#!/usr/bin/env sh
#
# Install Digi-Roll Studio for the current user. No root, nothing outside $HOME.
# Undo it with uninstall.sh next to this file.

set -eu

BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
HERE="$(cd "$(dirname "$0")" && pwd)"

install -Dm755 "$HERE/digi_roll_studio" "$BIN_DIR/digi_roll_studio"
install -Dm644 "$HERE/digi-roll-studio.png" "$ICON_DIR/digi-roll-studio.png"

# The desktop entry ships `Exec=digi_roll_studio`, which is right for the Arch
# package's /usr/bin and wrong here: a session's launcher does not always have
# ~/.local/bin on its PATH, and a menu entry that silently does nothing is
# worse than no menu entry. Absolute path, written at install time.
install -Dm644 "$HERE/digi-roll-studio.desktop" "$APP_DIR/digi-roll-studio.desktop"
sed -i "s|^Exec=.*|Exec=$BIN_DIR/digi_roll_studio|" "$APP_DIR/digi-roll-studio.desktop"

# Both are optional: the entry works without them, it just may not appear until
# the next login.
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null \
  && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" || true

echo "Installed to $BIN_DIR/digi_roll_studio"
case ":$PATH:" in
  *":$BIN_DIR:"*) echo "Run it with: digi_roll_studio" ;;
  *) echo "$BIN_DIR is not on your PATH — run it with: $BIN_DIR/digi_roll_studio" ;;
esac
INSTALLER
chmod 755 "$STAGE/install.sh"

cat > "$STAGE/uninstall.sh" <<'UNINSTALLER'
#!/usr/bin/env sh
# Remove what install.sh put down. Sessions you saved are yours and are not
# touched — this only removes the three files the install copied.
set -eu
rm -f "$HOME/.local/bin/digi_roll_studio" \
      "$HOME/.local/share/applications/digi-roll-studio.desktop" \
      "$HOME/.local/share/icons/hicolor/256x256/apps/digi-roll-studio.png"
echo "Removed."
UNINSTALLER
chmod 755 "$STAGE/uninstall.sh"

cat > "$STAGE/INSTALL.md" <<INSTALL
# Digi-Roll Studio $VERSION — Linux x86_64

    ./install.sh        # into ~/.local, no root
    digi_roll_studio

\`./uninstall.sh\` takes it back out. Or skip both and run \`./digi_roll_studio\`
from this folder; it needs nothing installed to start.

On Arch and derivatives, prefer the \`.pkg.tar.zst\` from the same release:
pacman then owns the files and checks the libraries below on install, which a
tarball cannot do.

## What it needs on the machine

Built against glibc 2.35, so it wants a distro no older than Debian 12 /
Ubuntu 22.04 / Fedora 36.

Only ALSA is linked. Everything else is opened at runtime by the graphics and
windowing layer, so a missing one is not a link error — it is the app quitting
inside \`dlopen\` at launch. On any desktop install they are already there; the
list is for when they are not:

| Needs | Debian/Ubuntu | Fedora |
|---|---|---|
| ALSA (all MIDI goes through it) | \`libasound2\` | \`alsa-lib\` |
| Wayland | \`libwayland-client0\`, \`libwayland-egl1\` | \`wayland-libs-client\`, \`wayland-libs-egl\` |
| X11 / XWayland fallback | \`libx11-6\`, \`libxcb1\`, \`libxcursor1\`, \`libxi6\` | \`libX11\`, \`libxcb\`, \`libXcursor\`, \`libXi\` |
| Keyboard | \`libxkbcommon0\`, \`libxkbcommon-x11-0\` | \`libxkbcommon\`, \`libxkbcommon-x11\` |
| Vulkan (the renderer) | \`libvulkan1\` + your GPU's driver | \`vulkan-loader\` + your GPU's driver |
| EGL | \`libegl1\` | \`libglvnd-egl\` |
| D-Bus | \`libdbus-1-3\` | \`dbus-libs\` |

## MIDI

Nothing to set up: the boxes appear as ALSA sequencer ports over USB, and
auto-connect claims them. \`aconnect -l\` lists what the system can see, which is
the first thing to check if the app cannot see a box.

## Read this before pressing anything

This app writes to your hardware. \`README.md\` in the repository has the five
rules every write goes through and the case for using throwaway projects on
your boxes while it is in beta.

<https://github.com/zooloo303/digi-roll-studio>
INSTALL

# ---------------------------------------------------------------- the tarball
say "tar: $TARBALL"
tar -C "$DIST" -czf "$TARBALL" "$STAGE_NAME"
rm -rf "$STAGE"

say "done"
ls -lh "$TARBALL"
