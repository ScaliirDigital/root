{pkgs}: let
  isLinux = pkgs.stdenv.hostPlatform.isLinux;

  target =
    if isLinux
    then pkgs.pkgsStatic
    else pkgs;

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

    env = {
      AWS_LC_SYS_CMAKE_BUILDER = "0";
      SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
    };

    stripAllList = ["bin"];

    nativeBuildInputs = [
      target.buildPackages.file
    ];

    nativeCheckInputs = pkgs.lib.optionals canExecute [
      target.buildPackages.rustfs
      target.buildPackages.curl
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
        ${target.buildPackages.file}/bin/file "$out/bin/document" |
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
