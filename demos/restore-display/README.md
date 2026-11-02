# Restore Display reference

A static image, a status line, and an independent PTY exerciser. From the repository
root, run the guided demonstration:

```sh
./scripts/restore-display-demo.sh
```

Nix builds the tools automatically. Press `i` to generate numbered images of
colored boxes. Follow the Enter prompts to detach and restore the selected image
unchanged in a fresh Kitty window. Enter works in either window and `q` finishes
normally. The guide handles setup and cleanup; it never changes images on detach.

To build the binaries manually:

```sh
cargo build --locked --manifest-path demos/restore-display/Cargo.toml
```

See the [walkthrough and integration guide](../../docs/restore-display-demo.md),
[wire specification](../../docs/terminal-recovery-protocol-spec.md), and
[staged requirements](../../docs/terminal-recovery-demo-spec.md).

The library separates the bounded wire decoder (`codec.rs`), application model,
image renderer and adapter (`demo.rs`), and persistent controller (`exerciser/`).
Neither executable depends on Kittyplex, SSH or a terminal emulator library.
