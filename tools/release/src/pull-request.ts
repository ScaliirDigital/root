// tools/release/src/pull-request.ts
import type { BranchResult } from './branch';
import { renderChangelog } from './changelog';
import type { ReleasePlan } from './types';

type Request = (
  endpoint: string,
  method: 'GET' | 'POST' | 'PATCH',
  body?: Record<string, string>,
) => Promise<unknown>;

type RemotePullRequest = {
  number: number;
  html_url: string;
  title: string;
  body: string | null;
  state: string;
  head: {
    ref: string;
    sha: string;
    repo: { full_name: string } | null;
  };
  base: {
    ref: string;
    repo: { full_name: string };
  };
};

export type PullRequestResult = {
  number: number;
  url: string;
  action: 'created' | 'updated' | 'unchanged';
};

async function request(
  endpoint: string,
  method: 'GET' | 'POST' | 'PATCH',
  body?: Record<string, string>,
): Promise<unknown> {
  const args = [
    'gh',
    'api',
    '--hostname',
    'github.com',
    '--method',
    method,
    endpoint,
  ];

  if (body !== undefined) {
    args.push('--input', '-');
  }

  const child = Bun.spawn(args, {
    stdin: body === undefined ? 'ignore' : Buffer.from(JSON.stringify(body)),
    stdout: 'pipe',
    stderr: 'pipe',
  });

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);

  if (exitCode !== 0) {
    throw new Error(stderr.trim() || `GitHub request failed: ${endpoint}`);
  }

  return JSON.parse(stdout);
}

export async function upsertPullRequest(
  repository: string,
  plan: ReleasePlan,
  branch: BranchResult,
  api: Request = request,
): Promise<PullRequestResult> {
  if (!/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(repository)) {
    throw new Error('Expected a GitHub repository in owner/name format');
  }

  if (
    plan.nextVersion === null ||
    !/^[a-z][a-z0-9-]*$/.test(plan.package) ||
    branch.branch !== `release/${plan.package}`
  ) {
    throw new Error('Expected a matching package release branch');
  }

  const owner = repository.split('/')[0];
  const endpoint = `repos/${repository}/pulls`;
  const marker = `<!-- scale-release:v1 package=${plan.package} -->`;
  const title = `chore(${plan.package}): release ${plan.nextVersion}`;
  const body = `${marker}\n\n${renderChangelog(plan, { availability: false })}`;
  const matches: RemotePullRequest[] = [];

  for (let page = 1; ; page += 1) {
    const query = new URLSearchParams({
      state: 'open',
      base: 'main',
      head: `${owner}:${branch.branch}`,
      per_page: '100',
      page: String(page),
    });

    const pulls = (await api(
      `${endpoint}?${query}`,
      'GET',
    )) as RemotePullRequest[];

    if (!Array.isArray(pulls)) {
      throw new Error('Expected a pull request list from GitHub');
    }

    matches.push(
      ...pulls.filter(
        (pull) =>
          pull.state === 'open' &&
          pull.head.ref === branch.branch &&
          pull.head.repo?.full_name.toLowerCase() ===
            repository.toLowerCase() &&
          pull.base.ref === 'main' &&
          pull.base.repo.full_name.toLowerCase() === repository.toLowerCase(),
      ),
    );

    if (pulls.length < 100) break;
  }

  if (matches.length > 1) {
    throw new Error(`${branch.branch}: multiple matching release PRs`);
  }

  let existing = matches[0];

  if (existing && existing.head.sha !== branch.sha) {
    const number = existing.number;

    for (const delay of [200, 400, 800]) {
      await Bun.sleep(delay);

      const response = await api(`${endpoint}/${number}`, 'GET');
      const refreshed = response as RemotePullRequest;

      if (!refreshed?.head || refreshed.number !== number) {
        throw new Error(
          `${branch.branch}: invalid response while refreshing PR head`,
        );
      }

      existing = refreshed;

      if (existing.head.sha === branch.sha) break;
    }
  }

  if (existing) {
    if (
      existing.state !== 'open' ||
      existing.head.ref !== branch.branch ||
      existing.head.repo?.full_name.toLowerCase() !==
        repository.toLowerCase() ||
      existing.base.ref !== 'main' ||
      existing.base.repo.full_name.toLowerCase() !== repository.toLowerCase()
    ) {
      throw new Error(
        `${branch.branch}: release PR changed during preparation`,
      );
    }

    if (existing.body?.split(/\r?\n/, 1)[0] !== marker) {
      throw new Error(`${branch.branch}: refusing to update an unmanaged PR`);
    }

    if (existing.head.sha !== branch.sha) {
      throw new Error(
        `${branch.branch}: PR head does not match release commit`,
      );
    }

    if (existing.title === title && existing.body === body) {
      return {
        number: existing.number,
        url: existing.html_url,
        action: 'unchanged',
      };
    }

    await api(`${endpoint}/${existing.number}`, 'PATCH', {
      title,
      body,
    });

    return {
      number: existing.number,
      url: existing.html_url,
      action: 'updated',
    };
  }

  const created = (await api(endpoint, 'POST', {
    title,
    body,
    head: branch.branch,
    base: 'main',
  })) as RemotePullRequest;

  if (
    !Number.isSafeInteger(created.number) ||
    created.number <= 0 ||
    typeof created.html_url !== 'string'
  ) {
    throw new Error('GitHub returned an invalid release PR');
  }

  return {
    number: created.number,
    url: created.html_url,
    action: 'created',
  };
}
