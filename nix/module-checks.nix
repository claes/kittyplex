{ pkgs, nixpkgs, home-manager }:
let
  lib = pkgs.lib;
  home = extra: home-manager.lib.homeManagerConfiguration {
    inherit pkgs;
    modules = [ ./home-manager-module.nix {
      home.username = "alice"; home.homeDirectory = "/home/alice"; home.stateVersion = "26.05";
      programs.kittyplex.enable = true;
    } extra ];
  };
  normal = home {};
  custom = home { programs.kittyplex.kitty.prefix = "ctrl+alt+m"; };
  unsafe = home { programs.kitty.settings.allow_remote_control = "yes"; };
  package = pkgs.callPackage ./package.nix {};
  # Evaluate the actual consumer example without its host-specific configuration.nix.
  # Only replace the system constructor; evaluate the resulting Home Manager
  # settings using stock modules, independently of our integration module.
  manualExample = (import ../examples/flake-manual-home-manager.nix).outputs {
    nixpkgs = nixpkgs // { lib = nixpkgs.lib // { nixosSystem = args: args; }; };
    inherit home-manager;
    kittyplex.packages.${pkgs.stdenv.hostPlatform.system}.default = package;
  };
  manualHost = (lib.last manualExample.nixosConfigurations.myhost.modules) { inherit pkgs; };
  manual = home-manager.lib.homeManagerConfiguration {
    inherit pkgs;
    modules = [ {
      home.username = "alice"; home.homeDirectory = "/home/alice";
    } manualHost.home-manager.users.alice ];
  };
  integrationFiles = [ "kittyplex.py" "kittyplex_auth.py" "kittyplex_watcher.py" "kittyplex_launch.py" "kittyplex.conf" ];
  installManualFiles = lib.concatMapStringsSep "\n" (name: ''
    cp ${manual.config.xdg.configFile."kitty/${name}".source} "$TMPDIR/manual/${name}"
    cp ${manual.config.xdg.configFile."kitty/${name}".source} "$TMPDIR/plain/${name}"
  '') integrationFiles;
  valid = c: builtins.all (a: a.assertion) c.config.assertions;
  system = enableRun0: nixpkgs.lib.nixosSystem {
    system = pkgs.stdenv.hostPlatform.system;
    modules = [ ./nixos-module.nix {
      boot.isContainer = true; system.stateVersion = "26.05";
      programs.kittyplex = { enable = true; run0.enable = enableRun0; };
    } ];
  };
  defaultConfig = pkgs.writeText "kittyplex-default.conf" normal.config.xdg.configFile."kitty/kittyplex.conf".text;
  customConfig = pkgs.writeText "kittyplex-custom.conf" custom.config.xdg.configFile."kitty/kittyplex.conf".text;
in
assert valid normal;
assert valid custom;
assert valid manual;
assert !(manual.options.programs ? kittyplex);
assert !(manualHost ? programs.kittyplex);
assert manualHost.security.polkit.enable;
assert builtins.elem package manualHost.environment.systemPackages;
assert builtins.elem pkgs.kitty.terminfo manualHost.environment.systemPackages;
assert builtins.length manualExample.nixosConfigurations.myhost.modules == 3;
assert builtins.all (name:
  toString manual.config.xdg.configFile."kitty/${name}".source == "${package}/share/kittyplex/kitty/${name}"
) integrationFiles;
assert !(builtins.tryEval unsafe.activationPackage.drvPath).success;
assert !(system false).config.security.polkit.enable;
assert (system true).config.security.polkit.enable;
assert normal.config.xdg.configFile ? "kitty/kittyplex_launch.py";
assert lib.hasInfix "ctrl+alt+m" custom.config.xdg.configFile."kitty/kittyplex.conf".text;
assert !(lib.hasInfix "ctrl+shift+m" custom.config.xdg.configFile."kitty/kittyplex.conf".text);
pkgs.runCommand "kittyplex-module-checks" { nativeBuildInputs = [ pkgs.kitty ]; } ''
  export XDG_CACHE_HOME="$TMPDIR/cache"
  mkdir -p "$TMPDIR/manual" "$TMPDIR/plain"
  ${installManualFiles}
  cp ${manual.config.xdg.configFile."kitty/kitty.conf".source} "$TMPDIR/manual/kitty.conf"
  cp ${../examples/kitty.conf} "$TMPDIR/plain/kitty.conf"
  kitty +runpy 'from kitty.config import load_config
from pathlib import Path
import os
root=Path(os.environ["TMPDIR"])
for path in ["${defaultConfig}", "${customConfig}", str(root/"manual/kitty.conf"), str(root/"plain/kitty.conf")]:
 errors=[]
 options=load_config(path,accumulate_bad_lines=errors)
 assert options.allow_remote_control == "password", path
 assert not errors, errors'
  touch $out
''
