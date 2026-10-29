import { afterEach, beforeEach, expect, test } from 'bun:test';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { git } from '../src/git';
import { fetchRemoteBranch, readRemoteBranch } from '../src/remote';

let directory: string;
let root: string;
let remote: string;
let initialSha: string;

const branch = 'release/document';
const ref = `refs/heads/${branch}`;

function g(...args: string[]): string {
  return git(
    { cwd: root },
    '-c',
    'core.hooksPath=/dev/null',
    '-c',
    'commit.gpgSign=false',
    ...args,
  ).trim();
}

function nextCommit(parent: string, message: string): string {
  const tree = g('rev-parse', `${parent}^{tree}`);

  return g('commit-tree', tree, '-p', parent, '-m', message);
}

beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), 'release-remote-'));
  root = join(directory, 'repository');
  remote = join(directory, 'remote.git');

  git('init', '--quiet', '--bare', remote);
  git('init', '--quiet', '--initial-branch=main', root);

  g('config', 'user.name', 'Release Test');
  g('config', 'user.email', 'release@example.invalid');

  await Bun.write(join(root, 'README.md'), '# Release test\n');

  g('add', '.');
  g('commit', '--quiet', '-m', 'chore(root): initialize fixture');

  initialSha = g('rev-parse', 'HEAD');

  g('branch', branch);
});

afterEach(async () => {
  await rm(directory, { recursive: true, force: true });
});

test('fetches a remote commit without changing local branches or files', () => {
  const next = nextCommit(initialSha, 'chore(document): release 0.1.1');

  g('push', remote, `${next}:${ref}`);

  const state = readRemoteBranch(root, remote, branch);
  const before = {
    head: g('rev-parse', 'HEAD'),
    refs: g('show-ref'),
    status: g('status', '--porcelain'),
  };

  fetchRemoteBranch(root, state);

  expect({
    head: g('rev-parse', 'HEAD'),
    refs: g('show-ref'),
    status: g('status', '--porcelain'),
  }).toEqual(before);

  expect(g('rev-parse', `${next}^{commit}`)).toBe(next);
});
