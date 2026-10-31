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
# Use a named profile so file writes stay workspace-scoped while commands can
# bind TCP/Unix sockets directly (SSH, kitty IPC, and Xvfb). A network proxy
# cannot carry these listeners. Do not mix this profile with --sandbox or the
# legacy sandbox_workspace_write settings: those take precedence over profiles.
# Administrator-managed requirements can still reject or restrict this policy.
exec nix develop --no-write-lock-file "path:$termlab_root" --command \
    "$termlab_codex" \
    --no-daemon \
    --cd "$termlab_root" \
    --ask-for-approval on-request \
    -c 'default_permissions="termlab-dev"' \
    -c 'permissions.termlab-dev.extends=":workspace"' \
    -c 'permissions.termlab-dev.network.enabled=true' \
    -c 'permissions.termlab-dev.network.allow_local_binding=true' \
    -c 'features.network_proxy=false' \
    -c 'allow_login_shell=false' \
    -c 'shell_environment_policy.inherit="all"' \
    "$@"
