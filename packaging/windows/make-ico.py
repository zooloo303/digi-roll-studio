#!/usr/bin/env python3
"""Assemble icons/windows/icon.ico from the committed PNG icon set.

macOS-only, and a *development* tool: the .ico it writes is committed, so
neither CI nor a Windows build box needs an image library to make an installer.
Re-run it only when the source art changes.

There is no ImageMagick here and no Pillow in the system Python, but macOS ships
an ICO *writer* in ImageIO, which `sips` exposes. So the image encoding is done
by sips, one size at a time, and this script only re-packs those single-entry
files into one multi-size icon -- pure byte shuffling, no pixel handling.

Sizes 16-128 are stored as DIBs, which is what sips emits and what every
version of Windows and every resource compiler accepts. The 256 is stored as
the PNG itself: that is the conventional encoding for the largest entry, is
supported everywhere since Vista, and keeps ~250 KB of never-drawn DIB out of
the file.

The 16 and 32 sources are the purpose-drawn ones from icons/mac -- see
icons/README.md, they use a simplified cut with one pip fewer and thicker bars
so the mark still reads at menu-bar size. Downscaling the 256 into those slots
would throw that work away.
"""

import os
import struct
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# (size, source png, resize?) -- resize only where there is no drawn master.
SOURCES = [
    (16,  "icons/mac/icon_16x16.png",   False),
    (32,  "icons/mac/icon_32x32.png",   False),
    (48,  "icons/mac/icon_256x256.png", True),
    (64,  "icons/mac/icon_64x64.png",   False),
    (128, "icons/mac/icon_128x128.png", False),
]
PNG_256 = "icons/windows/icon-256.png"
OUT = "icons/windows/icon.ico"


def sips(*args):
    subprocess.run(["sips", *args], check=True, stdout=subprocess.DEVNULL,
                   stderr=subprocess.PIPE)


def dib_entry(size, source, resize, tmp):
    """Encode one size with sips and return its raw DIB payload."""
    src = os.path.join(ROOT, source)
    staged = os.path.join(tmp, "in-%d.png" % size)
    subprocess.run(["cp", src, staged], check=True)
    if resize:
        sips("-z", str(size), str(size), staged)

    ico = os.path.join(tmp, "one-%d.ico" % size)
    sips("-s", "format", "ico", staged, "--out", ico)

    with open(ico, "rb") as fh:
        blob = fh.read()

    # Unpack the single-entry file sips just wrote and hand back the payload.
    reserved, kind, count = struct.unpack_from("<HHH", blob, 0)
    if (reserved, kind, count) != (0, 1, 1):
        sys.exit("sips wrote an unexpected ICO for %d: %r" % (size, (reserved, kind, count)))
    w, h, _colors, _res, planes, bpp, length, offset = struct.unpack_from("<BBBBHHII", blob, 6)
    if (w or 256, h or 256) != (size, size):
        sys.exit("sips wrote %dx%d into the %d slot" % (w, h, size))
    return blob[offset:offset + length], planes, bpp


def main():
    if sys.platform != "darwin":
        sys.exit("make-ico.py needs macOS's sips; the committed icon.ico is the artefact")

    entries = []
    with tempfile.TemporaryDirectory() as tmp:
        for size, source, resize in SOURCES:
            payload, planes, bpp = dib_entry(size, source, resize, tmp)
            entries.append((size, payload, planes, bpp))

    with open(os.path.join(ROOT, PNG_256), "rb") as fh:
        entries.append((256, fh.read(), 1, 32))

    # 6-byte header, then one 16-byte directory entry per image, then payloads.
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = len(header) + 16 * len(entries)
    directory, payloads = b"", b""
    for size, payload, planes, bpp in entries:
        directory += struct.pack(
            "<BBBBHHII",
            0 if size == 256 else size,   # 256 does not fit a byte; 0 means 256
            0 if size == 256 else size,
            0,                            # not a palette icon
            0,
            planes,
            bpp,
            len(payload),
            offset,
        )
        payloads += payload
        offset += len(payload)

    out = os.path.join(ROOT, OUT)
    with open(out, "wb") as fh:
        fh.write(header + directory + payloads)

    print("wrote %s (%d entries, %d bytes)" % (OUT, len(entries), os.path.getsize(out)))
    for size, payload, _p, bpp in entries:
        kind = "PNG" if payload[:8] == b"\x89PNG\r\n\x1a\n" else "DIB"
        print("  %4d  %-3s  %3d bpp  %7d bytes" % (size, kind, bpp, len(payload)))


if __name__ == "__main__":
    main()
