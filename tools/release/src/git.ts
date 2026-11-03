import type { Commit } from './types';

type GitOptions = {
  cwd?: string;
  env?: Record<string, string | undefined>;
  input?: string;
};

export function git(...args: string[]): string;
export function git(options: GitOptions, ...args: string[]): string;
export function git(first: string | GitOptions, ...rest: string[]): string {
  const options: GitOptions = typeof first === 'string' ? {} : first;
  const args = typeof first === 'string' ? [first, ...rest] : rest;

  const result = Bun.spawnSync(['git', ...args], {
    cwd: options.cwd,
    env: { ...process.env, ...options.env },
    stdin: options.input === undefined ? undefined : Buffer.from(options.input),
    stdout: 'pipe',
    stderr: 'pipe',
  });

  if (result.exitCode !== 0) {
    throw new Error(result.stderr.toString().trim() || 'Git command failed');
  }

  return result.stdout.toString();
}

export function parseCommit(
  sha: string,
  message: string,
  scope: string,
): Commit | null {
  const [subject = ''] = message.split('\n');

  const match = /^([a-z][a-z0-9-]*)(?:\(([^()\r\n]+)\))?(!)?: (.+)$/.exec(
    subject,
  );

  if (!match || match[2] !== scope) {
    return null;
  }

  return {
    sha,
    type: match[1],
    description: match[4],
    breaking: Boolean(match[3]) || /^BREAKING[ -]CHANGE: /m.test(message),
  };
}

export function readCommits(
  base: string,
  head: string,
  scope: string,
): Commit[] {
  const hashes = git(
    'log',
    '--no-merges',
    '--reverse',
    '--format=%H',
    `${base}..${head}`,
  ).trim();

  if (!hashes) return [];

  const commits: Commit[] = [];

  for (const sha of hashes.split('\n')) {
    const message = git('show', '-s', '--format=%B', sha);
    const commit = parseCommit(sha, message, scope);

    if (commit) {
      commits.push(commit);
    }
  }

  return commits;
}

export function readGitFile(ref: string, path: string): string | null {
  const entry = git('ls-tree', '--name-only', ref, '--', path).trim();

  return entry ? git('show', `${ref}:${path}`) : null;
}
