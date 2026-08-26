{ metis }:

{ pkgs, lib, ... }:

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
        ignores = [
          ".aegis.lock"
          ".aegis/"
          ".opencode/"
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
