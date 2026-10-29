import { planBranch } from './branch';
import { git } from './git';
import type { ReleasePlan } from './types';

export type RemoteBranch = {
  remote: string;
  branch: string;
  expectedSha: string | null;
};

function validate(remote: string, branch: string) {
  if (!remote || remote.startsWith('-') || /[\0\r\n]/.test(remote)) {
    throw new Error('Invalid release remote');
  }

  if (!/^release\/[a-z][a-z0-9-]*$/.test(branch)) {
    throw new Error('Invalid release branch');
  }
}

export function readRemoteBranch(
  root: string,
  remote: string,
  branch: string,
): RemoteBranch {
  validate(remote, branch);

  const ref = `refs/heads/${branch}`;

  const output = git({ cwd: root }, 'ls-remote', '--refs', remote, ref).trim();

  const entries = output
    .split('\n')
    .filter(Boolean)
    .map((line) => line.split('\t'))
    .filter((entry) => entry[1] === ref);

  if (entries.length > 1) {
    throw new Error(`${branch}: multiple remote refs found`);
  }

  const expectedSha = entries[0]?.[0] ?? null;

  if (
    expectedSha !== null &&
    !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(expectedSha)
  ) {
    throw new Error(`${branch}: invalid remote commit`);
  }

  return { remote, branch, expectedSha };
}

export function fetchRemoteBranch(root: string, state: RemoteBranch): void {
  validate(state.remote, state.branch);

  if (state.expectedSha === null) return;

  git(
    { cwd: root },
    'fetch',
    '--no-tags',
    '--no-write-fetch-head',
    state.remote,
    state.expectedSha,
  );

  const commit = git(
    { cwd: root },
    'rev-parse',
    '--verify',
    `${state.expectedSha}^{commit}`,
  ).trim();

  if (commit !== state.expectedSha) {
    throw new Error(`${state.branch}: expected a remote commit`);
  }
}

export function adoptRemoteBranch(
  root: string,
  state: RemoteBranch,
  plan: ReleasePlan,
): void {
  validate(state.remote, state.branch);

  if (state.branch !== `release/${plan.package}`) {
    throw new Error('Remote branch does not match the release package');
  }

  if (state.expectedSha === null) return;

  // Prüft Kennzeichnung, Commit-Struktur und Release-Baseline,
  // bevor ein lokaler Branch angelegt wird.
  const remotePlan = planBranch(root, plan, {
    sha: state.expectedSha,
  });

  if (!remotePlan) {
    throw new Error('Expected an active release plan');
  }

  const ref = `refs/heads/${state.branch}`;

  const entry = git(
    { cwd: root },
    'for-each-ref',
    '--format=%(refname) %(objectname) %(symref)',
    ref,
  )
    .trim()
    .split('\n')
    .find((line) => line.startsWith(`${ref} `));

  const [, localSha, symbolic] = entry?.split(' ') ?? [];

  if (symbolic) {
    throw new Error(`${state.branch}: symbolic branches are not supported`);
  }

  if (localSha && localSha !== state.expectedSha) {
    throw new Error(
      `${state.branch}: local and remote branches differ; refusing to overwrite`,
    );
  }

  const transaction = localSha
    ? `verify ${ref} ${state.expectedSha}`
    : `create ${ref} ${state.expectedSha}`;

  git(
    {
      cwd: root,
      input: [
        'start',
        `verify HEAD ${plan.headSha}`,
        transaction,
        'prepare',
        'commit',
        '',
      ].join('\n'),
    },
    'update-ref',
    '--stdin',
  );
}
