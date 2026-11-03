import { readPackages } from './config';
import { git } from './git';
import { githubApi } from './github';
import type { PackageConfig } from './types';
import { nextVersion } from './version';

type GitHubApi = (endpoint: string) => Promise<unknown>;

type MergedPullRequest = {
  number: number;
  body: string | null;
  merged_at: string | null;
  merge_commit_sha: string | null;
  head: {
    ref: string;
    repo: { full_name: string } | null;
  };
  base: {
    ref: string;
    repo: { full_name: string };
  };
};

export type Release = {
  package: string;
  path: string;
  version: string;
  tag: string;
  sha: string;
  pullRequest: number;
  notes: string;
  targets: PackageConfig['targets'];
  containers: PackageConfig['containers'];
};

export function releaseNotes(
  changelog: string,
  version: string,
  packageName?: string,
): string {
  const lines = changelog.split(/\r?\n/);
  const heading = packageName
    ? `## ${packageName} ${version}`
    : `## ${version}`;

  const start = lines.findIndex((line) => line.trim() === heading);

  if (start === -1) {
    throw new Error(`Missing changelog section for ${version}`);
  }

  let end = lines.length;

  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index].startsWith('## ')) {
      end = index;
      break;
    }
  }

  const notes = lines
    .slice(start + 1, end)
    .join('\n')
    .trim();

  if (!notes) {
    throw new Error(`Empty release notes for ${version}`);
  }

  return `${notes}\n`;
}

function manifestVersion(
  content: string,
  manifest: PackageConfig['manifest'],
): string {
  const data =
    manifest === 'package.json' ? JSON.parse(content) : Bun.TOML.parse(content);

  const version =
    manifest === 'package.json' ? data.version : data.package?.version;

  if (typeof version !== 'string') {
    throw new Error(`Missing version in ${manifest}`);
  }

  // Verwendet dieselbe Stable-Version-Prüfung wie die Release-Planung.
  nextVersion(version, 'patch');

  return version;
}

export async function detectReleases(
  root: string,
  repository: string,
  api: GitHubApi = githubApi,
): Promise<Release[]> {
  if (!/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(repository)) {
    throw new Error('Expected a GitHub repository in owner/name format');
  }

  const inspect = (...args: string[]) => git({ cwd: root }, ...args).trim();

  const sha = inspect('rev-parse', 'HEAD');
  const packages = await readPackages(root);
  const associated: MergedPullRequest[] = [];

  for (let page = 1; ; page += 1) {
    const response = await api(
      `repos/${repository}/commits/${sha}/pulls?per_page=100&page=${page}`,
    );

    if (!Array.isArray(response)) {
      throw new Error('Expected a pull request list from GitHub');
    }

    associated.push(...(response as MergedPullRequest[]));

    if (response.length < 100) break;
  }

  const releases: Release[] = [];

  for (const [name, pkg] of Object.entries(packages)) {
    const marker = `<!-- scale-release:v1 package=${name} -->`;

    const matches = associated.filter(
      (pull) =>
        pull.merged_at !== null &&
        pull.merge_commit_sha === sha &&
        pull.head.ref === `release/${name}` &&
        pull.head.repo?.full_name.toLowerCase() === repository.toLowerCase() &&
        pull.base.ref === 'main' &&
        pull.base.repo.full_name.toLowerCase() === repository.toLowerCase() &&
        pull.body?.split(/\r?\n/, 1)[0] === marker,
    );

    if (matches.length === 0) continue;

    if (matches.length !== 1) {
      throw new Error(`${name}: multiple matching merged release PRs`);
    }

    const pull = matches[0];

    if (!pull) {
      throw new Error(`${name}: missing merged release PR`);
    }

    const manifestPath = `${pkg.path}/${pkg.manifest}`;
    const parent = inspect('rev-parse', `${sha}^1`);

    const before = git({ cwd: root }, 'show', `${parent}:${manifestPath}`);

    const after = git({ cwd: root }, 'show', `${sha}:${manifestPath}`);

    const previousVersion = manifestVersion(before, pkg.manifest);
    const version = manifestVersion(after, pkg.manifest);

    const allowedVersions = [
      nextVersion(previousVersion, 'patch'),
      nextVersion(previousVersion, 'minor'),
      nextVersion(previousVersion, 'major'),
    ];

    if (!allowedVersions.includes(version)) {
      throw new Error(
        `${name}: invalid release version transition ${previousVersion} -> ${version}`,
      );
    }

    const changelogPath = `${pkg.path}/CHANGELOG.md`;

    const entry = inspect(
      '--literal-pathspecs',
      'ls-tree',
      '--name-only',
      sha,
      '--',
      changelogPath,
    );

    if (!entry) {
      throw new Error(`${name}: missing committed changelog`);
    }

    const changelog = git({ cwd: root }, 'show', `${sha}:${changelogPath}`);

    releases.push({
      package: name,
      path: pkg.path,
      version,
      tag: `${pkg.tag_prefix}${version}`,
      sha,
      pullRequest: pull.number,
      notes: releaseNotes(changelog, version, name),
      targets: pkg.targets,
      containers: pkg.containers,
    });
  }

  return releases;
}
