{ lib, rustPlatform }:
rustPlatform.buildRustPackage {
  pname = "restore-display";
  version = "0.1.0";
  src = lib.cleanSourceWith {
    src = ../demos/restore-display;
    filter = path: type: lib.cleanSourceFilter path type
      && !(builtins.elem (builtins.baseNameOf path) [ "target" "__pycache__" ]);
  };
  cargoLock.lockFile = ../demos/restore-display/Cargo.lock;
  postInstall = ''
    install -Dm644 walkthrough.py $out/share/restore-display/walkthrough.py
  '';
  # PTY and Kitty/Xvfb tests run explicitly in the development shell.
  doCheck = false;
  meta = {
    description = "Minimal Restore Display image demo and independent PTY exerciser";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "restore-display-demo-rust-direct";
  };
}
