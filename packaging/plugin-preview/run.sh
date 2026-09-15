#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="$ROOT/target/plugin-preview"
exec cargo run -p digi_roll_studio --features plugin-host -- "$@"
