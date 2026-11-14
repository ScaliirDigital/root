{
  pkgs,
  application,
  files ? [],
  system ? pkgs.stdenv.hostPlatform.system,
}: let
  name = "${application.pname}-${application.version}-${system}";
  executable = application.meta.mainProgram;
in
  pkgs.runCommand "${name}-archive" {
    nativeBuildInputs = [
      pkgs.gnutar
      pkgs.gzip
    ];
  } ''
    mkdir -p "$out" staging

    cp "${application}/bin/${executable}" staging/

    ${pkgs.lib.concatMapStrings (file: ''
        cp "${file}" "staging/${baseNameOf file}"
      '')
      files}

    tar \
      --sort=name \
      --mtime=@0 \
      --owner=0 \
      --group=0 \
      --numeric-owner \
      -C staging \
      -cf - . |
      gzip -n > "$out/${name}.tar.gz"
  ''
