import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, posix } from 'node:path';
import { isDeepStrictEqual } from 'node:util';
import { git } from './git';
import type { FileChange, PackageConfig } from './types';

type CargoLock = {
  package: Array<{
    name: string;
    version: string;
    source?: string;
  }>;
};

type BunLock = {
  workspaces: Record<string, { version?: string }>;
};

function readFile(root: string, head: string, path: string): string | null {
  const entry = git(
    { cwd: root },
    '--literal-pathspecs',
    'ls-tree',
    '--name-only',
    head,
    '--',
    path,
  ).trim();

  return entry ? git({ cwd: root }, 'show', `${head}:${path}`) : null;
}

function findLockfile(
  root: string,
  head: string,
  packagePath: string,
  filename: string,
): { path: string; content: string } | null {
  let directory = packagePath;

  while (true) {
    const path = directory === '.' ? filename : `${directory}/${filename}`;
    const content = readFile(root, head, path);

    if (content !== null) return { path, content };
    if (directory === '.') return null;

    directory = posix.dirname(directory);
  }
}

function run(command: string[], cwd: string): void {
  const result = Bun.spawnSync(command, {
    cwd,
    stdout: 'pipe',
    stderr: 'pipe',
  });

  if (result.exitCode !== 0) {
    throw new Error(
      result.stderr.toString().trim() || `Command failed: ${command.join(' ')}`,
    );
  }
}

export async function planLockfile(
  root: string,
  head: string,
  pkg: PackageConfig,
  manifestAfter: string,
): Promise<FileChange | null> {
  const filename = pkg.language === 'rust' ? 'Cargo.lock' : 'bun.lock';
  const lock = findLockfile(root, head, pkg.path, filename);

  // Keine neue Lockfile-Policy für Pakete ohne committierten Lockfile.
  if (!lock) return null;

  const manifestPath = `${pkg.path}/${pkg.manifest}`;
  const manifestBefore = git({ cwd: root }, 'show', `${head}:${manifestPath}`);

  const temporary = await mkdtemp(join(tmpdir(), 'release-lockfile-'));
  const snapshot = join(temporary, 'source');

  try {
    await mkdir(snapshot);

    const options = {
      cwd: root,
      env: {
        GIT_INDEX_FILE: join(temporary, 'index'),
      },
    };

    git(options, 'read-tree', head);
    git(options, 'checkout-index', '--all', `--prefix=${snapshot}/`);

    await Bun.write(join(snapshot, manifestPath), manifestAfter);

    if (pkg.language === 'rust') {
      run(
        [
          'cargo',
          'metadata',
          '--format-version',
          '1',
          '--manifest-path',
          join(snapshot, manifestPath),
        ],
        snapshot,
      );
    } else {
      run(
        ['bun', 'install', '--lockfile-only', '--ignore-scripts'],
        join(snapshot, posix.dirname(lock.path)),
      );
    }

    const after = await Bun.file(join(snapshot, lock.path)).text();

    if (pkg.language === 'rust') {
      type CargoManifest = {
        package?: {
          name?: unknown;
          version?: unknown;
        };
      };

      const beforeManifest = Bun.TOML.parse(manifestBefore) as CargoManifest;
      const afterManifest = Bun.TOML.parse(manifestAfter) as CargoManifest;

      const name = beforeManifest.package?.name;
      const currentVersion = beforeManifest.package?.version;
      const version = afterManifest.package?.version;

      if (
        typeof name !== 'string' ||
        typeof currentVersion !== 'string' ||
        typeof version !== 'string'
      ) {
        throw new Error(`${manifestPath}: missing package name or version`);
      }

      const original = Bun.TOML.parse(lock.content) as CargoLock;
      const updated = Bun.TOML.parse(after) as CargoLock;

      if (!Array.isArray(original.package) || !Array.isArray(updated.package)) {
        throw new Error(`${lock.path}: expected Cargo package entries`);
      }

      const matches = original.package.filter(
        (entry) =>
          entry.name === name &&
          entry.version === currentVersion &&
          entry.source === undefined,
      );

      if (matches.length !== 1) {
        throw new Error(`${lock.path}: expected one local package entry`);
      }

      const expected = {
        ...original,
        package: original.package.map((entry) =>
          entry === matches[0] ? { ...entry, version } : entry,
        ),
      };

      if (!isDeepStrictEqual(updated, expected)) {
        throw new Error(
          `${lock.path}: Cargo changed dependencies or other lockfile data`,
        );
      }
    } else {
      const original = Bun.JSONC.parse(lock.content) as BunLock;
      const updated = Bun.JSONC.parse(after) as BunLock;
      const manifest = JSON.parse(manifestAfter);

      const workspace = posix.relative(posix.dirname(lock.path), pkg.path);

      const entry = original.workspaces?.[workspace];

      if (!entry || typeof manifest.version !== 'string') {
        throw new Error(`${lock.path}: missing package workspace entry`);
      }

      const expected =
        workspace === ''
          ? original
          : {
              ...original,
              workspaces: {
                ...original.workspaces,
                [workspace]: {
                  ...entry,
                  version: manifest.version,
                },
              },
            };

      if (!isDeepStrictEqual(updated, expected)) {
        throw new Error(
          `${lock.path}: Bun changed dependencies or other lockfile data`,
        );
      }
    }

    return lock.content === after
      ? null
      : { path: lock.path, before: lock.content, after };
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
