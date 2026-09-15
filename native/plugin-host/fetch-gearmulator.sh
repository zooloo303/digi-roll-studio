#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"
cache=local/plugin-host
mkdir -p "$cache"
curl -fL https://github.com/joelanders/gearmulator-md-mm/releases/download/mdmm-v0.1.0-alpha.11/Gearmulator-Elektron-macOS-arm64-PGO.zip -o "$cache/gearmulator.zip"
printf '%s  %s\n' 9bfd89b84918fe3842dc0cbd97b14563977b425d3b90078dc886c7cf49d61dfc "$cache/gearmulator.zip" | shasum -a 256 -c -
# Refuse to overwrite an existing local plugin tree.
[ ! -e "$cache/gearmulator" ] || { echo 'Local Gearmulator directory already exists' >&2; exit 1; }
unzip -q "$cache/gearmulator.zip" -d "$cache/gearmulator"
