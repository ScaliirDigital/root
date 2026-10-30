{
  nixpkgs,
  nix2container,
  rust-overlay,
  system,
}: let
  inherit (nixpkgs) lib;

  architectures = {
    "x86_64" = {
      system = "x86_64-linux";
      config = "x86_64-unknown-linux-musl";
      arch = "amd64";
    };

    "aarch64" = {
      system = "aarch64-linux";
      config = "aarch64-unknown-linux-musl";
      arch = "arm64";
    };
  };

  cpu = lib.head (lib.splitString "-" system);

  target = architectures.${cpu} or (throw "Unsupported image build architecture: ${cpu}");

  pkgs = import nixpkgs {
    inherit system;
    config = {};
  };

  toolchain = import ../tools/toolchain.nix {
    inherit
      nixpkgs
      rust-overlay
      system
      target
      ;
  };

  buildTools =
    if system == target.system
    then pkgs
    else toolchain.pkgs;

  container = nix2container.packages.${system}.nix2container;
in {
  build = {
    package,
    image,
  }: let
    application = import package {
      pkgs = buildTools;
    };
  in
    import image {
      pkgs = buildTools;
      inherit application container;
      inherit (target) arch;
    };
}
