// tools/release/tests/remote-branch.test.ts
import { afterEach, beforeEach, expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { planBranch, prepareBranch } from '../src/branch';
import { git } from '../src/git';
import {
  adoptRemoteBranch,
  fetchRemoteBranch,
  readRemoteBranch,
} from '../src/remote';
import type { ReleasePlan } from '../src/types';

let directory: string;
let source: string;
let checkout: string;
let remote: string;
let plan: ReleasePlan;

const cargo = '[package]\nname = "document"\nversion = "0.1.0"\n';
const branch = 'release/document';

function runGit(root: string, ...args: string[]): string {
  return git(
    { cwd: root },
    '-c',
    'core.hooksPath=/dev/null',
    '-c',
    'commit.gpgSign=false',
    ...args,
  ).trim();
}

function configureIdentity(root: string) {
  runGit(root, 'config', 'user.name', 'Release Test');
  runGit(root, 'config', 'user.email', 'release@example.invalid');
}

function pushToRemote(root: string, sha: string) {
  runGit(root, 'push', remote, `${sha}:refs/heads/${branch}`);
}

function cloneMain() {
  git(
    'clone',
    '--quiet',
    '--single-branch',
    '--branch',
    'main',
    remote,
    checkout,
  );
}

beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), 'release-checkout-'));
  source = join(directory, 'source');
  checkout = join(directory, 'checkout');
  remote = join(directory, 'remote.git');

  git('init', '--quiet', '--bare', '--initial-branch=main', remote);
  git('init', '--quiet', '--initial-branch=main', source);

  configureIdentity(source);

  await Bun.write(join(source, 'packages/document/Cargo.toml'), cargo);

  runGit(source, 'add', '.');
  runGit(source, 'commit', '--quiet', '-m', 'chore(root): initialize fixture');

  const headSha = runGit(source, 'rev-parse', 'HEAD');

  runGit(source, 'push', remote, 'main');

  plan = {
    package: 'document',
    language: 'rust',
    targets: ['cargo'],
    containers: ['ghcr'],
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
    ],
  };
});

afterEach(async () => {
  await rm(directory, { recursive: true, force: true });
});

test('continues an existing release from a fresh checkout', async () => {
  const created = await prepareBranch(source, {
    ...plan,
    branch: planBranch(source, plan),
  });

  if (!created) throw new Error('Expected a release branch');

  pushToRemote(source, created.sha);

  cloneMain();
  configureIdentity(checkout);

  expect(
    runGit(
      checkout,
      'for-each-ref',
      '--format=%(refname)',
      'refs/heads/release/',
    ),
  ).toBe('');

  const state = readRemoteBranch(checkout, remote, branch);

  fetchRemoteBranch(checkout, state);

  const before = {
    head: runGit(checkout, 'rev-parse', 'HEAD'),
    refs: runGit(checkout, 'show-ref'),
    status: runGit(checkout, 'status', '--porcelain'),
    cargo: await Bun.file(
      join(checkout, 'packages/document/Cargo.toml'),
    ).text(),
  };

  const preview = planBranch(checkout, plan, {
    sha: state.expectedSha,
  });

  expect(preview).toEqual({
    name: branch,
    action: 'unchanged',
    expectedSha: created.sha,
  });

  expect({
    head: runGit(checkout, 'rev-parse', 'HEAD'),
    refs: runGit(checkout, 'show-ref'),
    status: runGit(checkout, 'status', '--porcelain'),
    cargo: await Bun.file(
      join(checkout, 'packages/document/Cargo.toml'),
    ).text(),
  }).toEqual(before);

  adoptRemoteBranch(checkout, state, plan);

  const repeated = await prepareBranch(checkout, {
    ...plan,
    branch: planBranch(checkout, plan),
  });

  if (!repeated) throw new Error('Expected a release branch');

  expect(repeated).toEqual({
    branch,
    sha: created.sha,
    changed: false,
  });

  expect(readRemoteBranch(checkout, remote, branch).expectedSha).toBe(
    created.sha,
  );

  expect(runGit(checkout, 'rev-parse', 'HEAD')).toBe(before.head);
  expect(runGit(checkout, 'status', '--porcelain')).toBe(before.status);

  expect(
    await Bun.file(join(checkout, 'packages/document/Cargo.toml')).text(),
  ).toBe(before.cargo);
});

test('rejects an unmanaged remote branch before adopting it', () => {
  runGit(source, 'push', remote, `main:refs/heads/${branch}`);

  cloneMain();

  const state = readRemoteBranch(checkout, remote, branch);

  fetchRemoteBranch(checkout, state);

  expect(() => adoptRemoteBranch(checkout, state, plan)).toThrow('unmanaged');

  expect(
    runGit(
      checkout,
      'for-each-ref',
      '--format=%(refname)',
      `refs/heads/${branch}`,
    ),
  ).toBe('');
});

test('preserves a differing local release branch', async () => {
  const result = await prepareBranch(source, {
    ...plan,
    branch: planBranch(source, plan),
  });

  if (!result) throw new Error('Expected a release branch');

  pushToRemote(source, result.sha);

  cloneMain();

  runGit(checkout, 'branch', branch);

  const localSha = runGit(checkout, 'rev-parse', branch);
  const observed = readRemoteBranch(checkout, remote, branch);

  fetchRemoteBranch(checkout, observed);

  expect(() => adoptRemoteBranch(checkout, observed, plan)).toThrow(
    'local and remote branches differ',
  );

  expect(runGit(checkout, 'rev-parse', branch)).toBe(localSha);
});
