import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { git } from './git';
import type { Committer, Entry } from './signed-commit';
import type { BranchPlan, ReleasePlan } from './types';

export type BranchResult = {
  branch: string;
  sha: string;
  changed: boolean;
};

function readEntry(root: string, ref: string, path: string) {
  const options = { cwd: root };

  const entry = git(
    options,
    '--literal-pathspecs',
    'ls-tree',
    ref,
    '--',
    path,
  ).trim();

  const mode = entry ? entry.split(' ')[0] : '100644';

  if (mode !== '100644' && mode !== '100755') {
    throw new Error(`${path}: expected a regular file`);
  }

  return {
    mode,
    content: entry ? git(options, 'show', `${ref}:${path}`) : null,
  };
}

function messageFor(plan: ReleasePlan, tree: string): string {
  return `chore(${plan.package}): release ${plan.nextVersion}\n\nRelease: v1\nRelease-Base: ${plan.headSha}\nRelease-Tree: ${tree}`;
}

export function planBranch(
  root: string,
  plan: ReleasePlan,
  existing?: { sha: string | null },
): BranchPlan | null {
  if (plan.nextVersion === null) return null;

  if (!/^[a-z][a-z0-9-]*$/.test(plan.package)) {
    throw new Error('Invalid package name for a release branch');
  }

  if (!plan.changes?.length) {
    throw new Error(`${plan.package}: release plan contains no file changes`);
  }

  const name = `release/${plan.package}`;
  const ref = `refs/heads/${name}`;

  const inspect = (...args: string[]) => git({ cwd: root }, ...args).trim();

  inspect('check-ref-format', ref);

  if (inspect('rev-parse', 'HEAD') !== plan.headSha) {
    throw new Error('HEAD changed since the release plan was created');
  }

  const worktrees = inspect('worktree', 'list', '--porcelain');

  if (worktrees.split('\n').includes(`branch ${ref}`)) {
    throw new Error(`${name}: branch is checked out; refusing to update it`);
  }

  const paths = new Set<string>();

  for (const change of plan.changes) {
    const path = change.path;

    const parts = path.split('/');
    const forbiddenCharacters = ['\\', '\0', '\r', '\n'];
    const forbiddenParts = ['', '.', '..', '.git'];

    if (!path || path.startsWith('/') || path.startsWith('-')) {
      throw new Error(`Expected a relative release path: ${path}`);
    }

    if (forbiddenCharacters.some((character) => path.includes(character))) {
      throw new Error(`Unsupported character in release path: ${path}`);
    }

    if (parts.some((part) => forbiddenParts.includes(part.toLowerCase()))) {
      throw new Error(`Invalid segment in release path: ${path}`);
    }

    if (paths.has(path)) {
      throw new Error(`Duplicate release path: ${path}`);
    }

    paths.add(path);

    if (readEntry(root, plan.headSha, path).content !== change.before) {
      throw new Error(`${path}: file does not match the release plan`);
    }
  }

  const entry = inspect(
    'for-each-ref',
    '--format=%(refname) %(objectname) %(symref)',
    ref,
  )
    .split('\n')
    .find((line) => line.startsWith(`${ref} `));

  const [, localSha, symbolic] = entry?.split(' ') ?? [];
  const previous = existing === undefined ? localSha : existing.sha;

  if (symbolic) {
    throw new Error(`${name}: symbolic branches are not supported`);
  }

  if (!previous) {
    return { name, action: 'create', expectedSha: null };
  }

  const message = inspect('show', '-s', '--format=%B', previous);

  const marker =
    /^chore\(([a-z][a-z0-9-]*)\): release ([^\n]+)\n\nRelease: v1\nRelease-Base: ([0-9a-f]+)\nRelease-Tree: ([0-9a-f]+)$/.exec(
      message,
    );

  const tree = inspect('rev-parse', `${previous}^{tree}`);

  if (
    !marker ||
    marker[1] !== plan.package ||
    inspect('show', '-s', '--format=%P', previous) !== marker[3] ||
    tree !== marker[4]
  ) {
    throw new Error(`${name}: unmanaged or manually changed branch`);
  }

  if (inspect('merge-base', marker[3], plan.headSha) !== marker[3]) {
    throw new Error(
      `${name}: release baseline is not an ancestor of the new plan`,
    );
  }

  let unchanged = false;

  if (message === messageFor(plan, tree)) {
    const actualPaths = git(
      { cwd: root },
      'diff-tree',
      '--no-commit-id',
      '--name-only',
      '-r',
      '-z',
      plan.headSha,
      previous,
    )
      .split('\0')
      .filter(Boolean)
      .sort();

    const expectedPaths = plan.changes
      .filter((change) => change.before !== change.after)
      .map((change) => change.path)
      .sort();

    unchanged =
      actualPaths.join('\0') === expectedPaths.join('\0') &&
      plan.changes.every((change) => {
        const before = readEntry(root, plan.headSha, change.path);
        const existing = readEntry(root, previous, change.path);

        return (
          existing.content === change.after && existing.mode === before.mode
        );
      });
  }

  return {
    name,
    action: unchanged ? 'unchanged' : 'update',
    expectedSha: previous,
  };
}

export async function prepareBranch(
  root: string,
  plan: ReleasePlan,
  commit?: Committer,
): Promise<BranchResult | null> {
  if (plan.nextVersion === null) return null;

  if (!plan.branch) {
    throw new Error(`${plan.package}: missing branch plan`);
  }

  const actual = planBranch(root, plan);

  if (!actual) return null;

  const ref = `refs/heads/${actual.name}`;
  const options = { cwd: root };

  if (actual.action === 'unchanged' && actual.expectedSha) {
    git(
      {
        ...options,
        input: `start\nverify HEAD ${plan.headSha}\nverify ${ref} ${actual.expectedSha}\nprepare\ncommit\n`,
      },
      'update-ref',
      '--stdin',
    );

    return {
      branch: actual.name,
      sha: actual.expectedSha,
      changed: false,
    };
  }

  if (
    actual.name !== plan.branch.name ||
    actual.action !== plan.branch.action ||
    actual.expectedSha !== plan.branch.expectedSha
  ) {
    throw new Error(`${actual.name}: branch changed since planning`);
  }

  const temporary = await mkdtemp(join(tmpdir(), 'release-index-'));

  const indexOptions = {
    ...options,
    env: {
      GIT_INDEX_FILE: join(temporary, 'index'),
    },
  };

  const run = (...args: string[]) => git(indexOptions, ...args).trim();

  try {
    run('read-tree', plan.headSha);

    const entries: Entry[] = [];

    for (const change of plan.changes ?? []) {
      const { mode } = readEntry(root, plan.headSha, change.path);

      const blob = git(
        { ...indexOptions, input: change.after },
        'hash-object',
        '-w',
        '--stdin',
      ).trim();

      run(
        'update-index',
        '--add',
        '--cacheinfo',
        `${mode},${blob},${change.path}`,
      );

      entries.push({ path: change.path, mode, content: change.after });
    }

    const tree = run('write-tree');
    const message = messageFor(plan, tree);

    const sha = commit
      ? await commit({
          tree,
          message,
          previous: actual.expectedSha,
          entries,
        })
      : run(
          '-c',
          'commit.gpgSign=false',
          'commit-tree',
          tree,
          '-p',
          plan.headSha,
          '-m',
          message,
        );

    const expected = actual.expectedSha ?? '0'.repeat(sha.length);

    git(
      {
        ...indexOptions,
        input: `start\nverify HEAD ${plan.headSha}\nupdate ${ref} ${sha} ${expected}\nprepare\ncommit\n`,
      },
      'update-ref',
      '--stdin',
    );

    return {
      branch: actual.name,
      sha,
      changed: true,
    };
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
