#!/usr/bin/env bash
# Start a project-local Codex session with the termlab development tools.
set -euo pipefail

termlab_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)

if ! command -v nix >/dev/null 2>&1; then
    echo 'codex-dev.sh: nix is required for the project development shell' >&2
    exit 127
fi

# Resolve Codex before entering the dev shell; keep the user's installation,
# authentication, model, and other settings. Do not change CODEX_HOME.
if ! termlab_codex=$(command -v codex); then
    echo 'codex-dev.sh: codex is not on PATH' >&2
    exit 127
fi

cd -- "$termlab_root"

# --no-daemon starts a server with this dev shell's environment, rather than
# reusing a shared server that may have started without Rust on PATH.
# Non-login command shells preserve that PATH instead of resetting it.
# Networking supports Cargo/Nix downloads and local socket tests. Managed
# requirements can still restrict it; no sandbox bypass is used here.
exec nix develop --no-write-lock-file "path:$termlab_root" --command \
    "$termlab_codex" \
    --no-daemon \
    --cd "$termlab_root" \
    --sandbox workspace-write \
    --ask-for-approval on-request \
    -c 'sandbox_workspace_write.network_access=true' \
    -c 'allow_login_shell=false' \
    -c 'shell_environment_policy.inherit="all"' \
    "$@"
