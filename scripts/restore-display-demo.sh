#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --help || "${1:-}" == -h ]]; then
    echo 'Usage: ./scripts/restore-display-demo.sh'
    echo 'Build and run the guided image detach/reattach walkthrough. Follow three Enter prompts.'
    exit 0
fi
restore_display_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
if ! command -v nix >/dev/null; then
    echo 'The Restore Display walkthrough requires Nix with flakes enabled.' >&2
    exit 127
fi
# A filtered snapshot includes new, unstaged files without importing target/.
restore_display_source=$(mktemp -d "${TMPDIR:-/tmp}/restore-display-src.XXXXXXXX")
trap 'rm -rf -- "$restore_display_source"' EXIT
for restore_display_entry in Cargo.toml Cargo.lock flake.nix flake.lock src kitty scripts tests docs nix examples README.md LICENSE; do
    cp -R -- "$restore_display_root/$restore_display_entry" "$restore_display_source/"
done
mkdir -p "$restore_display_source/demos/restore-display"
for restore_display_entry in Cargo.toml Cargo.lock README.md src assets scenarios tests walkthrough.py; do
    cp -R -- "$restore_display_root/demos/restore-display/$restore_display_entry" "$restore_display_source/demos/restore-display/"
done
echo 'Preparing the walkthrough (the first build may take a little while)...'
nix run --no-write-lock-file "path:$restore_display_source#restore-display-walkthrough" -- "$@"
