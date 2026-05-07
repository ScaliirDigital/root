{
  description = "Basic Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    pre-commit-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    pre-commit-hooks,
    ...
  }: let
    systems = [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ];
    forEachSystem = nixpkgs.lib.genAttrs systems;

    createSpace = system: let
      pkgs = import nixpkgs {
        inherit system;
        config = {};
      };
      config = {
        pre-commit = pre-commit-hooks.lib.${system}.run {
          src = self;
          hooks = import ./tools/pre-commit.nix {inherit pkgs;};
        };
      };
    in rec {
      checks.pre-commit = config.pre-commit;
      formatter = pkgs.alejandra;

      devShells.default = pkgs.mkShell {
        packages = with pkgs; [
          # Development tooling
          git
          tokei
          ripgrep
          gh
          pre-commit

          # Web tooling
          bun
        ];
        shellHook = ''
          # Generate the .pre-commit-config.yaml symlink when entering the dev shell
          ${config.pre-commit.shellHook}

          echo "Welcome to root dev shell on ${system}!"
        '';
      };
    };
  in {
    devShells = forEachSystem (system: (createSpace system).devShells);
    formatter = forEachSystem (system: (createSpace system).formatter);
    checks = forEachSystem (system: (createSpace system).checks);

    templates = import ./tools/template.nix;
  };
}
