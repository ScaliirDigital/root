import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type {
  AstroAdapter,
  AstroConfig,
  AstroIntegration,
  RouteToHeaders,
} from 'astro';
import { compressStaticAssets } from './compression/index.ts';
import { generateStaticManifest } from './manifest.ts';
import type { AdapterOptions } from './types.ts';

export type { AdapterOptions } from './types.ts';

const VIRTUAL_MODULE_ID = 'virtual:@scale.digital/astro-bun/config';
const RESOLVED_VIRTUAL_MODULE_ID = `\0${VIRTUAL_MODULE_ID}`;

function getAdapter(): AstroAdapter {
  return {
    name: '@scale.digital/astro-bun',
    serverEntrypoint: '@scale.digital/astro-bun/server.js',
    previewEntrypoint: '@scale.digital/astro-bun/preview',
    entrypointResolution: 'auto',
    adapterFeatures: {
      buildOutput: 'server',
      edgeMiddleware: false,
      staticHeaders: true,
    },
    supportedAstroFeatures: {
      hybridOutput: 'stable',
      staticOutput: 'stable',
      serverOutput: 'stable',
      sharpImageService: 'stable',
      envGetSecret: 'stable',
    },
  };
}

export type CacheConfig = {
  maxByteSize?: number;
  cacheDir?: string;
  warmOnInit?: boolean;
};

type BunAdapterConfig = {
  staticCacheControl?: string;
  cache?: boolean | CacheConfig;
  compress?: boolean;
};

export default function bun(
  adapterConfig?: BunAdapterConfig,
): AstroIntegration {
  const staticCacheControl =
    adapterConfig?.staticCacheControl ??
    'public, max-age=86400, must-revalidate';
  const compress = adapterConfig?.compress ?? true;

  let config: AstroConfig | undefined;
  let command: string | undefined;
  let adapterDir: string | undefined;
  let routeToHeaders: RouteToHeaders | undefined;
  let adapterOptions: AdapterOptions | undefined;

  return {
    name: '@scale.digital/astro-bun',
    hooks: {
      'astro:config:setup': (options) => {
        command = options.command;
        const { updateConfig, config: currentConfig } = options;
        updateConfig({
          build: {
            redirects: false,
          },
          image: {
            endpoint: {
              route: currentConfig.image.endpoint.route ?? '_image',
              entrypoint:
                currentConfig.image.endpoint.entrypoint ??
                (options.command === 'dev'
                  ? 'astro/assets/endpoint/dev'
                  : 'astro/assets/endpoint/node'),
            },
          },
          session: {
            driver: currentConfig.session?.driver ?? 'fs-lite',
          },
          vite: {
            ssr: {
              ...(options.command !== 'dev' && {
                noExternal: true,
                external: ['sharp'],
              }),
            },
            plugins: [
              {
                name: '@scale.digital/astro-bun/config',
                resolveId(id) {
                  if (id === VIRTUAL_MODULE_ID)
                    return RESOLVED_VIRTUAL_MODULE_ID;
                },
                load(id) {
                  if (id === RESOLVED_VIRTUAL_MODULE_ID) {
                    return `export default ${JSON.stringify(adapterOptions ?? {})}`;
                  }
                },
              },
            ],
          },
        });
      },
      'astro:config:done': ({ setAdapter, config: doneConfig }) => {
        config = doneConfig;
        const isDevMode = command === 'dev';
        const cacheConfig: CacheConfig =
          typeof adapterConfig?.cache === 'object' ? adapterConfig.cache : {};

        const relativeAdapterDir = '.astro-bun-adapter';
        const serverPath = fileURLToPath(new URL(doneConfig.build.server));
        adapterDir = join(serverPath, relativeAdapterDir);

        adapterOptions = {
          host: doneConfig.server.host,
          port: doneConfig.server.port,
          // Relative paths from the server entrypoint. Resolved at runtime.
          // Entrypoint lives in dist/server/, client in dist/client/, adapter artifacts in dist/server/.astro-bun-adapter/
          client: '../client',
          server: '.',
          adapterDir: relativeAdapterDir,
          assets: doneConfig.build.assets,
          staticCacheControl,
          imageEndpointRoute: doneConfig.image.endpoint.route.startsWith('/')
            ? doneConfig.image.endpoint.route
            : `/${doneConfig.image.endpoint.route}`,
          cache:
            !isDevMode && adapterConfig?.cache
              ? {
                  maxByteSize: cacheConfig.maxByteSize ?? 50 * 1024 * 1024,
                  cacheDir: cacheConfig.cacheDir,
                  warmOnInit: cacheConfig.warmOnInit ?? false,
                }
              : false,
        };

        setAdapter(getAdapter());
      },
      'astro:build:generated': ({ routeToHeaders: rth }) => {
        routeToHeaders = rth;
      },
      'astro:build:done': async ({ logger }) => {
        if (!config || !adapterDir) return;

        const clientDir = new URL(config.build.client, config.outDir);
        await mkdir(adapterDir, { recursive: true });

        let serializedRouteHeaders:
          | Record<string, Record<string, string>>
          | undefined;
        if (routeToHeaders && routeToHeaders.size > 0) {
          serializedRouteHeaders = {};
          for (const [route, payload] of routeToHeaders) {
            const headers: Record<string, string> = {};
            payload.headers.forEach((value, key) => {
              headers[key] = value;
            });
            serializedRouteHeaders[route] = headers;
          }
        }

        await generateStaticManifest(
          clientDir.pathname,
          adapterDir,
          config.build.assets,
          serializedRouteHeaders,
          staticCacheControl,
        );

        if (compress) {
          const start = performance.now();
          const { fileCount, bytesSaved } = await compressStaticAssets(
            clientDir.pathname,
          );
          const elapsed = ((performance.now() - start) / 1000).toFixed(2);
          const savedKiB = Math.round(bytesSaved / 1024);
          logger.info(
            `Compressed ${fileCount} assets (saved ~${savedKiB} KiB) in ${elapsed}s.`,
          );
        }

        const buildId = randomUUID();
        await writeFile(join(adapterDir, 'build-id'), buildId);
      },
    },
  };
}
