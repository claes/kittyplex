#!/usr/bin/env bash
set -euo pipefail
termlab_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$termlab_root"
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
kitty +runpy 'import runpy; runpy.run_path("tests/kitty_config.py", run_name="__main__")'
python3 tests/integration.py "$@"
