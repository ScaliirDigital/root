// tools/release/src/signed-commit.ts

import { githubApi } from './github';
import { fetchRemoteBranch, type RemoteBranch } from './remote';
import type { ReleasePlan } from './types';

const ZERO = '0'.repeat(40);

export type Api = (
  endpoint: string,
  method: 'GET' | 'POST',
  body?: unknown,
) => Promise<unknown>;

export type Entry = { path: string; mode: string; content: string };

export async function createSignedCommit(
  repository: string,
  plan: ReleasePlan,
  entries: Entry[],
  expectedTree: string,
  message: string,
  api: Api,
): Promise<string> {
  const git = `repos/${repository}/git`;

  const base = (await api(`${git}/commits/${plan.headSha}`, 'GET')) as {
    tree: { sha: string };
  };

  const tree = (await api(`${git}/trees`, 'POST', {
    base_tree: base.tree.sha,
    tree: entries.map(({ path, mode, content }) => ({
      path,
      mode,
      type: 'blob',
      content,
    })),
  })) as { sha: string };

  if (tree.sha !== expectedTree) {
    throw new Error(
      `${plan.package}: remote tree ${tree.sha} does not match local tree ${expectedTree}`,
    );
  }

  const commit = (await api(`${git}/commits`, 'POST', {
    message,
    tree: tree.sha,
    parents: [plan.headSha],
  })) as { sha: string; verification?: { verified: boolean; reason: string } };

  if (!commit.verification?.verified) {
    throw new Error(
      `${plan.package}: release commit is not verified (${commit.verification?.reason ?? 'unknown'})`,
    );
  }

  return commit.sha;
}

export async function updateReleaseRef(
  repository: string,
  branch: string,
  before: string | null,
  after: string,
  api: Api,
): Promise<void> {
  const [owner, name] = repository.split('/');

  const lookup = (await api('graphql', 'POST', {
    query:
      'query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id}}',
    variables: { owner, name },
  })) as { data?: { repository?: { id: string } } };

  const repositoryId = lookup.data?.repository?.id;

  if (!repositoryId) {
    throw new Error(`${repository}: repository not found`);
  }

  const result = (await api('graphql', 'POST', {
    query:
      'mutation($input:UpdateRefsInput!){updateRefs(input:$input){clientMutationId}}',
    variables: {
      input: {
        repositoryId,
        refUpdates: [
          {
            name: `refs/heads/${branch}`,
            beforeOid: before ?? ZERO,
            afterOid: after,
            force: true,
          },
        ],
      },
    },
  })) as {
    data?: {
      updateRefs?: {
        clientMutationId: string | null;
      } | null;
    } | null;
    errors?: Array<{ message: string }>;
  };

  if (result.errors?.length) {
    throw new Error(
      `${branch}: ${result.errors.map((error) => error.message).join('; ')}`,
    );
  }

  if (!result.data?.updateRefs) {
    throw new Error(`${branch}: GitHub did not confirm the ref update`);
  }
}

export type CommitInput = {
  tree: string;
  message: string;
  previous: string | null;
  entries: Entry[];
};

export type Committer = (input: CommitInput) => Promise<string>;

export function commitThroughGitHub(
  root: string,
  repository: string,
  plan: ReleasePlan,
  state: RemoteBranch,
  api: Api = githubApi,
): Committer {
  return async ({ tree, message, previous, entries }) => {
    if (previous !== state.expectedSha) {
      throw new Error(`${state.branch}: branch changed since planning`);
    }

    const sha = await createSignedCommit(
      repository,
      plan,
      entries,
      tree,
      message,
      api,
    );

    await updateReleaseRef(repository, state.branch, previous, sha, api);

    fetchRemoteBranch(root, { ...state, expectedSha: sha });

    return sha;
  };
}
