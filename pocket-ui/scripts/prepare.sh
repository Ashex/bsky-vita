#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
"$ROOT/pocket-ui/scripts/bootstrap-pocketjs.sh"
cd "$ROOT"
cargo fetch --locked
cd "$ROOT/pocket-ui"
bun install --frozen-lockfile
bun run package
