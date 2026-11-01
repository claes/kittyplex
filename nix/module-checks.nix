{ pkgs, nixpkgs, home-manager }:
let
  lib = pkgs.lib;
  home = extra: home-manager.lib.homeManagerConfiguration {
    inherit pkgs;
    modules = [ ./home-manager-module.nix {
      home.username = "alice"; home.homeDirectory = "/home/alice"; home.stateVersion = "26.05";
      programs.termlab.enable = true;
    } extra ];
  };
  normal = home {};
  custom = home { programs.termlab.kitty.prefix = "ctrl+alt+m"; };
  unsafe = home { programs.kitty.settings.allow_remote_control = "yes"; };
  valid = c: builtins.all (a: a.assertion) c.config.assertions;
  system = enableRun0: nixpkgs.lib.nixosSystem {
    system = pkgs.stdenv.hostPlatform.system;
    modules = [ ./nixos-module.nix {
      boot.isContainer = true; system.stateVersion = "26.05";
      programs.termlab = { enable = true; run0.enable = enableRun0; };
    } ];
  };
  defaultConfig = pkgs.writeText "termlab-default.conf" normal.config.xdg.configFile."kitty/termlab.conf".text;
  customConfig = pkgs.writeText "termlab-custom.conf" custom.config.xdg.configFile."kitty/termlab.conf".text;
in
assert valid normal;
assert valid custom;
assert !(builtins.tryEval unsafe.activationPackage.drvPath).success;
assert !(system false).config.security.polkit.enable;
assert (system true).config.security.polkit.enable;
assert normal.config.xdg.configFile ? "kitty/termlab_launch.py";
assert lib.hasInfix "ctrl+alt+m" custom.config.xdg.configFile."kitty/termlab.conf".text;
assert !(lib.hasInfix "ctrl+shift+m" custom.config.xdg.configFile."kitty/termlab.conf".text);
pkgs.runCommand "termlab-module-checks" { nativeBuildInputs = [ pkgs.kitty ]; } ''
  export XDG_CACHE_HOME="$TMPDIR/cache"
  kitty +runpy 'from kitty.config import load_config
for path in ["${defaultConfig}", "${customConfig}"]:
 errors=[]
 load_config(path,accumulate_bad_lines=errors)
 assert not errors, errors'
  touch $out
''
