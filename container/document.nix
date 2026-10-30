{
  pkgs,
  container,
  application,
  arch,
}: let
  document = application;
in
  container.buildImage {
    inherit arch;

    name = document.meta.mainProgram;
    tag = document.version;

    copyToRoot = [
      document
      pkgs.cacert
    ];

    config = {
      entrypoint = ["/bin/${document.meta.mainProgram}"];
      cmd = ["serve" "--listen" "0.0.0.0:8080"];
      env = ["SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt"];

      exposedPorts = {
        "8080/tcp" = {};
      };

      labels = {
        "org.opencontainers.image.title" = document.meta.mainProgram;
        "org.opencontainers.image.description" = document.meta.description;
        "org.opencontainers.image.version" = document.version;
        "org.opencontainers.image.source" = document.meta.homepage;
      };
    };
  }
