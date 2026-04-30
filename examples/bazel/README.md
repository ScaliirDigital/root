# Polyglot Bazel Starter

A **modern** polyglot build template with Bazel, Nix, and Protobuf for **serious engineering projects**.

## Features

| Feature | Status |
|---|---:|
| C++ | ✅ |
| Go | ✅ |
| Python | ✅ |
| Rust | ✅ |
| Protobuf | ✅ |
| External native dependencies | ✅ |
| External package-manager dependencies | ✅ |
| External Proto imports | ✅ |
| Multi-language Proto codegen | ✅ |
| Bazel 9 | ✅ |
| Bzlmod | ✅ |
| Template export via Nix flake | ✅ |

## Quick start

Enter the Nix development shell:

```sh
nix develop
```

Build all targets:

```sh
bazel build ...
```

Run all tests:

```sh
bazel test ...
```

List all targets:

```sh
bazel query //... | sort
```

List all tests:

```sh
bazel query 'tests(//...)' | sort
```

## What this template shows

This repository is designed as a practical starter for teams that need a reproducible multi-language workspace.

It includes:

- C++ binaries, tests, and native external dependencies
- Go binaries, tests, `go.mod`, and Gazelle `go_deps`
- Python binaries, tests, `uv`, and `rules_python` `pip.parse`
- Rust binaries, tests, Cargo metadata, and `crate.from_cargo`
- Shared Protobuf schemas
- External Proto imports from `googleapis`
- Language-specific Proto code generation for C++, Go, Python, and Rust
- A Nix development shell for consistent local tooling

## Repository structure

```txt
.
├── MODULE.bazel
├── flake.nix
├── third_party/
│   ├── BUILD.bazel
│   ├── deps.MODULE.bazel
│   ├── googleapis.BUILD.bazel
│   ├── usockets.BUILD.bazel
│   └── uwebsockets.BUILD.bazel
├── proto/
│   ├── BUILD.bazel
│   └── hello.proto
├── cpp/
│   ├── BUILD.bazel
│   ├── README.md
│   └── src/
├── go/
│   ├── BUILD.bazel
│   ├── README.md
│   ├── deps.MODULE.bazel
│   ├── go.mod
│   ├── go.sum
│   └── src/
├── python/
│   ├── BUILD.bazel
│   ├── README.md
│   ├── deps.MODULE.bazel
│   ├── pyproject.toml
│   ├── requirements_lock.txt
│   ├── uv.lock
│   └── src/
└── rust/
    ├── BUILD.bazel
    ├── README.md
    ├── Cargo.toml
    ├── Cargo.lock
    └── src/
```



## Language targets

### C++

```sh
bazel run //cpp:hello
bazel test //cpp:hello_test
bazel run //cpp:uwebsockets_server
```

### Go

```sh
bazel run //go:hello
bazel test //go:lib_test
```

### Python

```sh
bazel run //python:hello
bazel test //python:hello_test
```

### Rust

```sh
bazel run //rust:hello
bazel test //rust:lib_test
```

### Protobuf

```sh
bazel build //proto:all
```

Generated Proto targets include:

```txt
//proto:hello_cc_proto
//proto:hello_go_proto
//proto:hello_python_proto
//proto:hello_rust_proto
```

## Shared Protobuf contract

The shared schema lives in:

```txt
proto/hello.proto
```

It is compiled for:

- C++
- Go
- Python
- Rust

The schema also demonstrates external Proto imports:

```proto
import "google/type/date.proto";
```

The external Proto dependency is provided through:

```txt
third_party/googleapis.BUILD.bazel
third_party/deps.MODULE.bazel
```

## Dependency workflows

Each language uses the dependency workflow that is idiomatic for its ecosystem.

| Language | Source of truth | Bazel integration |
|---|---|---|
| C++ | `third_party/deps.MODULE.bazel` + `third_party/*.BUILD.bazel` | `http_archive` + custom BUILD wrappers |
| Go | `go/go.mod` / `go/go.sum` | Gazelle `go_deps.from_file(...)` |
| Python | `python/pyproject.toml` / `uv.lock` / `requirements_lock.txt` | `pip.parse(...)` + `requirement(...)` |
| Rust | `rust/Cargo.toml` / `rust/Cargo.lock` | `crate.from_cargo(...)` |
| Proto | `proto/*.proto` + external Proto archives | `proto_library` + language-specific codegen |

See the language-specific READMEs for details:

- [C++ dependency workflow](./cpp/README.md)
- [Go dependency workflow](./go/README.md)
- [Python dependency workflow](./python/README.md)
- [Rust dependency workflow](./rust/README.md)

## Nix flake template

This example can be exported as a Nix flake template.

From another directory:

```sh
nix flake init -t github:ScaliirDigital/root#bazel
nix develop
bazel build ...
bazel test ...
```

For local development of the template:

```sh
nix flake init -t /path/to/root#bazel
nix develop
bazel build ...
bazel test ...
```

## Roadmap

The next layers for this template are:

1. Polyglot quality workflow
   - buildifier
   - clang-format / clang-tidy
   - gofmt / go vet
   - ruff
   - rustfmt / clippy
   - optional Proto formatting/linting

2. Polyglot transport layer
   - gRPC
   - ZeroMQ
   - shared Protobuf payloads over multiple transports

3. External local build cache
   - NativeLink
   - local Bazel-compatible remote cache
   - optional CI cache configuration

Future benchmark layer:

- gRPC vs ZeroMQ
- shared benchmark Proto schema
- generated benchmark results
- CI-generated benchmark artifacts for changes to examples

## Philosophy

This template keeps dependency ownership explicit.

Package managers remain the source of truth for each language, while Bazel provides the unified build, test, and code generation entrypoint.

The goal is not to hide the underlying ecosystems, but to compose them into one reproducible workspace.
