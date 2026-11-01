{ config, lib, pkgs, ... }:
let cfg = config.programs.kittyplex; in {
  options.programs.kittyplex = {
    enable = lib.mkEnableOption "kittyplex terminal sessions and desktop launchers";
    package = lib.mkOption { type = lib.types.package; default = pkgs.callPackage ./package.nix {}; description = "Compatible kittyplex package."; };
    run0.enable = lib.mkEnableOption "run0 prerequisites (Polkit, without additional authorization rules)";
  };
  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      environment.systemPackages = [ cfg.package pkgs.kitty.terminfo ];
      environment.pathsToLink = [ "/share/kittyplex" ];
    }
    (lib.mkIf cfg.run0.enable {
      security.polkit.enable = lib.mkDefault true;
      assertions = [
        { assertion = lib.versionAtLeast config.systemd.package.version "258"; message = "kittyplex run0 requires systemd >=258."; }
        { assertion = config.security.polkit.enable; message = "kittyplex run0 requires Polkit; disable programs.kittyplex.run0.enable if this host is SSH-only."; }
      ];
    })
  ]);
}
