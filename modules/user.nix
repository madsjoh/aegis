{ metis }:

{ pkgs, ... }:

{
  users.users.agent = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    home = "/home/agent";
  };

  home-manager = {
    useGlobalPkgs = true;
    useUserPackages = true;

    users.agent = {
      imports = [
        metis.homeManagerModules.default
      ];

      metis.opencode = {
        enable = true;
      };

      programs.git = {
        enable = true;
        settings.user.name = builtins.getEnv "VM_GIT_NAME";
        settings.user.email = builtins.getEnv "VM_GIT_EMAIL";
        ignores = [
          ".aegis.lock"
          ".opencode/"
          ".opencode-env"
        ];
      };

      home.packages = with pkgs; [
        opencode
        gh
        git
        ripgrep
        fd
        bash
        fish
      ];

      home.stateVersion = "24.05";
    };
  };
}
