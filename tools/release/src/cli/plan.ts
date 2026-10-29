import { planBranch } from '../branch';
import { prependRelease, renderChangelog } from '../changelog';
import { readPackages, readVersion } from '../config';
import { git, readCommits, readGitFile } from '../git';
import { enrichPlans } from '../github';
import { planLockfile } from '../lockfile';
import { bumpManifest } from '../manifest';
import { publicationLinks } from '../publications';
import {
  fetchRemoteBranch,
  type RemoteBranch,
  readRemoteBranch,
} from '../remote';
import type { FileChange, ReleasePlan } from '../types';
import { calculateBump, nextVersion } from '../version';

export async function planRelease(github: boolean) {
  const root = git('rev-parse', '--show-toplevel').trim();
  const head = git('rev-parse', 'HEAD').trim();

  let repository: string | undefined;
  let remote: string | undefined;

  if (github) {
    repository = process.env.GH_REPO ?? process.env.GITHUB_REPOSITORY;

    if (!repository || !/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(repository)) {
      throw new Error('--github requires GH_REPO=owner/name');
    }

    remote = `https://github.com/${repository}.git`;
  }

  const packages = await readPackages(root);
  const plans: ReleasePlan[] = [];

  for (const [name, pkg] of Object.entries(packages)) {
    const version = await readVersion(root, pkg);

    const tags = git(
      'tag',
      '--merged',
      head,
      '--list',
      `${pkg.tag_prefix}*`,
      '--sort=-version:refname',
    );

    const tag = tags
      .split('\n')
      .find((candidate) =>
        /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(
          candidate.slice(pkg.tag_prefix.length),
        ),
      );

    if (!tag) {
      throw new Error(`${name}: no reachable stable release tag found`);
    }

    if (tag !== `${pkg.tag_prefix}${version}`) {
      throw new Error(
        `${name}: manifest version ${version} does not match latest tag ${tag}`,
      );
    }

    const base = git('rev-parse', `${tag}^{commit}`).trim();
    const commits = readCommits(base, head, pkg.scope);
    const bump = calculateBump(version, commits);
    const upcomingVersion = bump ? nextVersion(version, bump) : null;
    const changes: FileChange[] = [];

    if (upcomingVersion !== null) {
      const path = `${pkg.path}/${pkg.manifest}`;
      const before = git('show', `${head}:${path}`);
      const after = bumpManifest(
        before,
        pkg.manifest,
        version,
        upcomingVersion,
      );

      changes.push({ path, before, after });

      const lockChange = await planLockfile(root, head, pkg, after);

      if (lockChange) {
        changes.push(lockChange);
      }
    }

    const publications =
      upcomingVersion === null
        ? []
        : publicationLinks(
            name,
            pkg,
            git('show', `${head}:${pkg.path}/${pkg.manifest}`),
            upcomingVersion,
            repository,
          );

    plans.push({
      package: name,
      language: pkg.language,
      targets: pkg.targets,
      containers: pkg.containers,
      baseTag: tag,
      baseSha: base,
      headSha: head,
      currentVersion: version,
      nextVersion: upcomingVersion,
      bump,
      commits,
      changes,
      publications,
    });
  }

  const outputPlans = repository ? await enrichPlans(plans, repository) : plans;

  const remoteStates = new Map<string, RemoteBranch>();

  for (const plan of outputPlans) {
    if (plan.nextVersion === null) {
      plan.branch = null;
      continue;
    }

    const pkg = packages[plan.package];
    const path = `${pkg.path}/CHANGELOG.md`;

    const before = readGitFile(plan.headSha, path);
    const after = prependRelease(before, renderChangelog(plan));

    if (after !== before) {
      plan.changes ??= [];
      plan.changes.push({ path, before, after });
    }

    plan.branch = planBranch(root, plan);

    if (remote && plan.branch) {
      const state = readRemoteBranch(root, remote, plan.branch.name);

      remoteStates.set(plan.package, state);
      fetchRemoteBranch(root, state);

      if (state.expectedSha !== null) {
        plan.branch = planBranch(root, plan, {
          sha: state.expectedSha,
        });
      }
    }
  }

  return {
    root,
    head,
    repository,
    remote,
    plans: outputPlans,
    remoteStates,
  };
}

export async function plan(github: boolean, markdown: boolean) {
  const release = await planRelease(github);

  if (!markdown) {
    const output = release.plans.map((plan) => ({
      ...plan,
      changes: (plan.changes ?? []).map(({ path }) => path),
    }));

    console.log(JSON.stringify(output, null, 2));
    return;
  }

  const previews = release.plans
    .map((plan) => renderChangelog(plan))
    .filter(Boolean);

  if (previews.length > 0) {
    console.log(previews.join('\n'));
  }
}
