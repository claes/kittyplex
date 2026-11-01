{ config, lib, pkgs, ... }:
let
  cfg = config.programs.termlab;
  helpers = [ "termlab.py" "termlab_auth.py" "termlab_watcher.py" "termlab_launch.py" ];
  integration = lib.replaceStrings [ "ctrl+shift+m" ] [ cfg.kitty.prefix ] (builtins.readFile ../kitty/termlab.conf);
in {
  options.programs.termlab = {
    enable = lib.mkEnableOption "termlab desktop integration";
    package = lib.mkOption { type = lib.types.package; default = pkgs.callPackage ./package.nix {}; description = "Compatible termlab package; use the same package on the system and in Home Manager."; };
    kitty = {
      enable = lib.mkOption { type = lib.types.bool; default = true; description = "Manage the Kitty integration files and include."; };
      prefix = lib.mkOption { type = lib.types.str; default = "ctrl+shift+m"; description = "Single Kitty key chord for management mode and literal-prefix forwarding."; };
    };
  };
  config = lib.mkIf cfg.enable (lib.mkMerge [
    { home.packages = [ cfg.package ]; }
    (lib.mkIf cfg.kitty.enable {
      assertions = [
        { assertion = builtins.match "[^[:space:]]+" cfg.kitty.prefix != null; message = "programs.termlab.kitty.prefix must be one nonempty Kitty chord without whitespace."; }
        { assertion = config.programs.kitty.settings.allow_remote_control == "password"; message = "termlab requires programs.kitty.settings.allow_remote_control = \"password\", not unrestricted remote control."; }
      ];
      programs.kitty.enable = lib.mkDefault true;
      programs.kitty.settings.allow_remote_control = lib.mkDefault "password";
      programs.kitty.extraConfig = lib.mkAfter "\ninclude termlab.conf\n";
      xdg.configFile = (lib.genAttrs (map (name: "kitty/${name}") helpers)
        (name: { source = "${cfg.package}/share/termlab/${name}"; })) // {
          "kitty/termlab.conf".text = integration;
        };
    })
  ]);
}
