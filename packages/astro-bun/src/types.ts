/** Build-time configuration passed to the server entrypoint via virtual module. */
export type AdapterOptions = {
  /** Hostname or boolean (`true` = `"0.0.0.0"`, `false` = `"localhost"`). */
  host: string | boolean;
  /** Port the server listens on. */
  port: number;
  /** Absolute `file://` URL to `dist/client/`. */
  client: string;
  /** Absolute `file://` URL to `dist/server/`. */
  server: string;
  /** Relative path to adapter directory within `dist/server/`. */
  adapterDir: string;
  /** Name of the assets directory (default `_astro`). */
  assets: string;
  /** `Cache-Control` header for non-hashed static assets. */
  staticCacheControl: string;
  /** Image endpoint route with leading slash (e.g. `"/_image"`). */
  imageEndpointRoute: string;
  /** Cache configuration. `false` disables caching. */
  cache: false | CacheOptions;
};

/** Resolved cache configuration. */
export type CacheOptions = {
  maxByteSize: number;
  cacheDir?: string;
  warmOnInit: boolean;
};

/** A cached SSR response with timing metadata for fresh/stale/expired checks. */
export type CacheEntry = {
  body: Uint8Array<ArrayBuffer>;
  headers: [string, string][];
  status: number;
  cachedAt: number;
  /** `s-maxage` in seconds — defines the fresh window. */
  sMaxAge: number;
  /** `stale-while-revalidate` in seconds — defines the stale window. */
  swr: number;
};

/** Pre-computed response headers for a static file. */
export type ManifestEntry = {
  headers: Record<string, string>;
  /** Relative file path within client dir. */
  filePath: string;
};

export type StaticManifest = Record<string, ManifestEntry>;

/** Minimal control surface for on-demand cache expiration. */
export type CacheControl = {
  expire: (key: string) => Promise<void>;
  expireAll: () => Promise<void>;
};

/** Caching request handler with shutdown and on-demand expiration. */
export type CacheServer = {
  (
    request: Request,
    cacheKey: string,
    clientAddress?: string,
  ): Promise<Response>;
  shutdown: () => Promise<void>;
  cache: CacheControl;
};
