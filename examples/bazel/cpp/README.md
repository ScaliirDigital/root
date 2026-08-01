# C++ Example

This package demonstrates C++ targets with Bazel, including a normal binary/test setup and an external C/C++ dependency through `http_archive`.

## Targets

```sh
bazel run //cpp:hello
bazel test //cpp:hello_test
bazel run //cpp:uwebsockets_server
```

The `uwebsockets_server` target uses:

- `uWebSockets`
- `uSockets`

Both are provided through external Bazel archives in:

```txt
third_party/deps.MODULE.bazel
```

The BUILD wrappers are defined in:

```txt
third_party/usockets.BUILD.bazel
third_party/uwebsockets.BUILD.bazel
```

## Adding a new C / C++ dependency

### 1. Add the archive

Edit:

```txt
third_party/deps.MODULE.bazel
```

Example:

```starlark
http_archive(
    name = "some_c_library",
    build_file = "//third_party:some_c_library.BUILD.bazel",
    integrity = "...",
    strip_prefix = "some-c-library-1.0.0",
    urls = ["https://example.com/some-c-library-1.0.0.tar.gz"],
)
```

### 2. Add a BUILD wrapper

Create:

```txt
third_party/some_c_library.BUILD.bazel
```

Example:

```starlark
load("@rules_cc//cc:defs.bzl", "cc_library")

package(default_visibility = ["//visibility:public"])

cc_library(
    name = "some_c_library",
    srcs = glob(["src/**/*.c"]),
    hdrs = glob(["include/**/*.h"]),
    includes = ["include"],
)
```

### 3. Use the dependency

Edit:

```txt
cpp/BUILD.bazel
```

Example:

```starlark
cc_binary(
    name = "app",
    srcs = ["src/main.cc"],
    deps = [
        "@some_c_library//:some_c_library",
    ],
)
```

## Formatting and linting

This package includes:

```txt
cpp/.clang-format
cpp/.clang-tidy
```

Typical commands:

```sh
clang-format -i cpp/src/*.cc
clang-tidy cpp/src/*.cc
```

## Build validation

```sh
bazel build //cpp:all
bazel test //cpp:all
```
