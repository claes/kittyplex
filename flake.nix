{
  description = "kittyplex: remote PTYs and kitty-owned screens";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.home-manager = {
    url = "github:nix-community/home-manager/master";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs = { nixpkgs, home-manager, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
      packageFor = pkgs: pkgs.callPackage ./nix/package.nix {};
      restoreDisplayFor = pkgs: pkgs.callPackage ./nix/restore-display.nix {};
      restoreWalkthroughFor = pkgs: let restoreDisplay = restoreDisplayFor pkgs; in pkgs.writeShellApplication {
        name = "restore-display-walkthrough";
        runtimeInputs = with pkgs; [ kitty python3 ];
        text = ''
          exec python3 ${restoreDisplay}/share/restore-display/walkthrough.py \
            --bin-dir ${restoreDisplay}/bin "$@"
        '';
      };
      demoFor = pkgs: let kittyplex = packageFor pkgs; in pkgs.writeShellApplication {
        name = "kittyplex-demo";
        runtimeInputs = with pkgs; [ kitty openssh bash python3 coreutils vim less ];
        text = ''
          exec python3 ${kittyplex}/share/kittyplex/scripts/demo.py \
            --binary ${kittyplex}/bin/kittyplex \
            --integration ${kittyplex}/share/kittyplex/kitty "$@"
        '';
      };
    in {
      nixosModules.default = import ./nix/nixos-module.nix;
      homeManagerModules.default = import ./nix/home-manager-module.nix;
      packages = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        default = packageFor pkgs;
        demo = demoFor pkgs;
        restore-display = restoreDisplayFor pkgs;
        restore-display-walkthrough = restoreWalkthroughFor pkgs;
        desktop-test = import ./nix/desktop-test.nix { inherit pkgs home-manager; };
      });
      apps = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        default = { type = "app"; program = "${packageFor pkgs}/bin/kittyplex"; meta.description = "Remote PTYs with kitty-owned screens"; };
        restore-display-walkthrough = { type = "app"; program = "${restoreWalkthroughFor pkgs}/bin/restore-display-walkthrough"; meta.description = "Guided image detach/reattach walkthrough"; };
        restore-display-demo = { type = "app"; program = "${restoreDisplayFor pkgs}/bin/restore-display-demo-rust-direct"; meta.description = "Minimal Restore Display image demo"; };
        restore-display-exerciser = { type = "app"; program = "${restoreDisplayFor pkgs}/bin/restore-display-exerciser"; meta.description = "Independent Restore Display PTY exerciser"; };
        demo = { type = "app"; program = "${demoFor pkgs}/bin/kittyplex-demo"; meta.description = "Isolated interactive SSH/kitty demo"; };
      });
      checks = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        build = packageFor pkgs;
        restore-display = restoreDisplayFor pkgs;
        restore-display-walkthrough = restoreWalkthroughFor pkgs;
        modules = import ./nix/module-checks.nix { inherit pkgs nixpkgs home-manager; };
        documentation = let package = packageFor pkgs; in pkgs.runCommand "kittyplex-documentation-check" {
          nativeBuildInputs = [ pkgs.python3 pkgs.mandoc ];
        } ''
          python3 ${package.src}/tests/documentation.py --root ${package.src} \
            --binary ${package}/bin/kittyplex --lint-man --installed-man
          touch $out
        '';
        python = pkgs.runCommand "kittyplex-python-check" { nativeBuildInputs = [ pkgs.python3 ]; } ''
          export PYTHONPYCACHEPREFIX="$TMPDIR/pycache"
          python3 -m compileall -q ${./kitty} ${./scripts} ${./demos/restore-display/walkthrough.py}
          touch $out
        '';
      });
      devShells = eachSystem (system: let pkgs = nixpkgs.legacyPackages.${system}; in {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy python3 kitty openssh bash coreutils vim less xorg-server mandoc ];
        };
      });
    };
}
