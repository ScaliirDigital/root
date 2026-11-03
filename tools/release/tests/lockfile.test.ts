import { afterEach, beforeEach, expect, test } from 'bun:test';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { validatePackage } from '../src/config';
import { git } from '../src/git';
import { planLockfile } from '../src/lockfile';
import { bumpManifest } from '../src/manifest';

let root: string;

async function write(path: string, content: string) {
  const destination = join(root, path);

  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, content);
}

function run(command: string[], cwd = root) {
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

function commitFixture(): string {
  git({ cwd: root }, '-c', 'core.hooksPath=/dev/null', 'add', '.');

  git(
    { cwd: root },
    '-c',
    'user.name=Release Test',
    '-c',
    'user.email=release@example.invalid',
    '-c',
    'core.hooksPath=/dev/null',
    '-c',
    'commit.gpgSign=false',
    'commit',
    '--quiet',
    '-m',
    'chore(root): initialize fixture',
  );

  return git({ cwd: root }, 'rev-parse', 'HEAD').trim();
}

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'release-lockfile-test-'));
  git({ cwd: root }, 'init', '--quiet', '--initial-branch=main');
});

afterEach(async () => {
  await rm(root, { recursive: true, force: true });
});

test('Cargo updates its lockfile without changing the working tree', async () => {
  const manifest = [
    '[package]',
    'name = "document"',
    'version = "0.1.0"',
    'edition = "2021"',
    '',
  ].join('\n');

  await write('packages/document/Cargo.toml', manifest);
  await write('packages/document/src/main.rs', 'fn main() {}\n');

  run([
    'cargo',
    'metadata',
    '--offline',
    '--format-version',
    '1',
    '--manifest-path',
    join(root, 'packages/document/Cargo.toml'),
  ]);

  const path = 'packages/document/Cargo.lock';
  const before = await Bun.file(join(root, path)).text();
  const head = commitFixture();

  // Auch bereits vorhandene lokale Änderungen müssen erhalten bleiben.
  await write('local.txt', 'Keep this file\n');

  const status = git({ cwd: root }, 'status', '--porcelain');

  const pkg = validatePackage('document', {
    language: 'rust',
    targets: ['cargo'],
  });

  const change = await planLockfile(
    root,
    head,
    pkg,
    bumpManifest(manifest, 'Cargo.toml', '0.1.0', '0.1.1'),
  );

  if (!change) throw new Error('Expected a Cargo.lock change');

  expect(change.path).toBe(path);
  expect(change.before).toBe(before);
  expect(change.after).toContain('version = "0.1.1"');

  expect(await Bun.file(join(root, path)).text()).toBe(before);
  expect(
    await Bun.file(join(root, 'packages/document/Cargo.toml')).text(),
  ).toBe(manifest);

  expect(git({ cwd: root }, 'status', '--porcelain')).toBe(status);
  expect(await Bun.file(join(root, 'local.txt')).text()).toBe(
    'Keep this file\n',
  );

  // Die vorgeschlagenen Dateien müssen gemeinsam --locked akzeptieren.
  await write(
    'packages/document/Cargo.toml',
    bumpManifest(manifest, 'Cargo.toml', '0.1.0', '0.1.1'),
  );
  await write(path, change.after);

  run([
    'cargo',
    'metadata',
    '--offline',
    '--locked',
    '--format-version',
    '1',
    '--manifest-path',
    join(root, 'packages/document/Cargo.toml'),
  ]);
}, 30_000);

for (const workspace of [true, false]) {
  test(`Bun updates the ${workspace ? 'workspace' : 'package'} lockfile`, async () => {
    const packagePath = 'packages/adapter';
    const manifest = `${JSON.stringify(
      {
        name: '@example/adapter',
        version: '0.1.0',
        dependencies: {
          '@example/dependency': 'file:./dependency',
        },
      },
      null,
      2,
    )}\n`;

    await write(
      `${packagePath}/dependency/package.json`,
      `${JSON.stringify(
        {
          name: '@example/dependency',
          version: '1.0.0',
        },
        null,
        2,
      )}\n`,
    );

    if (workspace) {
      await write(
        'package.json',
        `${JSON.stringify(
          {
            name: '@example/root',
            private: true,
            workspaces: ['packages/adapter'],
          },
          null,
          2,
        )}\n`,
      );
    }

    await write(`${packagePath}/package.json`, manifest);

    const installRoot = workspace ? root : join(root, packagePath);

    run(
      [process.execPath, 'install', '--lockfile-only', '--ignore-scripts'],
      installRoot,
    );

    const path = workspace ? 'bun.lock' : `${packagePath}/bun.lock`;
    const before = await Bun.file(join(root, path)).text();
    const head = commitFixture();
    const status = git({ cwd: root }, 'status', '--porcelain');

    const pkg = validatePackage('adapter', {
      language: 'typescript',
      targets: ['npm'],
    });

    const manifestAfter = bumpManifest(
      manifest,
      'package.json',
      '0.1.0',
      '0.1.1',
    );

    const change = await planLockfile(root, head, pkg, manifestAfter);

    if (workspace) {
      if (!change) throw new Error('Expected a workspace bun.lock change');

      expect(change.path).toBe(path);
      expect(change.before).toBe(before);
      expect(change.after).toContain('0.1.1');
    } else {
      expect(change).toBeNull();
    }

    expect(await Bun.file(join(root, path)).text()).toBe(before);

    expect(await Bun.file(join(root, packagePath, 'package.json')).text()).toBe(
      manifest,
    );

    expect(git({ cwd: root }, 'status', '--porcelain')).toBe(status);

    await write(`${packagePath}/package.json`, manifestAfter);
    await write(path, change?.after ?? before);

    run(
      [
        process.execPath,
        'install',
        '--lockfile-only',
        '--ignore-scripts',
        '--frozen-lockfile',
      ],
      installRoot,
    );
  }, 30_000);
}
