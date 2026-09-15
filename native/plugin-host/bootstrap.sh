#!/bin/sh
# Worktree-local tools only. No plugin installation, ROM copying, or Rust build.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"
cache="$root/local/plugin-host"
mkdir -p "$cache"
if [ ! -x "$cache/tools/bin/cmake" ]; then
  python3 -m venv "$cache/tools"
  "$cache/tools/bin/pip" install cmake==3.31.6
fi
"$cache/tools/bin/cmake" -S native/plugin-host -B "$cache/build" -DCMAKE_BUILD_TYPE=Release
"$cache/tools/bin/cmake" --build "$cache/build" -j 6
"$cache/tools/bin/ctest" --test-dir "$cache/build" --output-on-failure
