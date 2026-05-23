# @scale.digital/astro-bun

A native Bun adapter for Astro 6 — runs SSR, hybrid, and static sites directly on `Bun.serve`.

> **Status: pre-1.0**
>
> This adapter is in active development and is used in production by Scaliir Digital.
>
> Currently validated with Astro 6.3.7 using the Qwik Astro integration:
>
> - `astro` 6.3.7
> - `@qwik.dev/astro` 1.0.1
> - `@qwik.dev/core` 2.0.0-beta.35
>
> Other Astro setups may work, but have not been validated yet.
> The public API may change in minor versions until 1.0.

## Features

- **Bun-native startup path.** Runs directly on Bun via `Bun.serve`.
- **Zero runtime dependencies.** Only adapter code and Astro runtime helpers.
- **Compression.** Build-time Brotli/gzip for static assets.
- **First-class caching.** Persistent Stale-While-Revalidate (SWR) cache with request coalescing for SSR routes.

## Installation

```sh
bun add @scale.digital/astro-bun
```

Requirements:

- Astro `^6.3.0`
- Bun `>=1.3.0`

Astro itself uses Node tooling at build time. Node `>=22.12` is required for `astro build`. The runtime is Bun-only.

## Quick start

```ts
// astro.config.ts
import { defineConfig } from 'astro/config';
import bun from '@scale.digital/astro-bun';

export default defineConfig({
  output: 'server',
  adapter: bun(),
});
```

Build and run:

```sh
bun run build
bun run ./dist/server/index.mjs
```

The server will start on `PORT` (default `4321`) and `HOST` (default `0.0.0.0`).

## Configuration

All options are optional.

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `cache` | `boolean \| CacheOptions` | `false` | Enable the Stale-While-Revalidate response cache. Pass `true` for defaults, or an object to customize. |
| `compress` | `boolean` | `true` | Generate Brotli and gzip versions of static assets at build time. |
| `staticCacheControl` | `string` | `public, max-age=86400, must-revalidate` | `Cache-Control` header for non-hashed static assets. |

### Compression

When `compress: true` (the default), the adapter generates `.br` and `.gz` versions of compressible static assets (`.css`, `.js`, `.html`, `.svg`, `.json`, `.xml`, `.txt`, `.mjs`) during `astro build`. At request time, the server reads the `Accept-Encoding` header and serves the matching pre-compressed file when available, falling back to the original.

Set `compress: false` to skip this step if you handle compression elsewhere (e.g. a custom build pipeline) or if the additional build time is a concern.

### `CacheOptions`

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `maxByteSize` | `number` | `104857600` (100 MB) | Maximum on-disk size of the cache. Oldest entries are evicted when the limit is reached. |
| `cacheDir` | `string` | OS temp dir | Directory where cache files are stored. |
| `warmOnInit` | `boolean` | `true` | Read the cache index on server start. Set `false` for faster startup at the cost of an empty cache on first request. |

### Example

```ts
import { defineConfig } from 'astro/config';
import bun from '@scale.digital/astro-bun';

export default defineConfig({
  output: 'server',
  adapter: bun({
    cache: {
      maxByteSize: 500 * 1024 * 1024,
      cacheDir: '/var/cache/astro-bun',
      warmOnInit: false,
    },
    staticCacheControl: 'public, max-age=3600, must-revalidate',
  }),
});
```

## Caching

The cache is opt-in and applies only to SSR responses that explicitly opt in via `Cache-Control: s-maxage=N`. Static assets are served from disk with their own `Cache-Control` (see `staticCacheControl`).

### How it works

1. A request hits an SSR route.
2. If a cached response exists and is fresh, it is served immediately (`x-astro-cache: HIT`).
3. If a cached response exists but is stale (past `s-maxage`, within `stale-while-revalidate`), the stale response is served (`x-astro-cache: STALE`) and a background revalidation is triggered.
4. If no cached response exists, the route is rendered, stored, and served (`x-astro-cache: MISS`).
5. Concurrent misses for the same path are coalesced — only one render runs.

### Response header: `x-astro-cache`

| Value | Meaning |
| --- | --- |
| `HIT` | Fresh cached response served. |
| `STALE` | Stale cached response served; background revalidation in progress. |
| `MISS` | No cache entry; response was rendered and stored. |
| `BYPASS` | Route opted out of caching (no `s-maxage` directive). |
| `STATIC` | Static asset served from disk; cache not applicable. |

### Opting routes into the cache

Set `Cache-Control` in your Astro route or middleware:

```ts
// src/pages/blog/[slug].astro
Astro.response.headers.set(
  'Cache-Control',
  'public, s-maxage=60, stale-while-revalidate=600'
);
```

Routes without `s-maxage` are passed through and tagged `BYPASS`.

## Cache invalidation

Two functions are exposed for on-demand invalidation:

```ts
import { expireAll, expirePath } from '@scale.digital/astro-bun/cache';

// Invalidate a single path
await expirePath('/blog/my-post');

// Invalidate the entire cache
await expireAll();
```

Both functions are safe to call when the cache is disabled — they become no-ops.

Typical use: call `expirePath` from a webhook after content updates in a CMS.

## Environment variables

| Variable | Default | Description |
| --- | --- | --- |
| `PORT` | `4321` | Port the server listens on. |
| `HOST` | `0.0.0.0` | Hostname or IP the server binds to. |

## Acknowledgements

This adapter is based on [`@wyattjoh/astro-bun-adapter`](https://github.com/wyattjoh/astro-bun-adapter) by Wyatt Johnson. It preserves the goal of running Astro on Bun and follows the upstream ISR caching model, including LRU caching, disk persistence, stale-while-revalidate behavior, image-endpoint cache overrides, and build namespacing.

This fork is a Bun-native rewrite with no external runtime dependencies and adds build-time Brotli and gzip compression.

The upstream adapter was itself inspired by [`astro-bun-adapter`](https://github.com/ido-pluto/astro-bun-adapter) by Ido Pluto.

## License

[BSD-3-Clause](./LICENSE). Portions derived from `@wyattjoh/astro-bun-adapter` remain under their original MIT license; see [LICENSE](./LICENSE) for the full text.
