// tools/release/src/changelog.ts
import type { Commit, ReleasePlan } from './types';

export type RenderOptions = { availability?: boolean };

const sections = [
  ['breaking', '💥 Breaking Changes'],
  ['feat', '✨ Features'],
  ['fix', '🐛 Fixes'],
  ['perf', '⚡ Performance'],
  ['revert', '⏪ Reverts'],
  ['docs', '📚 Documentation'],
  ['refactor', '♻️ Refactoring'],
  ['test', '🧪 Tests'],
  ['build', '📦 Build'],
  ['ci', '🔧 CI'],
  ['chore', '🧹 Maintenance'],
  ['other', 'Other Changes'],
] as const;

function escapeMarkdown(value: string): string {
  return value.replace(/[\\`*_[\]<>]/g, '\\$&');
}

function renderCommit(commit: Commit): string {
  const description = commit.description.replace(/\s+\(#\d+\)$/, '');

  let entry = `- ${escapeMarkdown(description)}`;

  if (commit.author) {
    const login = commit.author.login;
    entry += ` by [@${login}](https://github.com/${login})`;
  }

  if (commit.url) {
    entry += ` ([${commit.sha.slice(0, 7)}](${commit.url}))`;
  } else {
    entry += ` (\`${commit.sha.slice(0, 7)}\`)`;
  }

  return entry;
}

export function renderChangelog(
  plan: ReleasePlan,
  options: RenderOptions = {},
): string {
  if (plan.nextVersion === null) {
    return '';
  }

  if (plan.commits.length === 0) {
    throw new Error(`${plan.package}: cannot render a release without commits`);
  }

  const groups = new Map<string, string[]>();
  const knownTypes = new Set<string>(sections.map(([type]) => type));
  const contributors = new Map<string, { login: string; commits: number }>();

  for (const commit of plan.commits) {
    const group = commit.breaking
      ? 'breaking'
      : knownTypes.has(commit.type)
        ? commit.type
        : 'other';

    const entry = renderCommit(commit);
    const entries = groups.get(group);

    if (entries) {
      entries.push(entry);
    } else {
      groups.set(group, [entry]);
    }

    if (commit.author && !commit.author.isBot) {
      const { login } = commit.author;
      const key = login.toLowerCase();
      const contributor = contributors.get(key);

      if (contributor) {
        contributor.commits += 1;
      } else {
        contributors.set(key, { login, commits: 1 });
      }
    }
  }

  const ranked = [...contributors.values()].sort(
    (left, right) =>
      right.commits - left.commits ||
      left.login.toLowerCase().localeCompare(right.login.toLowerCase(), 'en'),
  );

  const output = [`## ${plan.package} ${plan.nextVersion}`];

  const publicationGroups = [
    ['binary', 'Binary'],
    ['container', 'Container'],
    ['packages', 'Packages'],
  ];

  const columns: Array<{ title: string; badges: string[] }> = [];

  for (const [category, title] of publicationGroups) {
    const publications = (plan.publications ?? [])
      .filter((publication) => publication.category === category)
      .sort((left, right) => left.label.localeCompare(right.label, 'en'));

    if (publications.length === 0) continue;

    const badges = publications.map((publication) => {
      const badge = new URL(
        `https://img.shields.io/badge/${encodeURIComponent(
          `${publication.label}-${plan.nextVersion}-${publication.color}`,
        )}`,
      );

      badge.searchParams.set('logo', publication.logo);
      badge.searchParams.set('logoColor', 'white');

      return `[![${publication.label}](${badge})](${publication.url})`;
    });

    columns.push({ title, badges });
  }

  if (columns.length > 0 && options.availability !== false) {
    const rowCount = Math.max(...columns.map(({ badges }) => badges.length));

    const rows = Array.from(
      { length: rowCount },
      (_, index) =>
        `| ${columns.map(({ badges }) => badges[index] ?? '').join(' | ')} |`,
    );

    const table = [
      `| ${columns.map(({ title }) => title).join(' | ')} |`,
      `| ${columns.map(() => ':---').join(' | ')} |`,
      ...rows,
    ].join('\n');

    output.push(`### Available on\n\n${table}`);
  }

  if (ranked.length > 0) {
    const entries = ranked.map(({ login, commits }) => {
      const label = commits === 1 ? 'commit' : 'commits';

      return `[@${login}](https://github.com/${login}) · **${commits} ${label}**`;
    });

    output.push(`### Contributors\n\n${entries.join(' | ')}`);
  }

  output.push('### Changes');

  for (const [type, title] of sections) {
    const entries = groups.get(type);

    if (entries) {
      output.push(`#### ${title}\n\n${entries.join('\n')}`);
    }
  }

  const newcomerLogins = new Set(
    (plan.newContributors ?? [])
      .filter((author) => !author.isBot)
      .map((author) => author.login.toLowerCase()),
  );

  const newcomers = ranked.filter(({ login }) =>
    newcomerLogins.has(login.toLowerCase()),
  );

  if (newcomers.length > 0) {
    const entries = newcomers.map(
      ({ login }) => `[@${login}](https://github.com/${login})`,
    );

    output.push(
      [
        '### 👋 New Contributors',
        'Welcome to our new contributors — thank you for helping make this release happen!',
        entries.join(' | '),
      ].join('\n\n'),
    );
  }

  return `${output.join('\n\n')}\n`;
}

export function prependRelease(
  existing: string | null,
  section: string,
): string {
  const release = section.trim();

  if (!release) {
    return existing ?? '';
  }

  if (existing === null || existing.length === 0) {
    return `# Changelog\n\n${release}\n`;
  }

  const firstRelease = /^##[ \t]+/m.exec(existing);
  const position = firstRelease?.index ?? existing.length;

  const introduction = existing.slice(0, position);
  const history = existing.slice(position);

  const separator =
    introduction.length === 0 || introduction.endsWith('\n\n')
      ? ''
      : introduction.endsWith('\n')
        ? '\n'
        : '\n\n';

  const suffix = history ? `\n${history}` : '';

  return `${introduction}${separator}${release}\n${suffix}`;
}
