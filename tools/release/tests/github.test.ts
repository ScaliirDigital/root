import { describe, expect, test } from 'bun:test';
import { renderChangelog } from '../src/changelog';
import { enrichPlans } from '../src/github';
import type { ReleasePlan } from '../src/types';

const plan: ReleasePlan = {
  package: 'document',
  language: 'rust',
  targets: ['cargo'],
  containers: [],
  baseTag: 'document-v0.1.0',
  baseSha: 'a'.repeat(40),
  headSha: 'b'.repeat(40),
  currentVersion: '0.1.0',
  nextVersion: '0.1.1',
  bump: 'patch',
  commits: [
    {
      sha: 'b'.repeat(40),
      type: 'fix',
      description: 'handle empty input (#61)',
      breaking: false,
    },
  ],
};

function api(previous: unknown[], authorType = 'User') {
  return async (endpoint: string): Promise<unknown> => {
    if (endpoint.includes('/pulls?')) {
      return [
        {
          number: 61,
          merged_at: '2026-10-29T10:00:00Z',
          merge_commit_sha: plan.headSha,
          base: {
            ref: 'main',
            repo: { full_name: 'example/root' },
          },
        },
      ];
    }

    if (endpoint.includes('/commits?')) {
      return previous;
    }

    if (endpoint.endsWith(`/commits/${plan.headSha}`)) {
      return {
        author: { login: 'alice', type: authorType },
      };
    }

    throw new Error(`Unexpected endpoint: ${endpoint}`);
  };
}

describe('GitHub metadata', () => {
  test('renders authors, commit links and new contributors', async () => {
    const [result] = await enrichPlans([plan], 'example/root', api([]));

    expect(result.newContributors).toEqual([{ login: 'alice', isBot: false }]);

    const markdown = renderChangelog(result);

    expect(markdown).toContain(
      'handle empty input by [@alice](https://github.com/alice)',
    );
    expect(markdown).toContain(
      `https://github.com/example/root/commit/${plan.commits[0]?.sha}`,
    );

    expect(markdown).not.toContain('/pull/61');
    expect(markdown).not.toContain('(#61)');
    expect(markdown).not.toContain('input (#61)');
    expect(markdown).toContain('### 👋 New Contributors');
  });

  test('does not label existing authors as new contributors', async () => {
    const [result] = await enrichPlans(
      [plan],
      'example/root',
      api([{ sha: plan.baseSha }]),
    );

    expect(result.newContributors).toEqual([]);
    expect(renderChangelog(result)).not.toContain('New Contributors');
  });

  test('excludes bots from new contributors', async () => {
    const [result] = await enrichPlans([plan], 'example/root', api([], 'Bot'));

    expect(result.newContributors).toEqual([]);
  });

  test('skips network requests when no commits exist', async () => {
    const unchanged = {
      ...plan,
      nextVersion: null,
      bump: null,
      commits: [],
    };

    const results = await enrichPlans([unchanged], 'example/root', async () => {
      throw new Error('No request should occur');
    });

    expect(results).toEqual([unchanged]);
  });

  test('propagates API errors instead of guessing contributor status', async () => {
    await expect(
      enrichPlans([plan], 'example/root', async () => {
        throw new Error('GitHub unavailable');
      }),
    ).rejects.toThrow('GitHub unavailable');
  });
});
