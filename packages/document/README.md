# document

![Version](https://img.shields.io/badge/version-0.1.0-blue)
![Coverage](https://img.shields.io/badge/coverage-100%25-brightgreen)
![Clippy](https://img.shields.io/badge/clippy-pedantic-brightgreen)
![MSRV](https://img.shields.io/badge/rust-1.92%2B-orange)
![License](https://img.shields.io/badge/license-BSD--3--Clause-blue)

**Documents you can prove.**

A reproducible document production toolchain built around an embedded Typst engine.

```text
Template workflow                         Document production

template init
     ↓
template check
     ↓                               ┌─────────────────┐
template publish ── publish once ──→ │ document server │
                                     │                 │
JSON ─────────────── render many ──→ │                 │ ── produce many → PDF
JSON ──────────────────────────────→ │                 │ ────────────────→ PDF
JSON ──────────────────────────────→ │                 │ ────────────────→ PDF
                                     └─────────────────┘
```

**Build and validate a template once, publish it to the server, then turn JSON into documents at scale.**

Templates are checked against local data before publishing. Published versions are immutable, every render is pinned to an explicit template version, fonts are embedded, and timestamps are explicit.

With the same template bundle, data, render profile, and renderer build, `document` produces the same PDF bytes — locally, in CI, and in production.

One static binary. No Chromium, no system fonts, no network.

Built for documents that must remain explainable years later: invoices, records, notices, reports, and other long-lived artifacts.

## Quick start

```console
cargo install document
document template init ./hello
```

That writes a starter template — an entrypoint and its local data — and leaves you
a directory to edit. Check it before it goes anywhere:

```console
document template check ./hello
```

```
PASSED  ./hello 3d0105655747 (1 files, 10515 bytes, 0 warnings)
```

Exit code `0` when it renders, `1` when it does not, with the compiler
diagnostics on stderr. That is the CI gate.

Then start the server and publish it:

```console
document serve --listen 127.0.0.1:8080
document template publish hello ./hello
```

```
PUBLISHED  hello v1 (3d0105655747)
```

Publishing compiles the template against its local data first — no data, no
publish, otherwise "it compiles" is the only guarantee you have. A published
version is never overwritten; republishing identical content returns the
existing version instead of minting a new one.

Rendering it takes data only:

```console
curl -X POST localhost:8080/templates/hello/1/render \
  -H 'content-type: application/json' \
  -d '{"data": {"title": "Hello", "message": "An example message."}}' \
  -o hello.pdf
```

For previews there is `POST /render`, which takes template and data together
and stores nothing.

## Templates

A template is a directory. Two of its files are JSON and they do different
jobs, which is worth getting right once:

```text
invoice/
  main.typ              the entrypoint
  brand.typ             partials and assets, imported by path
  logo.svg
  fixture.json          data that belongs to the template: your company,
                        your bank details. Published with the bundle and
                        frozen into every version made from it.
  __data/request.json   an example request, for editing locally. Excluded
                        from the bundle — it stands in for what a caller sends.
```

Everything that varies per document travels in the request. Everything that
does not lives in the template and cannot drift between renders.

Data arrives as a value, never as syntax:

```typst
#let request = json("/__data/request.json")
#let data = request.data
```

That removes the entire escaping bug class — a customer name containing `&`
cannot break the document, because it never becomes part of the source.
Missing or malformed data is a compile error, so a mistake fails loudly instead
of rendering an empty field.

At render time the server injects that same path, so the template reads the
same file locally and in production. The path resolves against the bundle root,
which means the entrypoint has to sit at the top of the bundle.

`entrypoint` defaults to `main.typ` and only needs naming for file sets with
several roots. Each uploaded part's filename becomes its path inside the
bundle, so `#import "brand.typ"` resolves and subdirectories survive.

A template reaches nothing outside its own file set: no file system resolver,
no package resolver, no network. Not providing them *is* the sandbox. Compiles
run in pooled worker processes, because a runaway Typst loop cannot be
cancelled from inside its own thread.

Two templates ship in the binary. `--template minimal` is the default and is
two files; `--template invoice` is a complete Factur-X invoice:

```console
document template init ./invoice --template invoice
```

## Invoices

`document` generates the Factur-X XML itself. You send invoice data and name a
profile; the server validates the data against that profile, builds the XML,
embeds it, and writes the XMP metadata that makes the result a ZUGFeRD invoice
rather than a PDF with a file stuck to it.

```console
document template init ./invoice --template invoice
document template publish invoice ./invoice

jq '{document: {type: "invoice", profile: "en16931", lang: "de"}, data: .data}' \
  ./invoice/__data/request.json > request.json

curl -fsS -X POST localhost:8080/templates/invoice/1/render \
  -H 'content-type: application/json' \
  -d @request.json \
  -o invoice.pdf
```

The request carries a `document` block next to the data:

```json
{
  "document": { "type": "invoice", "profile": "en16931", "lang": "de" },
  "data": { "number": "2026-0042", "issued": "2026-08-13" }
}
```

Seller and payment details are not in the request — they come from the
template's `fixture.json`, so they are fixed for every invoice made from that
version. An invoice is always rendered as PDF/A-3b with a fixed timestamp —
that is the container attachments require, so you never have to ask for it.

`minimum`, `basic` and `en16931` are implemented and each validates against
Mustangproject. Note that MINIMUM and BASIC WL are not valid invoices for
German VAT purposes and do not satisfy the e-invoicing mandate (BMF, October
2024) — they count as booking aids. Use `basic` at minimum, `en16931` for the
full data set.

## CLI

One binary, whether you are editing a template or running the service.

| | |
|---|---|
| `document template init <dir> [--template minimal\|invoice]` | write a built-in template |
| `document template check <dir>` | compile against the data, exit 0 or 1 |
| `document template hash <dir>` | the bundle's content address |
| `document template publish <id> <dir>` | upload it as a new version |
| `document template list` | what is published |
| `document template get <id> <version>` | the manifest of one version |
| `document compile <entrypoint>` | render locally, no server |
| `document serve` | the HTTP service |

`hash` is what makes the claim at the top checkable. It prints the same address
the server derives at publish time, so a bundle on disk can be matched against
a published version without uploading it:

```console
test "$(document template hash ./hello)" \
   = "$(document template get hello 1 | jq -r .content_hash)"
```

`DOCUMENT_TOKEN` and `--server` apply to every command that talks to a server.

## Configuration

```dotenv
DOCUMENT_TOKEN=
DOCUMENT_DATA_DIR=./data

DOCUMENT_S3_BUCKET=documents
DOCUMENT_S3_REGION=eu-central-1
DOCUMENT_S3_ENDPOINT=https://s3.example.com
DOCUMENT_S3_ACCESS_KEY_ID=
DOCUMENT_S3_SECRET_ACCESS_KEY=
```

Storage holds published templates and archived renders. A bucket wins when
configured, then a data directory, otherwise memory — and memory means nothing
survives a restart, which the server says out loud on startup. A bucket that
*is* configured but unreachable is a hard failure at startup rather than a
surprise on the first archival render. Anything unset falls back to the
standard `AWS_*` variables.

Built on `object_store`, so S3, R2, MinIO, Garage and Hetzner should work.
Version numbers are claimed with a conditional put, so the backend has to
support it — `document` has been exercised against RustFS.

## Security

With `DOCUMENT_TOKEN` set, every endpoint except the health probes requires
`Authorization: Bearer <token>`. Unset, the service is open — fine behind a
private network or a proxy that authenticates, a footgun anywhere else. The
token is hashed at startup and compared as a hash, so its plaintext does not
outlive the boot and the comparison leaks no timing.

Templates are sandboxed by omission: the compilation environment provides no
file system resolver, no package resolver and no network, so a template reaches
nothing beyond the files handed to it. Absolute paths and `..` are rejected
before compilation, and compiles run in worker processes so a runaway loop
cannot take the service with it. A memory limit is **not** enforced in-process
— `setrlimit` needs `unsafe`, which this crate forbids. Set it in your
deployment.

`fixture.json` is published with the bundle and recorded in the manifest, and a
published version is never deleted. Put your own issuer details there, not a
real customer record.

## Performance

Release build, 8 physical cores / 16 threads, WSL2. Renders of the `invoice`
template — layout only, without the Factur-X path, which adds XML generation,
embedding and the XMP patch on top. Storage was memory-backed.
Two runs per level after a 10 s warm-up, averaged.

| Concurrency | req/s | avg | p50 | p95 | p99 |
| --- | --- | --- | --- | --- | --- |
| 1 | 437 | 2.3 ms | 2.1 ms | 3.3 ms | 4.8 ms |
| 4 | 1167 | 3.4 ms | 2.6 ms | 6.3 ms | 11.0 ms |
| 8 | 1367 | 5.8 ms | 4.5 ms | 12.0 ms | 22.8 ms |
| 16 | 1592 | 10.0 ms | 9.0 ms | 18.7 ms | 27.3 ms |
| 32 | 1534 | 20.9 ms | 18.9 ms | 33.4 ms | 50.2 ms |

Throughput peaks at the thread count and falls off past it, where the queue
costs more than the parallelism buys. Latency grows, nothing fails: no errors
across any run.

## Status

**Working**

- Both render paths: published templates by id and version, and ad-hoc renders that store nothing
- Byte-identical output for the same bundle, data, profile and renderer build
- PDF/A-3b, with fonts embedded and the timestamp fixed
- Factur-X generated from the invoice data — MINIMUM, BASIC and EN 16931, each validated against Mustangproject
- Versioned, immutable templates on disk or S3, addressed by content hash
- Compiles in pooled worker processes, with wall-clock and size limits
- One binary for both: CLI and HTTP service

**Next**

- XRechnung for public-sector invoices
- `document preview`: a local server that re-renders on save
- OpenTelemetry: render rate, latencies, cache hits, per-stage timings
- named output profiles instead of the `archival` boolean
- Typst packages, vendored into the bundle at publish time

A renderer alone does not give you gapless numbering, transaction isolation,
tax decisions, or signatures. Those belong to the system that owns the
transaction.

## Development

```console
cargo llvm-cov clean --workspace
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo llvm-cov \
  --fail-under-lines 100 \
  --fail-under-functions 100 \
  --fail-under-regions 100
```

Typst is pre-1.0 and its Rust API changes between minor versions. The engine
implements Typst's compilation environment directly, so an upgrade is a local
change — but check it: the `World` trait itself, the path and `FileId` types,
`PdfOptions`, the attachment API, that `output_is_deterministic` stays green,
and that a ZUGFeRD render still validates.

## License

BSD 3-Clause. See [LICENSE](LICENSE).

The binary embeds Roboto, licensed under the SIL Open Font License 1.1
([assets/fonts/OFL.txt](assets/fonts/OFL.txt)). Third-party crate licenses are
in [NOTICE](NOTICE).
