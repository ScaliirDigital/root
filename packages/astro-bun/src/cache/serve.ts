import type { CacheControl, CacheEntry, CacheServer } from '../types.ts';
import { createCacheStore, defaultDeps, type StoreDeps } from './store.ts';

// ─── Constants ────────────────────────────────────────────────────────────

const CACHE_HEADER = 'x-astro-cache';

/**
 * Override for Astro's image endpoint: it hardcodes `max-age=31536000` without
 * `s-maxage`, so without this override images would always bypass the cache.
 */
const IMAGE_CACHE_CONTROL =
  'public, max-age=31536000, s-maxage=31536000, stale-while-revalidate=86400';

// ─── Types ────────────────────────────────────────────────────────────────

type CacheStatus = 'HIT' | 'STALE' | 'MISS' | 'BYPASS';

type CacheDecision =
  | { status: 'hit'; entry: CacheEntry }
  | { status: 'stale'; entry: CacheEntry }
  | { status: 'expired' }
  | { status: 'miss' };

export type ServerOptions = {
  origin: (request: Request, clientAddress?: string) => Promise<Response>;
  maxByteSize: number;
  cacheDir: string;
  buildId: string;
  warmOnInit: boolean;
  imageEndpointRoute: string;
};

// ─── Pure: cache-control parsing ──────────────────────────────────────────

/** Extract a numeric directive value from a Cache-Control header string. */
function parseDirective(header: string, name: string): number | undefined {
  const regex = new RegExp(`(?:^|,)\\s*${name}\\s*=\\s*(\\d+)`, 'i');
  const match = header.match(regex);
  return match ? Number(match[1]) : undefined;
}

/** Extract a cacheable entry from response data, or undefined if not cacheable. */
function extractCacheableEntry(
  headers: [string, string][],
  status: number,
  body: Uint8Array<ArrayBuffer>,
  now: number,
): CacheEntry | undefined {
  const ccHeader = headers.find(([n]) => n === 'cache-control')?.[1] ?? '';
  const sMaxAge = parseDirective(ccHeader, 's-maxage');
  if (!sMaxAge || sMaxAge <= 0) return undefined;

  return {
    body,
    headers,
    status,
    cachedAt: now,
    sMaxAge,
    swr: parseDirective(ccHeader, 'stale-while-revalidate') ?? 0,
  };
}

// ─── Pure: image endpoint override ────────────────────────────────────────

function isImageEndpointKey(key: string, route: string): boolean {
  return key === route || key.startsWith(`${route}?`);
}

function overrideImageCacheControl(headers: [string, string][]): void {
  for (let i = 0; i < headers.length; i++) {
    if (headers[i]?.[0] === 'cache-control') {
      headers[i] = ['cache-control', IMAGE_CACHE_CONTROL];
      return;
    }
  }
}

// ─── Pure: cache decision ─────────────────────────────────────────────────

/** Classify a cache lookup result against the current time. */
function classifyEntry(
  entry: CacheEntry | undefined,
  now: number,
): CacheDecision {
  if (!entry) return { status: 'miss' };
  const elapsed = now - entry.cachedAt;
  if (elapsed < entry.sMaxAge * 1000) return { status: 'hit', entry };
  if (elapsed < (entry.sMaxAge + entry.swr) * 1000) {
    return { status: 'stale', entry };
  }
  return { status: 'expired' };
}

// ─── Pure: response construction ──────────────────────────────────────────

function responseFromEntry(entry: CacheEntry, status: CacheStatus): Response {
  const response = new Response(entry.body, {
    status: entry.status,
    headers: entry.headers,
  });
  response.headers.set(CACHE_HEADER, status);
  return response;
}

// ─── Render path ──────────────────────────────────────────────────────────

type RenderResult = {
  streaming: Promise<Response>;
  entry: Promise<CacheEntry | undefined>;
};

/**
 * Render via origin, persist to cache if eligible, return streaming response
 * + a promise resolving to the cached entry (or undefined if not cacheable).
 *
 * Both `streaming` and `entry` may reject if origin throws. Callers MUST
 * either await/catch both, or use only one — the other will be silently
 * swallowed by an internal handler to prevent unhandled rejection warnings.
 */
function renderAndCache(
  request: Request,
  origin: ServerOptions['origin'],
  store: ReturnType<typeof createCacheStore>,
  cacheKey: string,
  initialStatus: CacheStatus,
  imageEndpointRoute: string,
  now: () => number,
  clientAddress?: string,
): RenderResult {
  const done = origin(request, clientAddress).then((response) => {
    const clone = response.clone();
    const headers: [string, string][] = Array.from(clone.headers.entries());

    if (isImageEndpointKey(cacheKey, imageEndpointRoute)) {
      overrideImageCacheControl(headers);
    }

    const entryPromise = clone.arrayBuffer().then(async (buf) => {
      const body = new Uint8Array(buf);
      const entry = extractCacheableEntry(headers, clone.status, body, now());
      if (entry) await store.set(cacheKey, entry);
      return entry;
    });

    response.headers.set(CACHE_HEADER, initialStatus);
    return { response, entryPromise };
  });

  // Pre-attach a no-op rejection handler so unawaited branches don't surface
  // as unhandled rejections (the SWR path only consumes `entry`; the miss
  // path may only consume `streaming`). Each consumer can still observe the
  // rejection via its own .catch / await.
  const streaming = done.then(({ response }) => response);
  const entry = done.then(({ entryPromise }) => entryPromise);
  streaming.catch(() => {});
  entry.catch(() => {});

  return { streaming, entry };
}

// ─── Public factory ───────────────────────────────────────────────────────

/**
 * Create a caching server with LRU storage, stale-while-revalidate, and
 * request coalescing in front of an origin handler.
 */
export function createCacheServer(
  options: ServerOptions,
  deps: StoreDeps = defaultDeps(),
): CacheServer {
  const {
    origin,
    maxByteSize,
    cacheDir,
    buildId,
    warmOnInit,
    imageEndpointRoute,
  } = options;

  const store = createCacheStore(
    { maxByteSize, cacheDir, buildId, warmOnInit },
    deps,
  );
  const revalidationsInFlight = new Set<string>();
  const rendersInFlight = new Map<string, Promise<CacheEntry | undefined>>();

  const now = () => deps.clock.now();

  const handler = (async (
    request: Request,
    cacheKey: string,
    clientAddress?: string,
  ) => {
    const decision = classifyEntry(await store.get(cacheKey), now());

    if (decision.status === 'hit') {
      return responseFromEntry(decision.entry, 'HIT');
    }

    if (decision.status === 'stale') {
      if (!revalidationsInFlight.has(cacheKey)) {
        revalidationsInFlight.add(cacheKey);
        // SWR revalidation
        const result = renderAndCache(
          new Request(request.url, request),
          origin,
          store,
          cacheKey,
          'STALE',
          imageEndpointRoute,
          now,
          clientAddress,
        );
        result.entry
          .catch(() => {})
          .finally(() => revalidationsInFlight.delete(cacheKey));
      }
      return responseFromEntry(decision.entry, 'STALE');
    }

    if (decision.status === 'expired') {
      await store.delete(cacheKey);
    }

    // Miss or expired — coalesce concurrent renders for the same key.
    const pending = rendersInFlight.get(cacheKey);
    if (!pending) {
      // Cache miss
      const result = renderAndCache(
        request,
        origin,
        store,
        cacheKey,
        'MISS',
        imageEndpointRoute,
        now,
        clientAddress,
      );
      rendersInFlight.set(cacheKey, result.entry);
      result.entry.finally(() => rendersInFlight.delete(cacheKey));
      return result.streaming;
    }

    const cached = await pending;
    if (cached) return responseFromEntry(cached, 'MISS');

    // Not cacheable — direct origin call.
    const response = await origin(request, clientAddress);
    response.headers.set(CACHE_HEADER, 'BYPASS');
    return response;
  }) as CacheServer;

  handler.shutdown = () => store.save();
  handler.cache = {
    expire: (key) => store.delete(key),
    expireAll: async () => {
      const deletes: Promise<void>[] = [];
      for (const key of [...store.keys]) {
        if (isImageEndpointKey(key, imageEndpointRoute)) continue;
        deletes.push(store.delete(key));
      }
      await Promise.all(deletes);
    },
  } satisfies CacheControl;

  return handler;
}

// ─── Test-only exports ────────────────────────────────────────────────────

/** @internal Exposed only for unit tests. Do not use. */
export const __test__ = {
  parseDirective,
  extractCacheableEntry,
  isImageEndpointKey,
  overrideImageCacheControl,
  classifyEntry,
  responseFromEntry,
};
