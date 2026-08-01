# Go Example

This package demonstrates Go targets with Bazel, including a local library, a binary, tests, and external Go module dependencies.

## Targets

```sh
bazel run //go:hello
bazel test //go:lib_test
```

## Dependency model

Go dependencies are managed through:

```txt
go/go.mod
go/go.sum
```

Bazel imports those dependencies through Gazelle's `go_deps` extension in:

```txt
go/deps.MODULE.bazel
```

Example:

```starlark
go_deps = use_extension("@gazelle//:extensions.bzl", "go_deps")
go_deps.from_file(
    go_mod = "//go:go.mod",
)
use_repo(
    go_deps,
    "com_github_google_uuid",
)
```

## Adding a new Go dependency

### 1. Add the module

From the repository root:

```sh
go get github.com/example/package
go mod tidy
```

### 2. Update Bazel module dependencies

From the repository root:

```sh
bazel mod tidy
```

This updates `use_repo(...)` entries for `go_deps` when needed.

### 3. Add the dependency to the Bazel target

Edit:

```txt
go/BUILD.bazel
```

Example:

```starlark
go_binary(
    name = "hello",
    srcs = ["src/main/main.go"],
    deps = [
        ":lib",
        "@com_github_example_package//:package",
    ],
)
```

The exact Bazel label depends on the generated repository name. Use `bazel mod tidy` and `bazel query` if needed.

## Dependency pinning for generated code

If a Go dependency is only imported by generated code, `go mod tidy` may remove it.

For example, generated Protobuf code may import:

```go
google.golang.org/genproto/googleapis/type/date
```

To keep this dependency in `go.mod`, add a small pin file:

```go
// go/src/proto_deps/deps.go
package proto_deps

import (
	_ "google.golang.org/genproto/googleapis/type/date"
)
```

Then run:

```sh
go mod tidy
bazel mod tidy
```

## Build validation

```sh
bazel build //go:all
bazel test //go:all
```
