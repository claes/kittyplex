{ config, lib, pkgs, ... }:
let cfg = config.programs.termlab; in {
  options.programs.termlab = {
    enable = lib.mkEnableOption "termlab terminal sessions and desktop launchers";
    package = lib.mkOption { type = lib.types.package; default = pkgs.callPackage ./package.nix {}; description = "Compatible termlab package."; };
    run0.enable = lib.mkEnableOption "run0 prerequisites (Polkit, without additional authorization rules)";
  };
  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      environment.systemPackages = [ cfg.package pkgs.kitty.terminfo ];
      environment.pathsToLink = [ "/share/termlab" ];
    }
    (lib.mkIf cfg.run0.enable {
      security.polkit.enable = lib.mkDefault true;
      assertions = [
        { assertion = lib.versionAtLeast config.systemd.package.version "258"; message = "termlab run0 requires systemd >=258."; }
        { assertion = config.security.polkit.enable; message = "termlab run0 requires Polkit; disable programs.termlab.run0.enable if this host is SSH-only."; }
      ];
    })
  ]);
}
