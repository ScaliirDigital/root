import { expect, test } from 'bun:test';
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const cli = fileURLToPath(new URL('../src/index.ts', import.meta.url));

type State = {
  merged: boolean;
  failCreate: boolean;
  attempts: number;
  creates: number;
  release: {
    id: number;
    tag_name: string;
    name: string;
    body: string;
    draft: boolean;
    prerelease: boolean;
  } | null;
};

test('publish creates, resumes and verifies releases without duplicates', async () => {
  const temporary = await mkdtemp(join(tmpdir(), 'release-publish-test-'));

  try {
    const root = join(temporary, 'repository');
    const remote = join(temporary, 'remote.git');
    const bin = join(temporary, 'bin');
    const statePath = join(temporary, 'github.json');
    const repository = 'release-test/root';
    const remoteUrl = `https://github.com/${repository}.git`;

    await mkdir(bin, { recursive: true });

    const env = {
      ...process.env,
      GH_REPO: repository,
      GITHUB_REPOSITORY: repository,
      GH_TOKEN: 'test-token',
      GITHUB_TOKEN: 'test-token',
      PATH: `${bin}:${process.env.PATH ?? ''}`,
      RELEASE_TEST_STATE: statePath,
      GIT_CONFIG_NOSYSTEM: '1',
      GIT_CONFIG_GLOBAL: '/dev/null',
      GIT_CONFIG_COUNT: '5',
      GIT_CONFIG_KEY_0: `url.${remote}.insteadOf`,
      GIT_CONFIG_VALUE_0: remoteUrl,
      GIT_CONFIG_KEY_1: 'user.name',
      GIT_CONFIG_VALUE_1: 'Release Test',
      GIT_CONFIG_KEY_2: 'user.email',
      GIT_CONFIG_VALUE_2: 'release@example.invalid',
      GIT_CONFIG_KEY_3: 'commit.gpgSign',
      GIT_CONFIG_VALUE_3: 'false',
      GIT_CONFIG_KEY_4: 'core.hooksPath',
      GIT_CONFIG_VALUE_4: '/dev/null',
    };

    function execute(cwd: string, command: string[]) {
      const result = Bun.spawnSync(command, {
        cwd,
        env,
        stdout: 'pipe',
        stderr: 'pipe',
      });

      return {
        code: result.exitCode,
        stdout: result.stdout.toString(),
        stderr: result.stderr.toString(),
      };
    }

    function run(cwd: string, command: string[]): string {
      const result = execute(cwd, command);

      if (result.code !== 0) {
        throw new Error(
          result.stderr.trim() || `Command failed: ${command.join(' ')}`,
        );
      }

      return result.stdout;
    }

    function git(...args: string[]): string {
      return run(root, ['git', ...args]).trim();
    }

    async function write(path: string, content: string) {
      const destination = join(root, path);

      await mkdir(dirname(destination), { recursive: true });
      await writeFile(destination, content);
    }

    async function readState(): Promise<State> {
      return Bun.file(statePath).json();
    }

    async function saveState(state: State) {
      await writeFile(statePath, JSON.stringify(state));
    }

    await saveState({
      merged: false,
      failCreate: false,
      attempts: 0,
      creates: 0,
      release: null,
    });

    const fakeGh = join(bin, 'gh');

    await writeFile(
      fakeGh,
      `#!${process.execPath}
const args = Bun.argv.slice(2);
const statePath = process.env.RELEASE_TEST_STATE;
const state = await Bun.file(statePath).json();

function fail(message) {
  console.error(message);
  process.exit(1);
}

if (args[0] !== 'api') fail('Unexpected gh command');

const methodIndex = args.indexOf('--method');
const method = args[methodIndex + 1];
const endpoint = args[methodIndex + 2];

if (!endpoint?.startsWith('repos/release-test/root/')) {
  fail('Unexpected repository: ' + endpoint);
}

let response;

if (
  method === 'GET' &&
  endpoint.startsWith('repos/release-test/root/commits/') &&
  endpoint.includes('/pulls?')
) {
  const sha = endpoint.split('/')[4];

  response = state.merged ? [{
    number: 42,
    body: '<!-- scale-release:v1 package=document -->',
    merged_at: '2026-10-29T12:00:00Z',
    merge_commit_sha: sha,
    head: {
      ref: 'release/document',
      repo: { full_name: 'release-test/root' },
    },
    base: {
      ref: 'main',
      repo: { full_name: 'release-test/root' },
    },
  }] : [];
} else if (
  method === 'GET' &&
  endpoint.startsWith('repos/release-test/root/releases?')
) {
  response = state.release ? [state.release] : [];
} else if (
  method === 'POST' &&
  endpoint === 'repos/release-test/root/releases'
) {
  state.attempts += 1;

  if (state.failCreate) {
    await Bun.write(statePath, JSON.stringify(state));
    fail('Simulated release creation failure');
  }

  if (state.release) fail('Duplicate release creation');

  const body = JSON.parse(await Bun.stdin.text());

  state.release = {
    id: 1,
    tag_name: body.tag_name,
    name: body.name,
    body: body.body,
    draft: body.draft,
    prerelease: body.prerelease,
  };

  state.creates += 1;
  response = state.release;
} else {
  fail('Unexpected GitHub request: ' + method + ' ' + endpoint);
}

await Bun.write(statePath, JSON.stringify(state));
console.log(JSON.stringify(response));
`,
    );

    await chmod(fakeGh, 0o755);

    run(temporary, [
      'git',
      'init',
      '--quiet',
      '--bare',
      '--initial-branch=main',
      remote,
    ]);

    run(temporary, ['git', 'init', '--quiet', '--initial-branch=main', root]);

    await write(
      'tools/release/packages.toml',
      [
        '[packages.document]',
        'language = "rust"',
        'targets = ["cargo", "binary"]',
        'containers = ["ghcr", "dockerhub"]',
        '',
      ].join('\n'),
    );

    await write(
      'packages/document/Cargo.toml',
      '[package]\nname = "document"\nversion = "0.1.0"\n',
    );

    git('add', '.');
    git('commit', '--quiet', '-m', 'chore(root): initialize fixture');

    const baseSha = git('rev-parse', 'HEAD');

    git('tag', 'document-v0.1.0');
    git('switch', '--quiet', '-c', 'release/document');

    await write(
      'packages/document/Cargo.toml',
      '[package]\nname = "document"\nversion = "0.1.1"\n',
    );

    const notes = '### 🐛 Fixes\n\n- Handle empty input.\n';

    await write(
      'packages/document/CHANGELOG.md',
      `# Changelog\n\n## document 0.1.1\n\n${notes}`,
    );

    git('add', '.');
    git('commit', '--quiet', '-m', 'chore(document): release 0.1.1');
    git('switch', '--quiet', 'main');
    git('merge', '--squash', 'release/document');
    git('commit', '--quiet', '-m', 'chore(document): release 0.1.1 (#42)');
    git('push', remoteUrl, 'main', '--tags');

    const releaseSha = git('rev-parse', 'HEAD');
    const command = [process.execPath, cli, 'publish'];
    const prepareCommand = [...command, '--prepare'];
    const finalizeCommand = [...command, '--package=document'];
    const tagRef = 'refs/tags/document-v0.1.1';

    // Kein erkannter Release-PR: keine Veröffentlichung.
    expect(JSON.parse(run(root, command))).toEqual([]);
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe('');
    expect((await readState()).attempts).toBe(0);

    await saveState({
      ...(await readState()),
      merged: true,
    });

    // Ein falscher vorhandener Tag muss unverändert bleiben.
    git('push', remoteUrl, `${baseSha}:${tagRef}`);

    const conflictingTag = execute(root, command);

    expect(conflictingTag.code).not.toBe(0);
    expect(conflictingTag.stderr).toContain('different commit');
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe(
      `${baseSha}\t${tagRef}`,
    );
    expect((await readState()).attempts).toBe(0);

    git('push', remoteUrl, `:${tagRef}`);

    // Ohne Vorbereitung darf der Abschluss keinen Tag oder Release erstellen.
    const unprepared = execute(root, finalizeCommand);

    expect(unprepared.code).not.toBe(0);
    expect(unprepared.stderr).toContain('release tag is missing');
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe('');
    expect((await readState()).attempts).toBe(0);

    // Vorbereitung erstellt ausschließlich den Tag.
    const prepared = JSON.parse(run(root, prepareCommand));

    expect(prepared).toHaveLength(1);
    expect(prepared[0]).toMatchObject({
      package: 'document',
      version: '0.1.1',
      tag: 'document-v0.1.1',
      sha: releaseSha,
    });

    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe(
      `${releaseSha}\t${tagRef}`,
    );
    expect(git('rev-parse', 'document-v0.1.1^{commit}')).toBe(releaseSha);

    const afterPrepare = await readState();

    expect(afterPrepare.release).toBeNull();
    expect(afterPrepare.attempts).toBe(0);
    expect(afterPrepare.creates).toBe(0);

    // Wiederholte Vorbereitung erstellt ebenfalls keinen GitHub Release.
    expect(JSON.parse(run(root, prepareCommand))).toEqual(prepared);
    expect(await readState()).toEqual(afterPrepare);

    // Ein anderes Paket darf nicht versehentlich abgeschlossen werden.
    const wrongPackage = execute(root, [...command, '--package=astro-bun']);

    expect(wrongPackage.code).not.toBe(0);
    expect(wrongPackage.stderr).toContain(
      'expected one matching merged release',
    );
    expect(await readState()).toEqual(afterPrepare);

    // Ein Fehler beim Abschluss lässt den vorbereiteten Release wiederaufnehmbar.
    await saveState({
      ...(await readState()),
      failCreate: true,
    });

    const interrupted = execute(root, finalizeCommand);

    expect(interrupted.code).not.toBe(0);
    expect(interrupted.stderr).toContain('Simulated release creation failure');
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe(
      `${releaseSha}\t${tagRef}`,
    );
    expect((await readState()).release).toBeNull();

    await saveState({
      ...(await readState()),
      failCreate: false,
    });

    const published = JSON.parse(run(root, finalizeCommand));
    expect(published).toEqual([
      {
        package: 'document',
        path: 'packages/document',
        version: '0.1.1',
        tag: 'document-v0.1.1',
        sha: releaseSha,
        pullRequest: 42,
        notes,
        targets: ['cargo', 'binary'],
        containers: ['ghcr', 'dockerhub'],
      },
    ]);

    expect((await readState()).creates).toBe(1);

    expect(git('rev-parse', 'document-v0.1.1^{commit}')).toBe(releaseSha);

    const nextPlans = JSON.parse(run(root, [process.execPath, cli, 'plan']));

    expect(nextPlans).toHaveLength(1);
    expect(nextPlans[0]).toMatchObject({
      package: 'document',
      currentVersion: '0.1.1',
      nextVersion: null,
      bump: null,
      baseTag: 'document-v0.1.1',
      changes: [],
      branch: null,
    });

    expect(JSON.parse(run(root, [process.execPath, cli, 'create']))).toEqual(
      [],
    );

    // Wiederholung liefert denselben Auftrag ohne weitere Erstellung.
    const beforeRepeat = await readState();

    expect(JSON.parse(run(root, finalizeCommand))).toEqual(published);
    expect(await readState()).toEqual(beforeRepeat);

    // Wiederholung auch aus einem frischen Checkout.
    const checkout = join(temporary, 'checkout');

    run(temporary, [
      'git',
      'clone',
      '--quiet',
      '--branch',
      'main',
      remoteUrl,
      checkout,
    ]);

    expect(JSON.parse(run(checkout, finalizeCommand))).toEqual(published);
    expect(await readState()).toEqual(beforeRepeat);

    // Abweichende Notes dürfen nicht still überschrieben werden.
    const modified = await readState();

    if (!modified.release) {
      throw new Error('Expected a GitHub release');
    }

    modified.release.body = 'Manually changed release notes';
    await saveState(modified);

    const conflictingRelease = execute(root, finalizeCommand);

    expect(conflictingRelease.code).not.toBe(0);
    expect(conflictingRelease.stderr).toContain('does not match');
    expect((await readState()).release?.body).toBe(
      'Manually changed release notes',
    );

    expect(git('rev-parse', 'HEAD')).toBe(releaseSha);
    expect(git('status', '--porcelain')).toBe('');
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}, 30_000);
