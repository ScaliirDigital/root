# Scaliir Digital · Open Source

We build software that closes the gap between what technology can do and what
people actually need — products that change something, not just ship.

This is the public monorepo for that work.

---

## 📦 Packages

| Package | What it does |
| --- | --- |
| [`@scale.digital/astro-bun`](./packages/astro-bun) | Runs Astro sites on the Bun runtime. ISR/SWR caching, Brotli/gzip pre-compression. |

More on the way — deployment tooling and Tauri plugins are next.

---

## 🚀 Getting started

Everything runs from a reproducible environment. One command, same toolchain
on every machine:

```sh
nix develop
```

Requires [Nix](https://nixos.org/) with flakes enabled.

---

## 📮 Releases

Versioning and changelogs are driven by
[Changesets](https://github.com/changesets/changesets). Packages go to npm via
OIDC trusted publishing with [Sigstore](https://www.sigstore.dev/) provenance —
so every release can be traced back to the commit and workflow that produced it.

---

## 🧭 Direction

The long-term goal is a fully hermetic repository where a new product can go
from issue to release without manual plumbing in between.

**Automation handles the mechanics, not the judgement.** Every change goes
through human review, held to the same bar as hand-written work: small, focused,
well-argued pull requests. Volume is not the point — quality is.

**Where we are:**

| | |
| --- | --- |
| ✅ **Hermetic environments** | Nix flakes — identical toolchain locally and in CI |
| ✅ **Automated releases** | Changesets drives versioning and changelogs |
| ✅ **Verifiable publishing** | OIDC trusted publishing with Sigstore provenance |
| 🚧 **Shared build tooling** | Across TypeScript and Rust |
| 🚧 **Issue-to-release automation** | The piece that ties it together |

---

## 📄 License

[BSD-3-Clause](./LICENSE) — use it, ship it.

---

<sub>Built in Frankfurt by [Scaliir Digital UG (haftungsbeschränkt)](https://github.com/ScaliirDigital) ·
[contact@scaliir.digital](mailto:contact@scaliir.digital)</sub>
