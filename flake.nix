{
  description = "Basic Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    pre-commit-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nix2container = {
      url = "github:nlewo/nix2container";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    pre-commit-hooks,
    nix2container,
    rust-overlay,
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

      imageBuilder = import ./images/builder.nix {
        inherit
          nixpkgs
          nix2container
          rust-overlay
          system
          ;
      };

      config = {
        pre-commit = pre-commit-hooks.lib.${system}.run {
          src = self;

          hooks = import ./tools/pre-commit.nix {inherit pkgs;};
        };
      };
    in {
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

      packages = {
        document = import ./packages/document/package.nix {inherit pkgs;};
      };

      legacyPackages = {
        image = {
          document = imageBuilder.build {
            package = ./packages/document/package.nix;
            image = ./images/document.nix;
          };
        };
      };
    };
  in {
    devShells = forEachSystem (system: (createSpace system).devShells);
    formatter = forEachSystem (system: (createSpace system).formatter);
    checks = forEachSystem (system: (createSpace system).checks);
    packages = forEachSystem (system: (createSpace system).packages);
    legacyPackages = forEachSystem (system: (createSpace system).legacyPackages);
  };
}
