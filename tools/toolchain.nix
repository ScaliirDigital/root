{
  nixpkgs,
  rust-overlay,
  system,
  target,
}: let
  nativePkgs = import nixpkgs {
    inherit system;

    overlays = [
      rust-overlay.overlays.default
    ];

    config = {};
  };

  targetPkgs = import nixpkgs {
    localSystem = {
      inherit system;
    };

    crossSystem = {
      inherit (target) config;
    };

    config = {};
  };

  llvmPkgs = targetPkgs.pkgsLLVM;

  rust = nativePkgs.rust-bin.stable."1.97.1".minimal.override {
    targets = [
      target.config
    ];
  };

  rustPlatform = llvmPkgs.makeRustPlatform {
    cargo = rust;
    rustc = rust;
  };
in {
  pkgs =
    llvmPkgs
    // {
      inherit rustPlatform;
    };
}
