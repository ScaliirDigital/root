// tools/release/tests/pull-request.test.ts
import { expect, test } from 'bun:test';
import { renderChangelog } from '../src/changelog';
import { upsertPullRequest } from '../src/pull-request';
import type { ReleasePlan } from '../src/types';

const repository = 'example/root';

const plan: ReleasePlan = {
  package: 'document',
  language: 'rust',
  targets: ['cargo'],
  containers: ['ghcr'],
  baseTag: 'document-v0.1.0',
  baseSha: 'a'.repeat(40),
  headSha: 'b'.repeat(40),
  currentVersion: '0.1.0',
  nextVersion: '0.1.1',
  bump: 'patch',
  commits: [
    {
      sha: 'c'.repeat(40),
      type: 'fix',
      description: 'handle empty input',
      breaking: false,
    },
  ],
  publications: [
    {
      category: 'packages',
      label: 'crates.io',
      url: 'https://crates.io/crates/document/0.1.1',
      logo: 'rust',
      color: 'F46623',
    },
  ],
};

const branch = {
  branch: 'release/document',
  sha: 'd'.repeat(40),
  changed: true,
};

const marker = '<!-- scale-release:v1 package=document -->';
const title = 'chore(document): release 0.1.1';
const body = `${marker}\n\n${renderChangelog(plan, { availability: false })}`;

function pull(overrides: Record<string, unknown> = {}) {
  return {
    number: 42,
    html_url: 'https://github.com/example/root/pull/42',
    title,
    body,
    state: 'open',
    head: {
      ref: branch.branch,
      sha: branch.sha,
      repo: { full_name: repository },
    },
    base: {
      ref: 'main',
      repo: { full_name: repository },
    },
    ...overrides,
  };
}

test('creates a release PR with the generated changelog', async () => {
  const calls: Array<{
    endpoint: string;
    method: string;
    body?: Record<string, string>;
  }> = [];

  const result = await upsertPullRequest(
    repository,
    plan,
    branch,
    async (endpoint, method, payload) => {
      calls.push({ endpoint, method, body: payload });

      return method === 'GET' ? [] : pull();
    },
  );

  expect(result).toEqual({
    number: 42,
    url: 'https://github.com/example/root/pull/42',
    action: 'created',
  });

  expect(calls[0]?.method).toBe('GET');

  expect(calls[1]).toEqual({
    endpoint: 'repos/example/root/pulls',
    method: 'POST',
    body: {
      title,
      body,
      head: 'release/document',
      base: 'main',
    },
  });
});

test('keeps the availability table out of the PR body', async () => {
  let created: Record<string, string> | undefined;

  await upsertPullRequest(
    repository,
    plan,
    branch,
    async (_endpoint, method, payload) => {
      if (method === 'POST') created = payload;

      return method === 'GET' ? [] : pull();
    },
  );

  expect(created?.body).toContain('### Changes');
  expect(created?.body).not.toContain('### Available on');
  expect(created?.body).not.toContain('crates.io');
});

test('leaves an identical release PR unchanged', async () => {
  const methods: string[] = [];

  const result = await upsertPullRequest(
    repository,
    plan,
    branch,
    async (_endpoint, method) => {
      methods.push(method);
      return [pull()];
    },
  );

  expect(result.action).toBe('unchanged');
  expect(methods).toEqual(['GET']);
});

test('updates a managed release PR', async () => {
  const calls: Array<{
    method: string;
    body?: Record<string, string>;
  }> = [];

  const result = await upsertPullRequest(
    repository,
    plan,
    branch,
    async (_endpoint, method, payload) => {
      calls.push({ method, body: payload });

      return method === 'GET'
        ? [pull({ body: `${marker}\n\nPrevious notes` })]
        : pull();
    },
  );

  expect(result.action).toBe('updated');

  expect(calls[1]).toEqual({
    method: 'PATCH',
    body: { title, body },
  });
});

test('refuses to overwrite an unmanaged PR', async () => {
  const methods: string[] = [];

  await expect(
    upsertPullRequest(repository, plan, branch, async (_endpoint, method) => {
      methods.push(method);
      return [pull({ body: 'Manually created PR' })];
    }),
  ).rejects.toThrow('unmanaged PR');

  expect(methods).toEqual(['GET']);
});

test('refuses a PR pointing to a different commit', async () => {
  const mismatched = pull({
    head: {
      ref: branch.branch,
      sha: 'e'.repeat(40),
      repo: { full_name: repository },
    },
  });

  await expect(
    upsertPullRequest(repository, plan, branch, async (endpoint) =>
      endpoint.includes('?') ? [mismatched] : mismatched,
    ),
  ).rejects.toThrow('PR head does not match');
});

test('refreshes a stale PR head before updating', async () => {
  let refreshed = false;
  let updated = false;

  const stale = pull({
    body: `${marker}\n\nPrevious notes`,
    head: {
      ref: branch.branch,
      sha: 'e'.repeat(40),
      repo: { full_name: repository },
    },
  });

  const result = await upsertPullRequest(
    repository,
    plan,
    branch,
    async (endpoint, method) => {
      if (method === 'PATCH') {
        updated = true;
        return pull();
      }

      if (endpoint.includes('?')) return [stale];

      refreshed = true;
      return pull({ body: `${marker}\n\nPrevious notes` });
    },
  );

  expect(refreshed).toBe(true);
  expect(updated).toBe(true);
  expect(result.action).toBe('updated');
});

test('refuses ambiguous matching PRs', async () => {
  await expect(
    upsertPullRequest(repository, plan, branch, async () => [
      pull(),
      pull({ number: 43 }),
    ]),
  ).rejects.toThrow('multiple matching');
});

test('propagates GitHub errors', async () => {
  await expect(
    upsertPullRequest(repository, plan, branch, async () => {
      throw new Error('API unavailable');
    }),
  ).rejects.toThrow('API unavailable');
});
