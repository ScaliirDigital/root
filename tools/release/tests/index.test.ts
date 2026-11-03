import { expect, test } from 'bun:test';
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ReleasePlan } from '../src/types';

type PlanOutput = Omit<ReleasePlan, 'changes'> & {
  changes: string[];
};

const cli = fileURLToPath(new URL('../src/index.ts', import.meta.url));

function run(cwd: string, command: string[]): string {
  const result = Bun.spawnSync(command, {
    cwd,
    stdout: 'pipe',
    stderr: 'pipe',
  });

  if (result.exitCode !== 0) {
    throw new Error(
      result.stderr.toString().trim() || `Command failed: ${command.join(' ')}`,
    );
  }

  return result.stdout.toString();
}

function git(cwd: string, ...args: string[]): string {
  return run(cwd, [
    'git',
    '-c',
    'user.name=Release Test',
    '-c',
    'user.email=release@example.invalid',
    '-c',
    'commit.gpgSign=false',
    '-c',
    'tag.gpgSign=false',
    '-c',
    'core.hooksPath=/dev/null',
    ...args,
  ]).trim();
}

async function write(root: string, path: string, content: string) {
  const destination = join(root, path);

  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, content);
}

async function readFiles(
  root: string,
  relative = '',
): Promise<Record<string, string>> {
  const files: Record<string, string> = {};
  const entries = await readdir(join(root, relative), {
    withFileTypes: true,
  });

  entries.sort((a, b) => a.name.localeCompare(b.name));

  for (const entry of entries) {
    if (relative === '' && entry.name === '.git') continue;

    const path = join(relative, entry.name);

    if (entry.isDirectory()) {
      Object.assign(files, await readFiles(root, path));
    } else {
      files[path] = await readFile(join(root, path), 'utf8');
    }
  }

  return files;
}

async function snapshot(root: string) {
  return {
    head: git(root, 'rev-parse', 'HEAD'),
    refs: git(root, 'show-ref'),
    status: git(root, 'status', '--porcelain=v1', '--untracked-files=all'),
    staged: git(root, 'diff', '--cached', '--binary'),
    files: await readFiles(root),
  };
}

type CreatedBranch = {
  package: string;
  branch: string;
  sha: string;
  changed: boolean;
};

test('CLI plans without writes and creates repeatable release branches', async () => {
  const root = await mkdtemp(join(tmpdir(), 'release-test-'));

  try {
    const cargo = [
      '[package]',
      'name = "document"',
      'version = "0.1.0"',
      'edition = "2024"',
      '',
    ].join('\n');

    await Promise.all([
      write(
        root,
        'tools/release/packages.toml',
        `
[packages.astro-bun]
language = "typescript"
targets = ["npm"]

[packages.document]
language = "rust"
targets = ["cargo"]
containers = ["ghcr"]
`,
      ),
      write(
        root,
        'packages/astro-bun/package.json',
        `${JSON.stringify(
          {
            name: '@scale.digital/astro-bun',
            version: '0.1.2',
          },
          null,
          2,
        )}\n`,
      ),
      write(root, 'packages/document/Cargo.toml', cargo),
      write(root, 'packages/document/README.md', '# Document\n'),
      write(root, 'README.md', '# Repository\n'),
    ]);

    git(root, 'init', '--quiet', '--initial-branch=main');
    git(root, 'config', 'user.name', 'Release Test');
    git(root, 'config', 'user.email', 'release@example.invalid');
    git(root, 'config', 'commit.gpgSign', 'false');
    git(root, 'config', 'core.hooksPath', '/dev/null');

    git(root, 'add', '.');
    git(root, 'commit', '--quiet', '-m', 'chore(root): initialize fixture');

    const baseSha = git(root, 'rev-parse', 'HEAD');

    git(root, 'tag', 'astro-bun-v0.1.2');
    git(root, 'tag', 'document-v0.1.0');

    const baseline = await snapshot(root);

    const initial: PlanOutput[] = JSON.parse(
      run(root, [process.execPath, cli, 'plan']),
    );

    expect(initial).toHaveLength(2);

    for (const plan of initial) {
      expect(plan.nextVersion).toBeNull();
      expect(plan.commits).toEqual([]);
      expect(plan.changes).toEqual([]);
      expect(plan.branch).toBeNull();
    }

    expect(await snapshot(root)).toEqual(baseline);

    const empty: CreatedBranch[] = JSON.parse(
      run(root, [process.execPath, cli, 'create']),
    );

    expect(empty).toEqual([]);
    expect(await snapshot(root)).toEqual(baseline);

    await write(
      root,
      'packages/document/README.md',
      '# Document\n\nEmpty input is supported.\n',
    );

    git(root, 'add', 'packages/document/README.md');
    git(root, 'commit', '--quiet', '-m', 'fix(document): handle empty input');

    const fixSha = git(root, 'rev-parse', 'HEAD');

    await write(root, 'README.md', '# Repository\n\nUpdated tooling.\n');

    git(root, 'add', 'README.md');
    git(
      root,
      'commit',
      '--quiet',
      '-m',
      'feat(root): extend repository tooling',
    );

    const headSha = git(root, 'rev-parse', 'HEAD');
    const before = await snapshot(root);

    const first = run(root, [process.execPath, cli, 'plan']);

    expect(await snapshot(root)).toEqual(before);

    const second = run(root, [process.execPath, cli, 'plan']);

    expect(second).toBe(first);
    expect(await snapshot(root)).toEqual(before);

    const plans: PlanOutput[] = JSON.parse(first);

    expect(plans).toHaveLength(2);

    const astro = plans.find((plan) => plan.package === 'astro-bun');
    const document = plans.find((plan) => plan.package === 'document');

    if (!astro || !document) {
      throw new Error('Expected both packages in the release plan');
    }

    expect(astro).toMatchObject({
      baseSha,
      headSha,
      currentVersion: '0.1.2',
      nextVersion: null,
      bump: null,
      commits: [],
      changes: [],
      branch: null,
    });

    expect(document).toMatchObject({
      baseSha,
      headSha,
      currentVersion: '0.1.0',
      nextVersion: '0.1.1',
      bump: 'patch',
      targets: ['cargo'],
      containers: ['ghcr'],
      branch: {
        name: 'release/document',
        action: 'create',
        expectedSha: null,
      },
    });

    expect(document.commits).toEqual([
      {
        sha: fixSha,
        type: 'fix',
        description: 'handle empty input',
        breaking: false,
      },
    ]);

    expect(document.changes).toEqual([
      'packages/document/Cargo.toml',
      'packages/document/CHANGELOG.md',
    ]);

    const created: CreatedBranch[] = JSON.parse(
      run(root, [process.execPath, cli, 'create']),
    );

    expect(created).toHaveLength(1);

    const release = created[0];

    if (!release) {
      throw new Error('Expected a document release branch');
    }

    expect(release).toMatchObject({
      package: 'document',
      branch: 'release/document',
      changed: true,
    });

    expect(git(root, 'rev-parse', 'release/document')).toBe(release.sha);
    expect(git(root, 'show', '-s', '--format=%P', release.sha)).toBe(headSha);

    expect(
      git(root, 'diff', '--name-only', headSha, release.sha).split('\n').sort(),
    ).toEqual([
      'packages/document/CHANGELOG.md',
      'packages/document/Cargo.toml',
    ]);

    expect(
      run(root, ['git', 'show', `${release.sha}:packages/document/Cargo.toml`]),
    ).toBe(cargo.replace('version = "0.1.0"', 'version = "0.1.1"'));

    expect(
      run(root, [
        'git',
        'show',
        `${release.sha}:packages/document/CHANGELOG.md`,
      ]),
    ).toBe(
      [
        '# Changelog',
        '',
        '## document 0.1.1',
        '',
        '### Available on',
        '',
        '| Packages |',
        '| :--- |',
        '| [![crates.io](https://img.shields.io/badge/crates.io-0.1.1-F46623?logo=rust&logoColor=white)](https://crates.io/crates/document/0.1.1) |',
        '',
        '### Changes',
        '',
        '#### 🐛 Fixes',
        '',
        `- handle empty input (\`${fixSha.slice(0, 7)}\`)`,
        '',
      ].join('\n'),
    );

    const after = await snapshot(root);
    const expectedRefs = [
      ...before.refs.split('\n'),
      `${release.sha} refs/heads/release/document`,
    ].sort();

    expect(after).toEqual({
      ...before,
      refs: after.refs,
    });

    expect(after.refs.split('\n').sort()).toEqual(expectedRefs);
    expect(git(root, 'symbolic-ref', '--short', 'HEAD')).toBe('main');

    const repeated: CreatedBranch[] = JSON.parse(
      run(root, [process.execPath, cli, 'create']),
    );

    expect(repeated).toEqual([
      {
        ...release,
        changed: false,
      },
    ]);

    expect(await snapshot(root)).toEqual(after);

    const updatedPlans: PlanOutput[] = JSON.parse(
      run(root, [process.execPath, cli, 'plan']),
    );

    const updatedDocument = updatedPlans.find(
      (plan) => plan.package === 'document',
    );

    if (!updatedDocument) {
      throw new Error('Expected document in the updated release plan');
    }

    expect(updatedDocument.nextVersion).toBe('0.1.1');
    expect(updatedDocument.branch).toEqual({
      name: 'release/document',
      action: 'unchanged',
      expectedSha: release.sha,
    });

    expect(await snapshot(root)).toEqual(after);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 15_000);
