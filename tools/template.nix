# tools/template.nix
{
  bazel = {
    path = ../examples/bazel;
    description = "Polyglot Bazel starter with C++, Go, Python, Rust, Protobuf, external dependencies, and Nix.";
    welcomeText = ''
      # Polyglot Bazel starter

      Next steps:

      ```sh
      nix develop
      bazel query //...
      bazel build ...
      bazel test ...
      ```

      Optional Git setup:

      ```sh
      git init
      git add .
      git commit -m "Initial commit"
      ```
    '';
  };
}
