{ lib, rustPlatform }:
rustPlatform.buildRustPackage {
  pname = "termlab";
  version = "0.4.0";
  src = lib.cleanSourceWith {
    src = ../.;
    # Module/VM-only changes should not rebuild the Rust executable.
    filter = path: type: lib.cleanSourceFilter path type
      && !(builtins.elem (builtins.baseNameOf path) [ "target" "__pycache__" "result" "nix" ]);
  };
  cargoLock.lockFile = ../Cargo.lock;
  postInstall = ''
    mkdir -p $out/share/termlab
    cp -r kitty scripts $out/share/termlab/
    substituteInPlace $out/share/termlab/kitty/termlab.py \
      --replace-fail 'PACKAGED_BINARY = None' "PACKAGED_BINARY = '$out/bin/termlab'"
  '';
  # PTY tests need /dev/ptmx; run explicitly in the dev shell instead.
  doCheck = false;
  meta = { description = "Remote PTYs with Kitty-owned screens"; license = lib.licenses.mit; platforms = lib.platforms.linux; mainProgram = "termlab"; };
}
