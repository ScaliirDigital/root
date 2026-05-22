_: {
  # Nix
  alejandra.enable = true;
  deadnix.enable = true;
  statix.enable = true;

  # Generic file checks
  check-yaml.enable = true;
  check-toml.enable = true;

  check-json = {
    enable = true;
    excludes = [
      "tsconfig\\.json$"
      "tsconfig\\..*\\.json$"
    ];
  };

  check-merge-conflicts.enable = true;
  end-of-file-fixer.enable = true;
  trim-trailing-whitespace.enable = true;
  check-added-large-files = {
    enable = true;
    args = ["--maxkb=1000"];
  };

  # Commit message
  convco.enable = true;

  # Web
  biome = {
    enable = true;
    excludes = ["^\\.changeset/.*"];
    types_or = [
      "javascript"
      "jsx"
      "ts"
      "tsx"
      "json"
      "vue"
      "astro"
    ];
  };
}
