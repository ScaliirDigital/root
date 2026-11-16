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
  failFinalize: boolean;
  finalizes: number;
  hiddenListReads?: number;
  missingIdReads?: number;
  staleDraftReads?: number;
  forbiddenIdReads?: number;
  loseCreateResponse?: boolean;
  loseFinalizeResponse?: boolean;
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
      failFinalize: false,
      finalizes: 0,
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
  if (state.release && (state.hiddenListReads ?? 0) > 0) {
    state.hiddenListReads -= 1;
    response = [];
  } else {
    response = state.release ? [state.release] : [];
  }
} else if (
  method === 'GET' &&
  endpoint === 'repos/release-test/root/releases/1'
) {
  if ((state.forbiddenIdReads ?? 0) > 0) {
    state.forbiddenIdReads -= 1;
    await Bun.write(statePath, JSON.stringify(state));
    fail('gh: Forbidden (HTTP 403)');
  }

  if (!state.release || (state.missingIdReads ?? 0) > 0) {
    if ((state.missingIdReads ?? 0) > 0) {
      state.missingIdReads -= 1;
    }

    await Bun.write(statePath, JSON.stringify(state));
    fail('gh: Not Found (HTTP 404)');
  }

  if ((state.staleDraftReads ?? 0) > 0) {
    state.staleDraftReads -= 1;
    response = { ...state.release, draft: true };
  } else {
    response = state.release;
  }
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

  if (state.loseCreateResponse) {
    await Bun.write(statePath, JSON.stringify(state));
    fail('Simulated lost creation response');
  }

  response = state.release;
} else if (
  method === 'PATCH' &&
  endpoint === 'repos/release-test/root/releases/1'
) {
  if (!state.release) fail('Release is missing');

  const body = JSON.parse(await Bun.stdin.text());

  if (body.draft !== false || body.make_latest !== 'false') {
    fail('Unexpected release update');
  }

  if (state.failFinalize) {
    fail('Simulated release publication failure');
  }

  state.release.draft = false;
  state.finalizes += 1;

  if (state.loseFinalizeResponse) {
    await Bun.write(statePath, JSON.stringify(state));
    fail('Simulated lost publication response');
  }

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
    const draftCommand = [...command, '--draft', '--package=document'];
    const tagRef = 'refs/tags/document-v0.1.1';

    // Do not publish without a recognized merged release PR.
    expect(JSON.parse(run(root, command))).toEqual([]);
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe('');
    expect((await readState()).attempts).toBe(0);

    await saveState({
      ...(await readState()),
      merged: true,
    });

    // Preserve an existing tag that points to a different commit.
    git('push', remoteUrl, `${baseSha}:${tagRef}`);

    const conflictingTag = execute(root, command);

    expect(conflictingTag.code).not.toBe(0);
    expect(conflictingTag.stderr).toContain('different commit');
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe(
      `${baseSha}\t${tagRef}`,
    );
    expect((await readState()).attempts).toBe(0);

    git('push', remoteUrl, `:${tagRef}`);

    // Finalization must not create a missing release tag.
    const unprepared = execute(root, finalizeCommand);

    expect(unprepared.code).not.toBe(0);
    expect(unprepared.stderr).toContain('release tag is missing');
    expect(git('ls-remote', '--refs', remoteUrl, tagRef)).toBe('');
    expect((await readState()).attempts).toBe(0);

    // Preparation creates only the tag.
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

    // Repeated preparation must not create a GitHub release.
    expect(JSON.parse(run(root, prepareCommand))).toEqual(prepared);
    expect(await readState()).toEqual(afterPrepare);

    // Reject a package that does not match the detected release.
    const wrongPackage = execute(root, [...command, '--package=astro-bun']);

    expect(wrongPackage.code).not.toBe(0);
    expect(wrongPackage.stderr).toContain(
      'expected one matching merged release',
    );
    expect(await readState()).toEqual(afterPrepare);

    // Invalid flags must not modify release state.
    for (const flags of [
      ['--prepare', '--draft'],
      ['--draft', '--draft'],
    ]) {
      const invalid = execute(root, [...command, ...flags]);

      expect(invalid.code).not.toBe(0);
      expect(invalid.stderr).toContain('Usage:');
      expect(await readState()).toEqual(afterPrepare);
    }

    // A failed draft creation remains resumable.
    await saveState({
      ...(await readState()),
      failCreate: true,
    });

    const failedDraft = execute(root, draftCommand);

    expect(failedDraft.code).not.toBe(0);
    expect(failedDraft.stderr).toContain('Simulated release creation failure');
    expect((await readState()).release).toBeNull();

    await saveState({
      ...(await readState()),
      failCreate: false,
    });

    const drafted = JSON.parse(run(root, draftCommand));
    const afterDraft = await readState();

    expect(drafted).toEqual(prepared);
    expect(afterDraft.release).toMatchObject({
      tag_name: 'document-v0.1.1',
      name: 'document 0.1.1',
      body: notes,
      draft: true,
      prerelease: false,
    });
    expect(afterDraft.creates).toBe(1);
    expect(afterDraft.finalizes).toBe(0);

    // Repetition and preparation must not publish an existing draft.
    expect(JSON.parse(run(root, draftCommand))).toEqual(drafted);
    expect(JSON.parse(run(root, prepareCommand))).toEqual(prepared);
    expect(await readState()).toEqual(afterDraft);

    // Reject modified draft notes before publishing.
    if (!afterDraft.release) {
      throw new Error('Expected a draft release');
    }

    await saveState({
      ...afterDraft,
      release: {
        ...afterDraft.release,
        body: 'Manually changed draft notes',
      },
    });

    const conflictingDraft = execute(root, finalizeCommand);

    expect(conflictingDraft.code).not.toBe(0);
    expect(conflictingDraft.stderr).toContain('does not match');
    expect((await readState()).release?.draft).toBe(true);
    expect((await readState()).finalizes).toBe(0);

    // A publication failure preserves the draft.
    await saveState({
      ...afterDraft,
      failFinalize: true,
    });

    const failedFinalize = execute(root, finalizeCommand);

    expect(failedFinalize.code).not.toBe(0);
    expect(failedFinalize.stderr).toContain(
      'Simulated release publication failure',
    );
    expect((await readState()).release?.draft).toBe(true);
    expect((await readState()).finalizes).toBe(0);

    await saveState({
      ...(await readState()),
      failFinalize: false,
    });

    expect(JSON.parse(run(root, finalizeCommand))).toEqual(drafted);

    const afterFinalize = await readState();

    expect(afterFinalize.release?.draft).toBe(false);
    expect(afterFinalize.creates).toBe(1);
    expect(afterFinalize.finalizes).toBe(1);

    // Repeated commands must not modify published releases.
    expect(JSON.parse(run(root, draftCommand))).toEqual(drafted);
    expect(JSON.parse(run(root, finalizeCommand))).toEqual(drafted);
    expect(await readState()).toEqual(afterFinalize);

    // Preserve support for direct publication without a draft.
    await saveState({
      ...afterFinalize,
      release: null,
      attempts: 0,
      creates: 0,
      finalizes: 0,
    });

    // A failed publication preserves the prepared tag for a retry.
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

    // Repetition returns the same release without another creation.
    const beforeRepeat = await readState();

    expect(JSON.parse(run(root, finalizeCommand))).toEqual(published);
    expect(await readState()).toEqual(beforeRepeat);

    // Repetition also works from a fresh checkout.
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

    // Propagate authorization errors without retrying the ID request.
    const beforeRecoveryTests = await readState();

    // Use the successful POST response without additional list or ID reads.
    await saveState({
      ...beforeRecoveryTests,
      release: null,
      attempts: 0,
      creates: 0,
      finalizes: 0,
      hiddenListReads: 2,
      missingIdReads: 2,
    });

    expect(JSON.parse(run(root, draftCommand))).toEqual(drafted);

    const afterCreation = await readState();

    expect(afterCreation.release?.draft).toBe(true);
    expect(afterCreation.attempts).toBe(1);
    expect(afterCreation.creates).toBe(1);
    expect(afterCreation.finalizes).toBe(0);
    expect(afterCreation.hiddenListReads).toBe(2);
    expect(afterCreation.missingIdReads).toBe(2);

    // Use the successful PATCH response without additional ID reads.
    await saveState({
      ...afterCreation,
      hiddenListReads: 0,
      staleDraftReads: 2,
    });

    expect(JSON.parse(run(root, finalizeCommand))).toEqual(drafted);

    const afterPublication = await readState();

    expect(afterPublication.release?.draft).toBe(false);
    expect(afterPublication.attempts).toBe(1);
    expect(afterPublication.creates).toBe(1);
    expect(afterPublication.finalizes).toBe(1);
    expect(afterPublication.missingIdReads).toBe(2);
    expect(afterPublication.staleDraftReads).toBe(2);

    // Recover a successful creation whose response was lost.
    await saveState({
      ...beforeRecoveryTests,
      release: null,
      attempts: 0,
      creates: 0,
      finalizes: 0,
      loseCreateResponse: true,
      hiddenListReads: 2,
    });

    expect(JSON.parse(run(root, draftCommand))).toEqual(drafted);

    const afterLostCreation = await readState();

    expect(afterLostCreation.release?.draft).toBe(true);
    expect(afterLostCreation.attempts).toBe(1);
    expect(afterLostCreation.creates).toBe(1);
    expect(afterLostCreation.finalizes).toBe(0);
    expect(afterLostCreation.hiddenListReads).toBe(0);

    // Recover a successful publication whose response was lost.
    await saveState({
      ...afterLostCreation,
      loseCreateResponse: false,
      loseFinalizeResponse: true,
      missingIdReads: 1,
      staleDraftReads: 1,
    });

    expect(JSON.parse(run(root, finalizeCommand))).toEqual(drafted);

    const afterLostPublication = await readState();

    expect(afterLostPublication.release?.draft).toBe(false);
    expect(afterLostPublication.attempts).toBe(1);
    expect(afterLostPublication.creates).toBe(1);
    expect(afterLostPublication.finalizes).toBe(1);
    expect(afterLostPublication.missingIdReads).toBe(0);
    expect(afterLostPublication.staleDraftReads).toBe(0);

    // Repetition must not issue another creation or publication request.
    expect(JSON.parse(run(root, draftCommand))).toEqual(drafted);
    expect(JSON.parse(run(root, finalizeCommand))).toEqual(drafted);
    expect(await readState()).toEqual(afterLostPublication);

    // Propagate authorization errors during recovery without retrying them.
    await saveState({
      ...afterLostCreation,
      loseCreateResponse: false,
      loseFinalizeResponse: true,
      forbiddenIdReads: 2,
    });

    const forbidden = execute(root, finalizeCommand);

    expect(forbidden.code).not.toBe(0);
    expect(forbidden.stderr).toContain('HTTP 403');

    const afterForbidden = await readState();

    expect(afterForbidden.forbiddenIdReads).toBe(1);
    expect(afterForbidden.release?.draft).toBe(false);
    expect(afterForbidden.attempts).toBe(1);
    expect(afterForbidden.creates).toBe(1);
    expect(afterForbidden.finalizes).toBe(1);

    // Exhaust recovery retries without repeating a successful mutation.
    await saveState({
      ...afterLostCreation,
      loseCreateResponse: false,
      loseFinalizeResponse: true,
      missingIdReads: 100,
    });

    const unrecovered = execute(root, finalizeCommand);

    expect(unrecovered.code).not.toBe(0);
    expect(unrecovered.stderr).toContain('Simulated lost publication response');

    const afterUnrecovered = await readState();

    expect(afterUnrecovered.release?.draft).toBe(false);
    expect(afterUnrecovered.attempts).toBe(1);
    expect(afterUnrecovered.creates).toBe(1);
    expect(afterUnrecovered.finalizes).toBe(1);
    expect(afterUnrecovered.missingIdReads).toBe(95);

    // A subsequent run recognizes the publication without another mutation.
    await saveState({
      ...afterUnrecovered,
      missingIdReads: 0,
    });

    const beforeResume = await readState();

    expect(JSON.parse(run(root, finalizeCommand))).toEqual(drafted);
    expect(await readState()).toEqual(beforeResume);

    await saveState(beforeRecoveryTests);

    // Conflicting notes must not be silently overwritten.
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
}, 60_000);
