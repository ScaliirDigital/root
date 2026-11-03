import { afterEach, beforeEach, expect, test } from 'bun:test';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { git } from '../src/git';
import { detectReleases, releaseNotes } from '../src/release';

let root: string;

const repository = 'example/root';
const notes = '### 🐛 Fixes\n\n- Handle empty input.\n';

function g(...args: string[]): string {
  return git(
    { cwd: root },
    '-c',
    'core.hooksPath=/dev/null',
    '-c',
    'commit.gpgSign=false',
    '-c',
    'tag.gpgSign=false',
    ...args,
  ).trim();
}

async function write(path: string, content: string) {
  const destination = join(root, path);

  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, content);
}

function pull(overrides: Record<string, unknown> = {}) {
  return {
    number: 42,
    body: '<!-- scale-release:v1 package=document -->',
    merged_at: '2026-10-29T12:00:00Z',
    merge_commit_sha: g('rev-parse', 'HEAD'),
    head: {
      ref: 'release/document',
      repo: { full_name: repository },
    },
    base: {
      ref: 'main',
      repo: { full_name: repository },
    },
    ...overrides,
  };
}

async function mergeRelease(
  mode: 'merge' | 'squash',
  version = '0.1.1',
  changelog = true,
) {
  g('switch', '--quiet', '-c', 'release/document');

  await write(
    'packages/document/Cargo.toml',
    `[package]\nname = "document"\nversion = "${version}"\n`,
  );

  if (changelog) {
    await write(
      'packages/document/CHANGELOG.md',
      `# Changelog\n\n## document ${version}\n\n${notes}\n## 0.1.0\n\n- Initial release.\n`,
    );
  }

  g('add', '.');
  g('commit', '--quiet', '-m', `chore(document): release ${version}`);
  g('switch', '--quiet', 'main');

  if (mode === 'squash') {
    g('merge', '--squash', 'release/document');
    g('commit', '--quiet', '-m', `chore(document): release ${version} (#42)`);
  } else {
    g(
      'merge',
      '--quiet',
      '--no-ff',
      'release/document',
      '-m',
      'Merge release PR #42',
    );
  }
}

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'release-detection-test-'));

  g('init', '--quiet', '--initial-branch=main');
  g('config', 'user.name', 'Release Test');
  g('config', 'user.email', 'release@example.invalid');

  await write(
    'tools/release/packages.toml',
    [
      '[packages.document]',
      'language = "rust"',
      'targets = ["cargo", "binary"]',
      'containers = ["ghcr", "dockerhub"]',
      '',
    ].join('\n'),
  );

  await write(
    'packages/document/Cargo.toml',
    '[package]\nname = "document"\nversion = "0.1.0"\n',
  );

  g('add', '.');
  g('commit', '--quiet', '-m', 'chore(root): initialize fixture');
});

afterEach(async () => {
  await rm(root, { recursive: true, force: true });
});

for (const mode of ['merge', 'squash'] as const) {
  test(`detects a release after a ${mode} merge`, async () => {
    await mergeRelease(mode);

    const sha = g('rev-parse', 'HEAD');

    const releases = await detectReleases(root, repository, async () => [
      pull(),
    ]);

    expect(releases).toEqual([
      {
        package: 'document',
        path: 'packages/document',
        version: '0.1.1',
        tag: 'document-v0.1.1',
        sha,
        pullRequest: 42,
        notes,
        targets: ['cargo', 'binary'],
        containers: ['ghcr', 'dockerhub'],
      },
    ]);
  });
}

test('ignores a commit without an associated release PR', async () => {
  expect(await detectReleases(root, repository, async () => [])).toEqual([]);
});

test('ignores ordinary PRs and mismatched release metadata', async () => {
  await mergeRelease('squash');

  const candidates = [
    pull({ body: 'An ordinary PR' }),
    pull({ merged_at: null }),
    pull({ merge_commit_sha: 'a'.repeat(40) }),
    pull({
      head: {
        ref: 'feature/document',
        repo: { full_name: repository },
      },
    }),
    pull({
      head: {
        ref: 'release/document',
        repo: { full_name: 'another/root' },
      },
    }),
    pull({
      base: {
        ref: 'develop',
        repo: { full_name: repository },
      },
    }),
  ];

  expect(
    await detectReleases(root, repository, async () => candidates),
  ).toEqual([]);
});

test('rejects ambiguous matching release PRs', async () => {
  await mergeRelease('squash');

  await expect(
    detectReleases(root, repository, async () => [
      pull(),
      pull({ number: 43 }),
    ]),
  ).rejects.toThrow('multiple matching');
});

test('rejects a release without a version bump', async () => {
  await mergeRelease('squash', '0.1.0');

  await expect(
    detectReleases(root, repository, async () => [pull()]),
  ).rejects.toThrow('invalid release version transition');
});

test('rejects a release without a committed changelog', async () => {
  await mergeRelease('squash', '0.1.1', false);

  await expect(
    detectReleases(root, repository, async () => [pull()]),
  ).rejects.toThrow('missing committed changelog');
});

test('detects the same release again after its tag exists', async () => {
  await mergeRelease('squash');

  const before = await detectReleases(root, repository, async () => [pull()]);

  g('tag', 'document-v0.1.1');

  const refs = g('show-ref');

  const repeated = await detectReleases(root, repository, async () => [pull()]);

  expect(repeated).toEqual(before);
  expect(g('show-ref')).toBe(refs);
  expect(g('status', '--porcelain')).toBe('');
});

test('reads the committed manifest instead of local edits', async () => {
  await mergeRelease('squash');

  await write(
    'packages/document/Cargo.toml',
    '[package]\nname = "document"\nversion = "9.9.9"\n',
  );

  const releases = await detectReleases(root, repository, async () => [pull()]);

  expect(releases[0]?.version).toBe('0.1.1');
});

test('propagates GitHub errors', async () => {
  await expect(
    detectReleases(root, repository, async () => {
      throw new Error('API unavailable');
    }),
  ).rejects.toThrow('API unavailable');
});

test('extracts only the selected release notes', () => {
  expect(
    releaseNotes(
      '# Changelog\n\n## 0.1.1\n\nNew notes.\n\n## 0.1.0\n\nOld notes.\n',
      '0.1.1',
    ),
  ).toBe('New notes.\n');
});

test('rejects missing or empty release notes', () => {
  expect(() => releaseNotes('# Changelog\n', '0.1.1')).toThrow(
    'Missing changelog section',
  );

  expect(() =>
    releaseNotes('# Changelog\n\n## 0.1.1\n\n## 0.1.0\n', '0.1.1'),
  ).toThrow('Empty release notes');
});
