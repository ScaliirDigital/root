{
  description = "Basic Bazel Flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs = {nixpkgs, ...}: let
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
    in rec {
      devShells.default = pkgs.mkShell {
        packages = with pkgs; [
          # Bazel tooling
          bazelisk
          buildifier

          # Generic tooling
          git
          curl
          jq

          # C / C++
          clang
          clang-tools

          # Go
          go
          gopls
          gotools

          # Python
          uv
          python313

          # Rust
          cargo
          rustc
          rustfmt
          clippy

          # Proto
          protobuf
        ];
        shellHook = ''
          alias bazel=bazelisk

          echo "Welcome to bazel build dev shell on ${system}!"

          echo ""
          echo "Useful commands:"
          echo "  bazel query //..."
          echo "  bazel build ..."
          echo "  bazel test ..."
          echo "  bazel query //..."
        '';
      };
    };
  in {
    devShells = forEachSystem (system: (createSpace system).devShells);
  };
}
