// tools/release/tests/changelog.test.ts
import { describe, expect, test } from 'bun:test';
import { prependRelease, renderChangelog } from '../src/changelog';
import type { ReleasePlan } from '../src/types';

function plan(overrides: Partial<ReleasePlan> = {}): ReleasePlan {
  return {
    package: 'astro-bun',
    language: 'typescript',
    targets: ['npm'],
    containers: [],
    baseTag: 'astro-bun-v0.1.2',
    baseSha: 'a'.repeat(40),
    headSha: 'b'.repeat(40),
    currentVersion: '0.1.2',
    nextVersion: '0.1.3',
    bump: 'patch',
    commits: [
      {
        sha: 'abcdef1234567890',
        type: 'fix',
        description: 'handle empty responses',
        breaking: false,
      },
    ],
    ...overrides,
  };
}

const npmPublication = {
  category: 'packages',
  label: 'npm',
  url: 'https://www.npmjs.com/package/@scale.digital/astro-bun/v/0.1.3',
  logo: 'npm',
  color: 'CB3837',
} as const;

const npmBadge =
  '[![npm](https://img.shields.io/badge/npm-0.1.3-CB3837?logo=npm&logoColor=white)](https://www.npmjs.com/package/@scale.digital/astro-bun/v/0.1.3)';

describe('changelog rendering', () => {
  test('renders a release section', () => {
    expect(renderChangelog(plan())).toBe(
      [
        '## astro-bun 0.1.3',
        '',
        '### Changes',
        '',
        '#### 🐛 Fixes',
        '',
        '- handle empty responses (`abcdef1`)',
        '',
      ].join('\n'),
    );
  });

  test('produces no output without a release', () => {
    expect(renderChangelog(plan({ nextVersion: null, bump: null }))).toBe('');
  });

  test('omits empty sections', () => {
    const markdown = renderChangelog(plan());

    expect(markdown).not.toContain('Features');
    expect(markdown).not.toContain('Breaking Changes');
    expect(markdown).not.toContain('Maintenance');
  });

  test('puts breaking changes first without duplicating them', () => {
    const markdown = renderChangelog(
      plan({
        nextVersion: '0.2.0',
        bump: 'minor',
        commits: [
          ...plan().commits,
          {
            sha: '123456789abcdef0',
            type: 'feat',
            description: 'require the new request format',
            breaking: true,
          },
        ],
      }),
    );

    expect(markdown.indexOf('Breaking Changes')).toBeLessThan(
      markdown.indexOf('Fixes'),
    );
    expect(markdown).not.toContain('### ✨ Features');
    expect(markdown.match(/require the new request format/g)).toHaveLength(1);
  });

  test('keeps unknown types visible', () => {
    const markdown = renderChangelog(
      plan({
        commits: [
          {
            sha: 'abcdef1234567890',
            type: 'custom',
            description: 'update integration',
            breaking: false,
          },
        ],
      }),
    );

    expect(markdown).toContain('### Other Changes');
    expect(markdown).toContain('update integration');
  });

  test('escapes Markdown in commit descriptions', () => {
    const markdown = renderChangelog(
      plan({
        commits: [
          {
            sha: 'abcdef1234567890',
            type: 'fix',
            description: 'handle <input> and *values*',
            breaking: false,
          },
        ],
      }),
    );

    expect(markdown).toContain('handle \\<input\\> and \\*values\\*');
  });

  test('rejects a release without commits', () => {
    expect(() => renderChangelog(plan({ commits: [] }))).toThrow(
      'without commits',
    );
  });

  test('renders the availability table before the changes', () => {
    const markdown = renderChangelog(plan({ publications: [npmPublication] }));

    expect(markdown).toContain('### Available on');
    expect(markdown).toContain('| Packages |');
    expect(markdown).toContain(npmBadge);
    expect(markdown.indexOf('### Available on')).toBeLessThan(
      markdown.indexOf('### Changes'),
    );
  });

  test('omits the availability table when requested', () => {
    const markdown = renderChangelog(plan({ publications: [npmPublication] }), {
      availability: false,
    });

    expect(markdown).not.toContain('### Available on');
    expect(markdown).not.toContain(npmBadge);
    expect(markdown).toContain('### Changes');
  });
});

describe('changelog history', () => {
  const section = '## 0.1.3\n\n### 🐛 Fixes\n\n- Handle empty input\n';

  test('creates a new changelog', () => {
    expect(prependRelease(null, section)).toBe(`# Changelog\n\n${section}`);
  });

  test('preserves the existing title, introduction and release history', () => {
    const introduction =
      '# @scale.digital/astro-bun\n\nRelease history for the adapter.\n\n';

    const history = [
      '## 0.1.2',
      '',
      '### Patch Changes',
      '',
      '- Previously published description.',
      '',
      '## 0.1.1',
      '',
      '- Older published description.',
      '',
    ].join('\n');

    expect(prependRelease(`${introduction}${history}`, section)).toBe(
      `${introduction}${section}\n${history}`,
    );
  });

  test('supports a changelog containing only a title', () => {
    expect(prependRelease('# Changelog\n', section)).toBe(
      `# Changelog\n\n${section}`,
    );
  });

  test('leaves the file unchanged when no section is generated', () => {
    const existing = '# Changelog\n\n## 0.1.2\n\n- Existing entry.\n';

    expect(prependRelease(existing, '')).toBe(existing);
    expect(prependRelease(null, '')).toBe('');
  });

  test('produces identical output from the same baseline', () => {
    const existing = '# Changelog\n\n## 0.1.2\n\n- Existing entry.\n';

    expect(prependRelease(existing, section)).toBe(
      prependRelease(existing, section),
    );

    expect(
      prependRelease(existing, section).match(/^## 0\.1\.3$/gm),
    ).toHaveLength(1);
  });
});
