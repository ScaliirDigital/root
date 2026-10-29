import { afterEach, beforeEach, expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { planBranch, prepareBranch } from '../src/branch';
import { git } from '../src/git';
import type { ReleasePlan } from '../src/types';

let root: string;
let plan: ReleasePlan;

const cargo = ['[package]', 'name = "document"', 'version = "0.1.0"', ''].join(
  '\n',
);

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

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'release-branch-test-'));

  await Bun.write(join(root, 'packages/document/Cargo.toml'), cargo);

  await Bun.write(
    join(root, 'packages/astro-bun/package.json'),
    '{"version":"0.1.2"}\n',
  );

  await Bun.write(join(root, 'README.md'), '# Repository\n');

  g('init', '--quiet', '--initial-branch=main');
  g('config', 'user.name', 'Release Test');
  g('config', 'user.email', 'release@example.invalid');
  g('add', '.');
  g('commit', '--quiet', '-m', 'chore(root): initialize fixture');

  const headSha = g('rev-parse', 'HEAD');

  plan = {
    package: 'document',
    language: 'rust',
    targets: ['cargo'],
    containers: [],
    baseTag: 'document-v0.1.0',
    baseSha: headSha,
    headSha,
    currentVersion: '0.1.0',
    nextVersion: '0.1.1',
    bump: 'patch',
    commits: [],
    changes: [
      {
        path: 'packages/document/Cargo.toml',
        before: cargo,
        after: cargo.replace('0.1.0', '0.1.1'),
      },
      {
        path: 'packages/document/CHANGELOG.md',
        before: null,
        after: '# Changelog\n\n## 0.1.1\n',
      },
    ],
  };
});

afterEach(async () => {
  if (root) {
    await rm(root, { recursive: true, force: true });
  }
});

test('creates only the planned package changes on a separate branch', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  expect(planned.branch).toEqual({
    name: 'release/document',
    action: 'create',
    expectedSha: null,
  });

  const result = await prepareBranch(root, planned);

  if (!result) throw new Error('Expected a release branch');

  expect(result.branch).toBe('release/document');
  expect(result.changed).toBe(true);

  expect(g('show', '-s', '--format=%P', result.sha)).toBe(plan.headSha);

  expect(
    g('diff', '--name-only', plan.headSha, result.sha).split('\n').sort(),
  ).toEqual(['packages/document/CHANGELOG.md', 'packages/document/Cargo.toml']);

  expect(g('show', `${result.sha}:packages/document/Cargo.toml`)).toBe(
    cargo.replace('0.1.0', '0.1.1').trim(),
  );

  expect(g('status', '--porcelain')).toBe('');
  expect(g('symbolic-ref', '--short', 'HEAD')).toBe('main');
});

test('does not create another commit for an identical plan', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  const first = await prepareBranch(root, planned);

  const repeated = {
    ...plan,
    branch: planBranch(root, plan),
  };

  expect(repeated.branch?.action).toBe('unchanged');

  const second = await prepareBranch(root, repeated);

  if (!first || !second) {
    throw new Error('Expected release branches');
  }

  expect(first.changed).toBe(true);
  expect(second.changed).toBe(false);
  expect(second.branch).toBe(first.branch);
  expect(second.sha).toBe(first.sha);
});

test('updates the existing branch from a newer baseline without another version bump', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  const first = await prepareBranch(root, planned);

  await Bun.write(join(root, 'README.md'), '# Repository\n\nNew change.\n');

  g('add', 'README.md');
  g('commit', '--quiet', '-m', 'fix(document): handle another case');

  const next = {
    ...plan,
    headSha: g('rev-parse', 'HEAD'),
  };

  const updated = {
    ...next,
    branch: planBranch(root, next),
  };

  expect(updated.branch?.action).toBe('update');

  const result = await prepareBranch(root, updated);

  if (!result || !first) {
    throw new Error('Expected release branches');
  }

  expect(updated.branch?.expectedSha).toBe(first.sha);
  expect(result.branch).toBe(first.branch);
  expect(result.sha).not.toBe(first.sha);
  expect(result.changed).toBe(true);

  expect(g('show', '-s', '--format=%P', result.sha)).toBe(next.headSha);

  expect(g('show', `${result.sha}:packages/document/Cargo.toml`)).toContain(
    'version = "0.1.1"',
  );
});

test('refuses a manually extended release branch', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  const result = await prepareBranch(root, planned);

  if (!result) throw new Error('Expected a release branch');

  const tree = g('rev-parse', `${result.sha}^{tree}`);

  const manual = g(
    'commit-tree',
    tree,
    '-p',
    result.sha,
    '-m',
    'docs: manual change',
  );

  g('update-ref', 'refs/heads/release/document', manual, result.sha);

  expect(() => planBranch(root, plan)).toThrow('manually changed');

  await expect(prepareBranch(root, planned)).rejects.toThrow(
    'manually changed',
  );

  expect(g('rev-parse', 'release/document')).toBe(manual);
});

test('refuses an unmanaged existing branch', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  g('branch', 'release/document');

  expect(() => planBranch(root, plan)).toThrow('unmanaged');

  await expect(prepareBranch(root, planned)).rejects.toThrow('unmanaged');
});

test('refuses amended file contents even if the release message was preserved', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  const result = await prepareBranch(root, planned);

  if (!result) throw new Error('Expected a release branch');

  const options = {
    cwd: root,
    env: {
      GIT_INDEX_FILE: join(root, '.git/manual-test-index'),
    },
  };

  git(options, 'read-tree', result.sha);

  const blob = git(
    { ...options, input: 'Manually changed contents\n' },
    'hash-object',
    '-w',
    '--stdin',
  ).trim();

  git(
    options,
    'update-index',
    '--cacheinfo',
    `100644,${blob},packages/document/Cargo.toml`,
  );

  const tree = git(options, 'write-tree').trim();
  const message = g('show', '-s', '--format=%B', result.sha);

  const manual = g('commit-tree', tree, '-p', plan.headSha, '-m', message);

  g('update-ref', 'refs/heads/release/document', manual, result.sha);

  expect(() => planBranch(root, plan)).toThrow('manually changed');

  await expect(prepareBranch(root, planned)).rejects.toThrow(
    'manually changed',
  );

  expect(g('rev-parse', 'release/document')).toBe(manual);
});

test('preserves staged, unstaged and untracked files', async () => {
  await Bun.write(join(root, 'README.md'), 'Staged content\n');
  g('add', 'README.md');

  await Bun.write(join(root, 'README.md'), 'Unstaged content\n');
  await Bun.write(join(root, 'untracked.txt'), 'Keep this file\n');

  const snapshot = async () => ({
    head: g('rev-parse', 'HEAD'),
    status: g('status', '--porcelain=v1', '--untracked-files=all'),
    staged: g('diff', '--cached', '--binary'),
    unstaged: g('diff', '--binary'),
    readme: await Bun.file(join(root, 'README.md')).text(),
    untracked: await Bun.file(join(root, 'untracked.txt')).text(),
    cargo: await Bun.file(join(root, 'packages/document/Cargo.toml')).text(),
  });

  const before = await snapshot();

  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  await prepareBranch(root, planned);

  expect(await snapshot()).toEqual(before);
});

test('rejects a stale file proposal without creating a release branch', () => {
  const stale = {
    ...plan,
    changes: [
      {
        path: 'packages/document/Cargo.toml',
        before: 'wrong content',
        after: 'wrong update',
      },
    ],
  };

  expect(() => planBranch(root, stale)).toThrow('does not match');

  expect(
    g('for-each-ref', '--format=%(refname)', 'refs/heads/release/document'),
  ).toBe('');

  expect(g('status', '--porcelain')).toBe('');
});

test('allows only one of two competing branch creations', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  const results = await Promise.allSettled([
    prepareBranch(root, planned),
    prepareBranch(root, planned),
  ]);

  expect(
    results.filter((result) => result.status === 'fulfilled'),
  ).toHaveLength(1);

  expect(results.filter((result) => result.status === 'rejected')).toHaveLength(
    1,
  );
});

test('refuses to update a checked-out release branch', async () => {
  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  await prepareBranch(root, planned);

  g('switch', '--quiet', 'release/document');

  const next = {
    ...planned,
    headSha: g('rev-parse', 'HEAD'),
  };

  expect(() => planBranch(root, next)).toThrow('checked out');

  await expect(prepareBranch(root, next)).rejects.toThrow('checked out');
});

test('describes create and unchanged without creating commits or refs', async () => {
  const refs = g('show-ref');
  const objects = g('count-objects', '-v');

  const planned = {
    ...plan,
    branch: planBranch(root, plan),
  };

  expect(planned.branch).toEqual({
    name: 'release/document',
    action: 'create',
    expectedSha: null,
  });

  expect(g('show-ref')).toBe(refs);
  expect(g('count-objects', '-v')).toBe(objects);

  const result = await prepareBranch(root, planned);

  if (!result) throw new Error('Expected a release branch');

  const updatedRefs = g('show-ref');
  const updatedObjects = g('count-objects', '-v');

  expect(planBranch(root, plan)).toEqual({
    name: 'release/document',
    action: 'unchanged',
    expectedSha: result.sha,
  });

  expect(g('show-ref')).toBe(updatedRefs);
  expect(g('count-objects', '-v')).toBe(updatedObjects);
});

test('rejects a changed branch between plan and apply', async () => {
  const initial = {
    ...plan,
    branch: planBranch(root, plan),
  };

  await prepareBranch(root, initial);

  const saved = {
    ...plan,
    branch: planBranch(root, plan),
  };

  const other = {
    ...plan,
    changes: plan.changes?.map((change) =>
      change.path.endsWith('CHANGELOG.md')
        ? {
            ...change,
            after: `${change.after}\nUpdated notes.\n`,
          }
        : change,
    ),
  };

  const updated = {
    ...other,
    branch: planBranch(root, other),
  };

  const result = await prepareBranch(root, updated);

  if (!result) throw new Error('Expected a release branch');

  await expect(prepareBranch(root, saved)).rejects.toThrow(
    'changed since planning',
  );

  expect(g('rev-parse', 'release/document')).toBe(result.sha);
});
