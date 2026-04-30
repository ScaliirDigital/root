# Rust Example

This package demonstrates Rust targets with Bazel, Cargo metadata, and external crate dependencies.

## Targets

```sh
bazel run //rust:hello
bazel test //rust:lib_test
```

## Dependency model

Rust dependencies are managed through:

```txt
rust/Cargo.toml
rust/Cargo.lock
```

Bazel imports those dependencies through `crate.from_cargo(...)` in `MODULE.bazel`.

The flow is:

```txt
Cargo.toml
→ Cargo.lock
→ crate.from_cargo(...)
→ @crates//:<crate>
```

Example:

```starlark
crate = use_extension("@rules_rs//rs:extensions.bzl", "crate")
crate.from_cargo(
    name = "crates",
    cargo_lock = "//rust:Cargo.lock",
    cargo_toml = "//rust:Cargo.toml",
    platform_triples = [
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    ],
)
use_repo(crate, "crates")
```

## Adding a new Rust dependency

### 1. Add the crate

From the repository root:

```sh
cargo add uuid --features v4
cd ..
```

This updates:

```txt
rust/Cargo.toml
rust/Cargo.lock
```

If `cargo add` is not available, edit `Cargo.toml` manually and then run:

```sh
cargo generate-lockfile
```

or:

```sh
cargo update
```

### 2. Add the dependency to the Bazel target

Edit:

```txt
rust/BUILD.bazel
```

Example:

```starlark
rust_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    crate_name = "basic_bazel_example_rust",
    visibility = ["//visibility:public"],
    deps = [
        "@crates//:uuid",
    ],
)
```

## Current BUILD pattern

```starlark
load("@rules_rs//rs:rust_binary.bzl", "rust_binary")
load("@rules_rs//rs:rust_library.bzl", "rust_library")
load("@rules_rs//rs:rust_test.bzl", "rust_test")

rust_library(
    name = "lib",
    srcs = ["src/lib.rs"],
    crate_name = "basic_bazel_example_rust",
    visibility = ["//visibility:public"],
    deps = ["@crates//:uuid"],
)

rust_binary(
    name = "hello",
    srcs = ["src/main.rs"],
    deps = [":lib"],
)

rust_test(
    name = "lib_test",
    crate = ":lib",
)
```

## Formatting and linting

Typical local commands:

```sh
cargo fmt
cargo clippy
```

Bazel remains the main build entrypoint:

```sh
bazel build //rust:all
bazel test //rust:all
```

## Build validation

```sh
bazel build //rust:all
bazel test //rust:all
```
