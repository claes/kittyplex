{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    kittyplex = {
      url = "github:claes/kittyplex";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { nixpkgs, kittyplex, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix # Your existing host configuration.
        ({ pkgs, ... }: {
          environment.systemPackages = [
            kittyplex.packages.${pkgs.stdenv.hostPlatform.system}.default
            pkgs.kitty
            pkgs.kitty.terminfo
          ];
          environment.pathsToLink = [ "/share/kittyplex" ];
          security.polkit.enable = true; # run0 authentication, with normal policy.
        })
      ];
    };
  };
}
