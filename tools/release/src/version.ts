import type { Bump, Commit } from './types';

export function parseVersion(version: string): [number, number, number] {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) {
    throw new Error(`Expected a stable version, received: ${version}`);
  }

  const [major, minor, patch] = version.split('.').map(Number);

  if (![major, minor, patch].every(Number.isSafeInteger)) {
    throw new Error(`Version exceeds supported numeric range: ${version}`);
  }

  return [major, minor, patch];
}

export function nextVersion(version: string, bump: Bump): string {
  const [major, minor, patch] = parseVersion(version);

  switch (bump) {
    case 'major':
      return `${major + 1}.0.0`;
    case 'minor':
      return `${major}.${minor + 1}.0`;
    case 'patch':
      return `${major}.${minor}.${patch + 1}`;
  }
}

export function calculateBump(version: string, commits: Commit[]): Bump | null {
  const [major] = parseVersion(version);

  if (commits.some((commit) => commit.breaking)) {
    return major === 0 ? 'minor' : 'major';
  }

  if (commits.some((commit) => commit.type === 'feat')) {
    return major === 0 ? 'patch' : 'minor';
  }

  if (commits.some((commit) => commit.type === 'fix')) {
    return 'patch';
  }

  return null;
}
