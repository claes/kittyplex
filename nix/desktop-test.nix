{ pkgs, home-manager }:
let termlab = pkgs.callPackage ./package.nix {}; in
pkgs.testers.runNixOSTest {
  name = "termlab-desktop";
  # Prefer acceleration when available; allow slower TCG on development hosts.
  requiredFeatures.kvm = false;
  nodes.desktop = { ... }: {
    imports = [ ./nixos-module.nix home-manager.nixosModules.home-manager ];
    programs.termlab = { enable = true; package = termlab; run0.enable = true; };
    services.openssh.enable = true;
    hardware.graphics.enable = true; # llvmpipe for Kitty on the private Xvfb display.
    users.users.alice = { isNormalUser = true; uid = 1000; initialPassword = "test"; extraGroups = [ "wheel" ]; };
    users.users.bob = { isNormalUser = true; uid = 1001; initialPassword = "test"; shell = pkgs.zsh; };
    users.users.denied = { isNormalUser = true; initialPassword = "test"; };
    users.users.carol = { isNormalUser = true; initialPassword = "test"; shell = pkgs.fish; };
    programs.fish.enable = true;
    programs.fish.interactiveShellInit = ''if test "$USER" = carol; function fish_prompt; printf "CAROL_READY> "; end; end'';
    programs.zsh.enable = true;
    programs.zsh.enableGlobalCompInit = false; # Avoid first-run completion scans under TCG.
    programs.zsh.promptInit = ''if [[ $USER == bob ]]; then PROMPT='BOB_READY> '; fi'';
    programs.bash.promptInit = ''if [[ $USER == root ]]; then PS1='ROOT_READY> '; fi'';
    home-manager.sharedModules = [ ./home-manager-module.nix ];
    home-manager.users.alice = {
      home.stateVersion = "26.05";
      programs.termlab = { enable = true; package = termlab; };
    };
    # Test VM only. The exported production module never adds authorization rules.
    security.polkit.extraConfig = ''
      polkit.addRule(function(action, subject) {
        if (action.id === "org.freedesktop.systemd1.manage-units") {
          if (subject.user === "alice") return polkit.Result.YES;
          if (subject.user === "denied") return polkit.Result.NO;
        }
      });
    '';
    environment.systemPackages = [ pkgs.kitty pkgs.python3 pkgs.xorg-server pkgs.openssh pkgs.procps ];
    virtualisation.memorySize = 2048;
  };
  testScript = ''
    desktop.start()
    desktop.wait_for_unit("multi-user.target")
    desktop.wait_for_unit("home-manager-alice.service")
    desktop.succeed("test -L /home/alice/.config/kitty/termlab_launch.py")
    status, output = desktop.execute("su - alice -c 'env TERMLAB_TEST_BINARY=${termlab}/bin/termlab TERMLAB_TEST_INTEGRATION=/home/alice/.config/kitty TERMLAB_TEST_RUN0_USER=bob TERMLAB_TEST_TIMEOUT_FACTOR=4 python3 ${termlab.src}/tests/desktop.py DesktopTests.test_real_run0_identity_shell_home_and_pty_modes DesktopTests.test_run0_explicit_group_directory_and_job_control -f'")
    print(output)
    if status != 0:
        print(desktop.succeed("cat /tmp/termlab-demo-*/kitty.log /tmp/termlab-demo-*/sshd.log /tmp/termlab-demo-*/screens.json 2>/dev/null || true"))
        raise Exception("Desktop integration suite failed")
    status, output = desktop.execute("su - denied -c 'run0 --no-ask-password --pipe true' 2>&1")
    assert status != 0, "Unauthorized run0 unexpectedly succeeded"
    assert "denied" in output.lower() or "not authorized" in output.lower(), output
  '';
}
