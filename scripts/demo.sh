#!/usr/bin/env bash
set -euo pipefail
kittyplex_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
if ! command -v nix >/dev/null; then
    echo 'kittyplex demo requires Nix with flakes enabled. See README.md.' >&2
    exit 127
fi
# Include new source files in working checkouts without copying target/ into
# Nix's flake input. Keep first startup independent of Git staging and artifacts.
kittyplex_source=$(mktemp -d "${TMPDIR:-/tmp}/kittyplex-demo-source.XXXXXXXX")
trap 'rm -rf -- "$kittyplex_source"' EXIT
for kittyplex_entry in Cargo.toml Cargo.lock flake.nix flake.lock src kitty scripts tests docs nix examples README.md LICENSE; do
    cp -R -- "$kittyplex_root/$kittyplex_entry" "$kittyplex_source/"
done
# The controller shares its Restore Display codec with the reference application.
mkdir -p "$kittyplex_source/demos/restore-display"
for kittyplex_entry in Cargo.toml Cargo.lock README.md src assets scenarios tests walkthrough.py; do
    cp -R -- "$kittyplex_root/demos/restore-display/$kittyplex_entry" "$kittyplex_source/demos/restore-display/"
done
nix run --no-write-lock-file "path:$kittyplex_source#demo" -- "$@"
