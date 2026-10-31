#!/usr/bin/env bash
set -euo pipefail
termlab_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
if ! command -v nix >/dev/null; then
    echo 'termlab demo requires Nix with flakes enabled. See README.md.' >&2
    exit 127
fi
# Include new source files in working checkouts without copying target/ into
# Nix's flake input. Keep first startup independent of Git staging and artifacts.
termlab_source=$(mktemp -d "${TMPDIR:-/tmp}/termlab-demo-source.XXXXXXXX")
trap 'rm -rf -- "$termlab_source"' EXIT
for termlab_entry in Cargo.toml Cargo.lock flake.nix flake.lock src kitty scripts tests docs README.md LICENSE; do
    cp -R -- "$termlab_root/$termlab_entry" "$termlab_source/"
done
nix run --no-write-lock-file "path:$termlab_source#demo" -- "$@"
