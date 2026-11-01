{ config, lib, pkgs, ... }:
let
  cfg = config.programs.kittyplex;
  helpers = [ "kittyplex.py" "kittyplex_auth.py" "kittyplex_watcher.py" "kittyplex_launch.py" "kittyplex_help.py" ];
  integration = lib.replaceStrings [ "ctrl+shift+m" ] [ cfg.kitty.prefix ] (builtins.readFile ../kitty/kittyplex.conf);
in {
  options.programs.kittyplex = {
    enable = lib.mkEnableOption "kittyplex desktop integration";
    package = lib.mkOption { type = lib.types.package; default = pkgs.callPackage ./package.nix {}; description = "Compatible kittyplex package; use the same package on the system and in Home Manager."; };
    kitty = {
      enable = lib.mkOption { type = lib.types.bool; default = true; description = "Manage the Kitty integration files and include."; };
      prefix = lib.mkOption { type = lib.types.str; default = "ctrl+shift+m"; description = "Single Kitty key chord for management mode and literal-prefix forwarding."; };
    };
  };
  config = lib.mkIf cfg.enable (lib.mkMerge [
    { home.packages = [ cfg.package ]; }
    (lib.mkIf cfg.kitty.enable {
      assertions = [
        { assertion = builtins.match "[^[:space:]]+" cfg.kitty.prefix != null; message = "programs.kittyplex.kitty.prefix must be one nonempty Kitty chord without whitespace."; }
        { assertion = config.programs.kitty.settings.allow_remote_control == "password"; message = "kittyplex requires programs.kitty.settings.allow_remote_control = \"password\", not unrestricted remote control."; }
      ];
      programs.kitty.enable = lib.mkDefault true;
      programs.kitty.settings.allow_remote_control = lib.mkDefault "password";
      programs.kitty.extraConfig = lib.mkAfter "\ninclude kittyplex.conf\n";
      xdg.configFile = (lib.genAttrs (map (name: "kitty/${name}") helpers)
        (name: { source = "${cfg.package}/share/kittyplex/${name}"; })) // {
          "kitty/kittyplex.conf".text = integration;
        };
    })
  ]);
}
