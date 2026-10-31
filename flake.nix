{
  description = "termlab: remote PTYs and kitty-owned screens";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      packageFor = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "termlab";
        version = "0.2.0";
        src = nixpkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            nixpkgs.lib.cleanSourceFilter path type
            && !(builtins.elem (builtins.baseNameOf path) [ "target" "__pycache__" "result" ]);
        };
        cargoLock.lockFile = ./Cargo.lock;
        postInstall = ''
          mkdir -p $out/share/termlab
          cp -r kitty scripts $out/share/termlab/
        '';
        # PTY tests need /dev/ptmx; run explicitly in the dev shell instead.
        doCheck = false;
      };
      demoFor = pkgs: let termlab = packageFor pkgs; in pkgs.writeShellApplication {
        name = "termlab-demo";
        runtimeInputs = with pkgs; [ kitty openssh bash python3 coreutils vim less ];
        text = ''
          exec python3 ${termlab}/share/termlab/scripts/demo.py \
            --binary ${termlab}/bin/termlab \
            --integration ${termlab}/share/termlab/kitty "$@"
        '';
      };
    in {
      packages = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        default = packageFor pkgs;
        demo = demoFor pkgs;
      });
      apps = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        default = { type = "app"; program = "${packageFor pkgs}/bin/termlab"; meta.description = "Remote PTYs with kitty-owned screens"; };
        demo = { type = "app"; program = "${demoFor pkgs}/bin/termlab-demo"; meta.description = "Isolated interactive SSH/kitty demo"; };
      });
      checks = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        build = packageFor pkgs;
        python = pkgs.runCommand "termlab-python-check" { nativeBuildInputs = [ pkgs.python3 ]; } ''
          export PYTHONPYCACHEPREFIX="$TMPDIR/pycache"
          python3 -m compileall -q ${./kitty} ${./scripts}
          touch $out
        '';
      });
      devShells = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy python3 kitty openssh bash coreutils vim less xorg-server ];
        };
      });
    };
}
