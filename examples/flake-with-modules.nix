{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager = {
      url = "github:nix-community/home-manager/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    kittyplex = {
      url = "github:claes/kittyplex";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { nixpkgs, home-manager, kittyplex, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux"; # Or aarch64-linux.
      modules = [
        ./configuration.nix # Your existing host configuration, including users.
        kittyplex.nixosModules.default
        home-manager.nixosModules.home-manager
        ({ pkgs, ... }: {
          programs.kittyplex = {
            enable = true;
            package = kittyplex.packages.${pkgs.stdenv.hostPlatform.system}.default;
            run0.enable = true;
          };
          home-manager.useGlobalPkgs = true;
          home-manager.useUserPackages = true;
          home-manager.sharedModules = [ kittyplex.homeManagerModules.default ];
          home-manager.users.alice = {
            home.stateVersion = "26.05"; # Keep your existing value if already set.
            programs.kittyplex = {
              enable = true;
              package = kittyplex.packages.${pkgs.stdenv.hostPlatform.system}.default;
              # kitty.prefix = "ctrl+alt+m"; # Optional; default ctrl+shift+m.
            };
          };
        })
      ];
    };
  };
}
