{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    termlab = {
      url = "github:claes/termlab";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { nixpkgs, termlab, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix # Your existing host configuration.
        ({ pkgs, ... }: {
          environment.systemPackages = [
            termlab.packages.${pkgs.stdenv.hostPlatform.system}.default
            pkgs.kitty
            pkgs.kitty.terminfo
          ];
          environment.pathsToLink = [ "/share/termlab" ];
          security.polkit.enable = true; # run0 authentication, with normal policy.
        })
      ];
    };
  };
}
