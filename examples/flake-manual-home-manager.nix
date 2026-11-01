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
        home-manager.nixosModules.home-manager
        ({ pkgs, ... }:
          let
            kittyplexPackage = kittyplex.packages.${pkgs.stdenv.hostPlatform.system}.default;
            integration = "${kittyplexPackage}/share/kittyplex/kitty";
          in {
            # Standard NixOS options; no kittyplex integration modules are imported.
            environment.systemPackages = [ kittyplexPackage pkgs.kitty.terminfo ];
            security.polkit.enable = true; # Normal run0 authentication; systemd >=258.
            home-manager.useGlobalPkgs = true;
            home-manager.useUserPackages = true;
            home-manager.users.alice = {
              home.stateVersion = "26.05"; # Keep your existing value if already set.
              programs.kitty = {
                enable = true;
                settings.allow_remote_control = "password";
                extraConfig = ''
                  include kittyplex.conf
                  # Optional: choose unused chords for your desktop.
                  # map ctrl+alt+f8 kittyplex-run0 -u bob --pty
                  # map ctrl+alt+f9 kittyplex-ssh user@server
                  # map ctrl+alt+f10 kittyplex-local
                '';
              };
              # All five files must be in Kitty's actual configuration directory.
              xdg.configFile."kitty/kittyplex.py".source = "${integration}/kittyplex.py";
              xdg.configFile."kitty/kittyplex_auth.py".source = "${integration}/kittyplex_auth.py";
              xdg.configFile."kitty/kittyplex_watcher.py".source = "${integration}/kittyplex_watcher.py";
              xdg.configFile."kitty/kittyplex_launch.py".source = "${integration}/kittyplex_launch.py";
              xdg.configFile."kitty/kittyplex.conf".source = "${integration}/kittyplex.conf";
            };
          })
      ];
    };
  };
}
