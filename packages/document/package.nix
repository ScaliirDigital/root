{pkgs}: let
  isLinux = pkgs.stdenv.hostPlatform.isLinux;
  isMusl = pkgs.stdenv.hostPlatform.isMusl;

  target =
    if isLinux && !isMusl
    then pkgs.pkgsStatic
    else pkgs;

  buildTools = pkgs.buildPackages;

  canExecute =
    target.stdenv.buildPlatform.canExecute
    target.stdenv.hostPlatform;

  source = pkgs.lib.cleanSourceWith {
    name = "document-src";
    src = ./.;

    filter = path: type:
      pkgs.lib.cleanSourceFilter path type
      && builtins.baseNameOf (toString path) != "package.nix";
  };
in
  target.rustPlatform.buildRustPackage {
    pname = "document";
    version = "0.1.0";

    src = source;
    cargoLock.lockFile = "${source}/Cargo.lock";

    env =
      {
        CARGO_BUILD_TARGET = target.stdenv.hostPlatform.rust.cargoShortTarget;
        AWS_LC_SYS_CMAKE_BUILDER = "0";
        SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      }
      // pkgs.lib.optionalAttrs target.stdenv.hostPlatform.isMusl {
        RUSTFLAGS = "-C target-feature=+crt-static";
      };

    stripAllList = ["bin"];

    nativeBuildInputs = [
      buildTools.file
    ];

    nativeCheckInputs = pkgs.lib.optionals canExecute [
      buildTools.rustfs
      buildTools.curl
    ];

    doCheck = canExecute;

    postInstall =
      ''
        test -x "$out/bin/document"
      ''
      + pkgs.lib.optionalString canExecute ''
        "$out/bin/document" --version
        "$out/bin/document" --help >/dev/null
      ''
      + pkgs.lib.optionalString isLinux ''
        fileOutput="$(
          ${buildTools.file}/bin/file \
            "$out/bin/document"
        )"

        echo "$fileOutput"
        echo "$fileOutput" |
          grep -E "statically linked|static-pie linked"
      '';

    meta = {
      description = "Deterministic PDF rendering as a service";
      homepage = "https://github.com/ScaliirDigital/root";
      mainProgram = "document";
      license = pkgs.lib.licenses.bsd3;
      platforms = pkgs.lib.platforms.linux ++ pkgs.lib.platforms.darwin;
    };
  }
