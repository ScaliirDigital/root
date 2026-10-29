{pkgs}:
pkgs.writeShellApplication {
  name = "release";

  runtimeInputs = [
    pkgs.bun
    pkgs.cargo
    pkgs.git
    pkgs.gh
  ];

  text = ''
    exec bun ${./src}/index.ts "$@"
  '';
}
