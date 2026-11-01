{ lib, rustPlatform }:
rustPlatform.buildRustPackage {
  pname = "kittyplex";
  version = "0.4.0";
  src = lib.cleanSourceWith {
    src = ../.;
    # Module/VM-only changes should not rebuild the Rust executable.
    filter = path: type: lib.cleanSourceFilter path type
      && !(builtins.elem (builtins.baseNameOf path) [ "target" "__pycache__" "result" "nix" ]);
  };
  cargoLock.lockFile = ../Cargo.lock;
  postInstall = ''
    mkdir -p $out/share/kittyplex
    cp -r kitty scripts $out/share/kittyplex/
    mkdir -p $out/share/man/man1
    cp docs/kittyplex.1 $out/share/man/man1/
    substituteInPlace $out/share/kittyplex/kitty/kittyplex.py \
      --replace-fail 'PACKAGED_BINARY = None' "PACKAGED_BINARY = '$out/bin/kittyplex'"
  '';
  # PTY tests need /dev/ptmx; run explicitly in the dev shell instead.
  doCheck = false;
  meta = { description = "Remote PTYs with Kitty-owned screens"; license = lib.licenses.mit; platforms = lib.platforms.linux; mainProgram = "kittyplex"; };
}
