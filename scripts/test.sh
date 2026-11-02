#!/usr/bin/env bash
set -euo pipefail
kittyplex_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd "$kittyplex_root"
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
cargo fmt --manifest-path demos/restore-display/Cargo.toml --check
cargo clippy --locked --manifest-path demos/restore-display/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path demos/restore-display/Cargo.toml
cargo build --locked --manifest-path demos/restore-display/Cargo.toml
python3 demos/restore-display/tests/integration.py
python3 demos/restore-display/tests/walkthrough.py
python3 tests/documentation.py --lint-man
kitty +runpy 'import runpy; runpy.run_path("tests/kitty_config.py", run_name="__main__")'
python3 tests/integration.py "$@"
python3 tests/compatibility.py
python3 tests/desktop.py
