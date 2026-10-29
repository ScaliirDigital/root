import type { Author, Commit, ReleasePlan } from './types';

type GitHubApi = (endpoint: string) => Promise<unknown>;

type RemoteCommit = {
  author: {
    login: string;
    type: string;
  } | null;
};

type RemotePullRequest = {
  number: number;
  merged_at: string | null;
  merge_commit_sha: string | null;
  base: {
    ref: string;
    repo: { full_name: string };
  };
};

export async function githubApi(
  endpoint: string,
  method: 'GET' | 'POST' | 'PATCH' = 'GET',
  body?: unknown,
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

export async function enrichPlans(
  plans: ReleasePlan[],
  repository: string,
  api: GitHubApi = githubApi,
): Promise<ReleasePlan[]> {
  if (!/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(repository)) {
    throw new Error('Expected a GitHub repository in owner/name format');
  }

  const cache = new Map<string, Promise<unknown>>();

  function read<T>(endpoint: string): Promise<T> {
    let response = cache.get(endpoint);

    if (!response) {
      response = api(endpoint);
      cache.set(endpoint, response);
    }

    return response as Promise<T>;
  }

  const apiRoot = `repos/${repository}`;
  const webRoot = `https://github.com/${repository}`;
  const enriched: ReleasePlan[] = [];

  for (const plan of plans) {
    if (plan.commits.length === 0) {
      enriched.push(plan);
      continue;
    }

    const commits: Commit[] = [];
    const authors = new Map<string, Author>();

    for (const commit of plan.commits) {
      const endpoint = `${apiRoot}/commits/${commit.sha}`;

      const [remote, pulls] = await Promise.all([
        read<RemoteCommit>(endpoint),
        read<RemotePullRequest[]>(`${endpoint}/pulls?per_page=100`),
      ]);

      const candidates = pulls.filter(
        (pull) =>
          pull.merged_at !== null &&
          pull.base.ref === 'main' &&
          pull.base.repo.full_name.toLowerCase() === repository.toLowerCase(),
      );

      const exact = candidates.filter(
        (pull) => pull.merge_commit_sha === commit.sha,
      );

      const matching = exact.length > 0 ? exact : candidates;
      const pull = matching.length === 1 ? matching[0] : undefined;

      const author: Author | undefined = remote.author
        ? {
            login: remote.author.login,
            isBot:
              remote.author.type === 'Bot' ||
              remote.author.login.endsWith('[bot]'),
          }
        : undefined;

      if (author && !author.isBot) {
        authors.set(author.login.toLowerCase(), author);
      }

      commits.push({
        ...commit,
        url: `${webRoot}/commit/${commit.sha}`,
        author,
        pullRequest: pull
          ? {
              number: pull.number,
              url: `${webRoot}/pull/${pull.number}`,
            }
          : undefined,
      });
    }

    const newContributors: Author[] = [];

    for (const author of authors.values()) {
      const query = new URLSearchParams({
        sha: plan.baseSha,
        author: author.login,
        per_page: '1',
      });

      const previous = await read<unknown[]>(`${apiRoot}/commits?${query}`);

      if (previous.length === 0) {
        newContributors.push(author);
      }
    }

    enriched.push({
      ...plan,
      commits,
      newContributors,
    });
  }

  return enriched;
}
