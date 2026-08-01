# Python Example

This package demonstrates Python targets with Bazel, `uv`, and external PyPI dependencies.

## Targets

```sh
bazel run //python:hello
bazel test //python:hello_test
```

## Dependency model

Python dependencies are managed with `uv`.

The flow is:

```txt
pyproject.toml
→ uv.lock
→ requirements_lock.txt
→ pip.parse(...)
→ requirement("package")
```

Bazel imports Python dependencies through:

```txt
python/deps.MODULE.bazel
```

Example:

```starlark
pip = use_extension("@rules_python//python/extensions:pip.bzl", "pip")
pip.parse(
    hub_name = "python_deps",
    python_version = "3.13",
    requirements_lock = "//python:requirements_lock.txt",
)
use_repo(pip, "python_deps")
```

## Adding a new Python dependency

### 1. Add the dependency with uv

From the repository root:

```sh
uv add rich
```

This updates:

```txt
python/pyproject.toml
python/uv.lock
```

### 2. Export the Bazel requirements lockfile

```sh
uv export \
  --format requirements-txt \
  --output-file requirements_lock.txt \
  --no-dev \
  --hashes
```

This updates:

```txt
python/requirements_lock.txt
```

### 3. Add the dependency to the Bazel target

Edit:

```txt
python/BUILD.bazel
```

Example:

```starlark
load("@python_deps//:requirements.bzl", "requirement")

py_library(
    name = "lib",
    srcs = ["src/main.py"],
    imports = ["src"],
    deps = [
        requirement("rich"),
    ],
)
```

Adding a package to `pyproject.toml` makes it available to Bazel through `pip.parse(...)`.

Each Bazel target still needs to explicitly declare the packages it imports.

## Current BUILD pattern

```starlark
load("@python_deps//:requirements.bzl", "requirement")
load("@rules_python//python:defs.bzl", "py_binary", "py_library", "py_test")

py_library(
    name = "lib",
    srcs = ["src/main.py"],
    imports = ["src"],
    visibility = ["//visibility:public"],
    deps = [
        requirement("rich"),
    ],
)

py_binary(
    name = "hello",
    srcs = ["src/main.py"],
    main = "src/main.py",
    deps = [":lib"],
)

py_test(
    name = "hello_test",
    srcs = ["src/main_test.py"],
    main = "src/main_test.py",
    deps = [":lib"],
)
```

## Build validation

```sh
bazel build //python:all
bazel test //python:all
```
