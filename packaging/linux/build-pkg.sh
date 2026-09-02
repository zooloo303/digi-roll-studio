#!/usr/bin/env bash
#
# Build the Arch package (.pkg.tar.zst) for the machine you are on.
#
# The PKGBUILD's source is a tarball of this repo, made here with the release
# artefacts and VCS state filtered out so the compiler sees exactly what CI
# would see.
#
#   packaging/linux/build-pkg.sh
#
# The package lands in packaging/linux/ and can be installed with
# `sudo pacman -U` — see the PKGBUILD header.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/p' Cargo.toml \
           | sed -n 's/^version *= *"\([^"]*\)".*/\1/p' | head -1)"
[ -n "$VERSION" ] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

TARBALL="packaging/linux/digi-roll-studio-$VERSION.tar.gz"

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

# The source tarball the PKGBUILD extracts. Excludes the build directory, VCS
# state and past packages; the extracted root is digi-roll-studio-<ver>/ to
# match what `git archive --prefix` would produce on CI.
say "source tarball: $TARBALL"
rm -f "$TARBALL"
git ls-files -z --cached --others --exclude-standard \
  | tar --null -czf "$TARBALL" --transform "s,^,digi-roll-studio-$VERSION/," -T -

# `pkgver` is stamped from Cargo.toml rather than kept in step by hand. The
# version is in the package's *filename* and in what pacman records, so a
# forgotten bump here ships 0.3.2 to someone who downloaded 0.3.3 — and it is
# the sort of thing that is only noticed later, from a bug report against the
# wrong version. Same argument as the release workflow's tag check.
sed -i "s/^pkgver=.*/pkgver=$VERSION/" packaging/linux/PKGBUILD

# The checksum in the PKGBUILD is replaced with the real one so a rebuild or a
# `makepkg --verifysource` outside this script stays honest. Replaced
# wholesale (not just the SKIP placeholder): the first run stamps a real SHA,
# and the next run regenerates a different tarball, so only a per-run stamp
# that matches whatever is currently there has a chance.
SHA="$(sha256sum "$TARBALL" | cut -d' ' -f1)"
sed -i "s/^sha256sums=.*/sha256sums=('$SHA')/" packaging/linux/PKGBUILD

say "makepkg (this recompiles from the tarball, ~minutes)"
cd packaging/linux
# `--nodeps` because the toolchain here is rustup's, not pacman's `rust`
# package: makepkg's buildtime check only understands pacman packages, and the
# rustup cargo satisfies the same requirement. The file still declares its
# runtime `depends` (`alsa-lib`), so `pacman -U` verifies those on install.
makepkg --nodeps --force --clean

say "done"
ls -lh *.pkg.tar.zst