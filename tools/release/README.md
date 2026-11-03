<!-- tools/release/README.md -->
<div align="center">

# release

**Release planning, creating and publishing for multi-language monolithic repositories**

![Status: work in progress](https://img.shields.io/badge/status-work%20in%20progress-orange)
![License: BSD-3-Clause](https://img.shields.io/badge/license-BSD--3--Clause-blue)
![Bun 1.4](https://img.shields.io/badge/bun-1.4-black?logo=bun)

</div>

> [!WARNING]
> This tool is a work in progress and built for this repository. Configuration
> paths and some defaults are still hard-coded, so it is not yet usable in other
> repositories without changes.

## About

`release` is the release system we use to ship our packages in this repo.
It is language-agnostic: packages written in Rust and TypeScript go through the
same flow today, and new languages only need a manifest and lockfile adapter.

It derives versions from Conventional Commits, writes the changelog, opens one
release pull request per package and creates the tag and GitHub release once
that pull request is merged. Changelog, release notes and pull request
description come from the same source, so they never drift apart.

Publishing to registries (npm, crates.io, container registries) is not done by
the tool itself. It reports what was released, and CI takes it from there.

## Features

- Version bumps from Conventional Commits, scoped per package
- Changelog sections with contributors and links to commits
- One release pull request per package, created and updated idempotently
- Release commits created through the GitHub API, signed and marked as
  verified for the GitHub App that runs the workflow
- Atomic branch updates: a release branch is only moved from the commit the
  plan expects, and the tree built on GitHub is checked against the local one
- Refuses to continue on manually changed or unmanaged release branches
- Tags and GitHub releases that can be retried safely after partial failures
- Language-agnostic core, currently configured for Rust and TypeScript

## How it works

1. **Plan**: For every package, find the latest release tag, collect scoped
   commits since then and compute the next version. Nothing is written.
2. **Create**: Build the release commit (manifest, lockfile, changelog), push
   it to `release/<package>` and open or update the release pull request.
3. **Prepare**: After the release pull request is merged, tag the merge commit
   and report the release to CI.
4. **Publish**: CI publishes the artifacts, then the tool creates the GitHub
   release from the changelog section.

### Versioning

| Commits since last release | Before 1.0 | From 1.0 |
| :--- | :--- | :--- |
| `fix` only | patch | patch |
| `feat` | patch | minor |
| Breaking change | minor | major |
| Other types only | no release | no release |

Only commits whose scope matches the package name count, for example
`fix(document): …` for the `document` package.

## Usage

The tool is packaged with Nix and runs only through it. The package provides
Bun, Git, the GitHub CLI and Cargo, so nothing else needs to be installed.
Commands that talk to GitHub need `GH_REPO=owner/name`.

```sh
nix run .#release -- plan               # show planned releases as JSON
nix run .#release -- plan --markdown    # preview the changelog sections
nix run .#release -- create             # build release branches locally
nix run .#release -- create --github    # push branches and update release PRs
nix run .#release -- publish --prepare  # tag merged releases
nix run .#release -- publish            # create GitHub releases
```

Add `--github` to `plan` to include authors and links from GitHub.

## Configuration

Packages are listed in `tools/release/packages.toml`:

```toml
[packages.document]
language = "rust"
targets = ["cargo", "binary"]
containers = ["ghcr", "dockerhub"]
```

| Field | Values |
| :--- | :--- |
| `language` | `rust`, `typescript` |
| `targets` | `npm`, `cargo`, `binary` |
| `containers` | `ghcr`, `dockerhub` |

Tag prefix, manifest and changelog location follow from the package name and
language. Every package needs a reachable release tag such as
`document-v0.1.0` before its first release through this tool.

## Limitations

- Only GitHub is supported, and the base branch is always `main`
- The configuration path and the Docker Hub namespace are hard-coded
- No prereleases
- Commits are assigned to packages by scope only, not by changed paths
- A package without a matching release tag stops the planning for all packages

## Development

The tests need the same tools as the package (Bun, Git and Cargo). Run them
from the repository's Nix development shell:

```sh
bun install
bun test --parallel
```

The tests run against real Git repositories in temporary directories. The
GitHub API is replaced by a local fake that reproduces the endpoints the tool
uses.

## Contributing

Issues and pull requests are welcome. Please use Conventional Commits with the
`release` scope, for example `fix(release): …`, and keep the test suite green.

## License

BSD-3-Clause, see the [license](../../LICENSE) at the repository root.
