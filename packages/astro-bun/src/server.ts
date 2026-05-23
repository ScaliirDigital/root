import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import options from 'virtual:@scale.digital/astro-bun/config';
import { createApp } from 'astro/app/entrypoint';
import { setGetEnv } from 'astro/env/setup';
import { registerCache } from './cache';
import { buildImageCacheKey } from './cache/images.ts';
import { createCacheServer } from './cache/serve.ts';
import { resolveCompressed } from './compression/serve.ts';
import { findAdapterDir } from './paths.ts';
import type { AdapterOptions, CacheServer, ManifestEntry } from './types.ts';

const CACHE_HEADER = 'x-astro-cache';

setGetEnv((key) => process.env[key]);

// --- Auto-start ---
const config = options as AdapterOptions;

// Astro 6: createApp() loads the manifest
// internally via the build-emitted entrypoint.
const app = createApp();
const logger = app.adapterLogger;

const ssrHandler = async (request: Request): Promise<Response> => {
  const routeData = app.match(request);
  if (!routeData) {
    return app.render(request, { addCookieHeader: true });
  }
  return app.render(request, { addCookieHeader: true, routeData });
};

// Resolve paths at runtime relative to this entrypoint file.
// Config values are relative paths from the server entrypoint's location.
const entryDir = dirname(fileURLToPath(import.meta.url));
const adapterDir = findAdapterDir(entryDir);
const clientDir = resolve(dirname(adapterDir), '..', 'client');

const staticManifest = new Map<string, ManifestEntry>(
  Object.entries(
    JSON.parse(readFileSync(join(adapterDir, 'static-manifest.json'), 'utf-8')),
  ),
);

let cacheServer: CacheServer | undefined;
if (config.cache) {
  const buildId = readFileSync(join(adapterDir, 'build-id'), 'utf-8').trim();
  const cacheDir = config.cache.cacheDir ?? join(adapterDir, 'cache');
  cacheServer = createCacheServer({
    origin: ssrHandler,
    maxByteSize: config.cache.maxByteSize,
    cacheDir,
    buildId,
    warmOnInit: config.cache.warmOnInit,
    imageEndpointRoute: config.imageEndpointRoute,
  });
  registerCache(cacheServer.cache);

  const shutdown = () => {
    cacheServer
      ?.shutdown()
      .catch((err: unknown) => console.error('Cache flush failed:', err))
      .finally(() => process.exit(0));
  };
  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);
}

const port = Number(process.env.PORT || config.port || 4321);
const host =
  process.env.HOST ??
  (typeof config.host === 'boolean'
    ? config.host
      ? '0.0.0.0'
      : 'localhost'
    : config.host);

export const server = Bun.serve({
  port,
  hostname: host,
  async fetch(request) {
    const url = new URL(request.url);
    const pathname = decodeURIComponent(url.pathname);
    const acceptEncoding = request.headers.get('accept-encoding') ?? '';

    if (request.method === 'GET' || request.method === 'HEAD') {
      const meta = staticManifest.get(pathname);
      if (meta) {
        const headers = new Headers(meta.headers);
        headers.set(CACHE_HEADER, 'STATIC');

        if (request.headers.get('if-none-match') === meta.headers.ETag) {
          headers.delete('Content-Length');
          headers.delete('Content-Type');
          return new Response(null, { status: 304, headers });
        }

        const fullPath = join(clientDir, meta.filePath);
        const { path: servePath, encoding } = resolveCompressed(
          fullPath,
          acceptEncoding,
        );

        if (encoding) {
          headers.set('Content-Encoding', encoding);
          headers.delete('Content-Length');
        }
        headers.set('Vary', 'Accept-Encoding');

        return new Response(Bun.file(servePath), { status: 200, headers });
      }

      const file = Bun.file(join(clientDir, pathname));
      if (await file.exists()) {
        const fullPath = join(clientDir, pathname);
        const { path: servePath, encoding } = resolveCompressed(
          fullPath,
          acceptEncoding,
        );

        const headers = new Headers();
        if (encoding) {
          headers.set('Content-Encoding', encoding);
          headers.set('Vary', 'Accept-Encoding');
        }

        return new Response(Bun.file(servePath), { headers });
      }
    }

    if (!cacheServer || request.method !== 'GET') {
      const response = await ssrHandler(request);
      response.headers.set(CACHE_HEADER, 'BYPASS');
      return response;
    }

    const cacheKey = pathname.startsWith(config.imageEndpointRoute)
      ? buildImageCacheKey(pathname, url.searchParams)
      : pathname;
    return cacheServer(request, cacheKey);
  },
});

logger.info(`Server listening on http://${host}:${port}`);
// Suppress noisy "Internal Warning: route cache overwritten" — upstream Astro
// bug where the route cache is re-set on every SSR request for dynamic routes.
// We can't filter just that one message via the logger API, so we drop the
// log level to 'error' after our own startup info is printed.
logger.options.level = 'error';
