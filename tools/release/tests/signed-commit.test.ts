// tools/release/tests/signed-commit.test.ts
import { describe, expect, test } from 'bun:test';
import {
  commitThroughGitHub,
  createSignedCommit,
  updateReleaseRef,
} from '../src/signed-commit';
import type { ReleasePlan } from '../src/types';

type RefUpdate = {
  name: string;
  beforeOid: string;
  afterOid: string;
  force: boolean;
};

type RefUpdatesInput = { repositoryId: string; refUpdates: RefUpdate[] };

type GraphqlBody = {
  query: string;
  variables: { input: RefUpdatesInput };
};

const repository = 'release-test/root';
const head = 'a'.repeat(40);
const baseTree = 'b'.repeat(40);
const tree = 'c'.repeat(40);
const commit = 'd'.repeat(40);
const previous = 'e'.repeat(40);
const zero = '0'.repeat(40);

const plan = { package: 'document', headSha: head } as ReleasePlan;

const entries = [
  {
    path: 'packages/document/Cargo.toml',
    mode: '100644',
    content: 'version = "0.1.1"\n',
  },
];

type Call = { endpoint: string; method: string; body?: unknown };
type Handler = (body: unknown) => unknown;

function fakeApi(handlers: Record<string, Handler>) {
  const calls: Call[] = [];

  const api = async (
    endpoint: string,
    method: 'GET' | 'POST',
    body?: unknown,
  ) => {
    calls.push({ endpoint, method, body });

    const handler = handlers[`${method} ${endpoint}`];

    if (!handler) {
      throw new Error(`Unexpected request: ${method} ${endpoint}`);
    }

    return handler(body);
  };

  return { api, calls };
}

function gitData(overrides: Record<string, Handler> = {}) {
  return {
    [`GET repos/${repository}/git/commits/${head}`]: () => ({
      tree: { sha: baseTree },
    }),
    [`POST repos/${repository}/git/trees`]: () => ({ sha: tree }),
    [`POST repos/${repository}/git/commits`]: () => ({
      sha: commit,
      verification: { verified: true, reason: 'valid' },
    }),
    ...overrides,
  };
}

function graphql(
  refUpdate: (input: RefUpdatesInput) => unknown = () => ({
    data: { updateRefs: { clientMutationId: null } },
  }),
) {
  return {
    'POST graphql': (body: unknown) => {
      const request = body as GraphqlBody;

      return request.query.includes('repository(')
        ? { data: { repository: { id: 'R_test' } } }
        : refUpdate(request.variables.input);
    },
  };
}

describe('signed commits', () => {
  test('creates a verified commit without custom identity', async () => {
    const { api, calls } = fakeApi(gitData());

    const sha = await createSignedCommit(
      repository,
      plan,
      entries,
      tree,
      'message',
      api,
    );

    expect(sha).toBe(commit);

    const created = calls.find((call) =>
      call.endpoint.endsWith('/git/commits'),
    );

    expect(created?.body).toEqual({
      message: 'message',
      tree,
      parents: [head],
    });
  });

  test('rejects a remote tree that differs from the local tree', async () => {
    const { api, calls } = fakeApi(
      gitData({
        [`POST repos/${repository}/git/trees`]: () => ({ sha: 'f'.repeat(40) }),
      }),
    );

    await expect(
      createSignedCommit(repository, plan, entries, tree, 'message', api),
    ).rejects.toThrow('does not match local tree');

    expect(calls.some((call) => call.endpoint.endsWith('/git/commits'))).toBe(
      false,
    );
  });

  test('rejects an unverified commit', async () => {
    const { api } = fakeApi(
      gitData({
        [`POST repos/${repository}/git/commits`]: () => ({
          sha: commit,
          verification: { verified: false, reason: 'unsigned' },
        }),
      }),
    );

    await expect(
      createSignedCommit(repository, plan, entries, tree, 'message', api),
    ).rejects.toThrow('not verified (unsigned)');
  });

  test('creates a new branch only if it does not exist', async () => {
    let input: RefUpdatesInput | undefined;

    const { api } = fakeApi(
      graphql((value) => {
        input = value;
        return { data: { updateRefs: { clientMutationId: null } } };
      }),
    );

    await updateReleaseRef(repository, 'release/document', null, commit, api);

    expect(input?.refUpdates).toEqual([
      {
        name: 'refs/heads/release/document',
        beforeOid: zero,
        afterOid: commit,
        force: true,
      },
    ]);
  });

  test('updates an existing branch only from the expected commit', async () => {
    let input: RefUpdatesInput | undefined;

    const { api } = fakeApi(
      graphql((value) => {
        input = value;
        return { data: { updateRefs: { clientMutationId: null } } };
      }),
    );

    await updateReleaseRef(
      repository,
      'release/document',
      previous,
      commit,
      api,
    );

    expect(input?.refUpdates[0]?.beforeOid).toBe(previous);
  });

  test('propagates a rejected ref update', async () => {
    const { api } = fakeApi(
      graphql(() => ({ errors: [{ message: 'ref changed concurrently' }] })),
    );

    await expect(
      updateReleaseRef(repository, 'release/document', previous, commit, api),
    ).rejects.toThrow('ref changed concurrently');
  });

  test('refuses a branch changed since planning before calling GitHub', async () => {
    const { api, calls } = fakeApi({});

    const committer = commitThroughGitHub(
      '/nonexistent',
      repository,
      plan,
      {
        remote: 'https://github.com/release-test/root.git',
        branch: 'release/document',
        expectedSha: previous,
      },
      api,
    );

    await expect(
      committer({ tree, message: 'message', previous: null, entries }),
    ).rejects.toThrow('branch changed since planning');

    expect(calls).toHaveLength(0);
  });

  test('rejects an unconfirmed ref update', async () => {
    const { api } = fakeApi(graphql(() => ({ data: {} })));

    await expect(
      updateReleaseRef(repository, 'release/document', previous, commit, api),
    ).rejects.toThrow('GitHub did not confirm the ref update');
  });
});
