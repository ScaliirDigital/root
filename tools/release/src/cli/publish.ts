import { git } from '../git';
import { githubApi } from '../github';
import { detectReleases, type Release } from '../release';

type GitHubRelease = {
  id: number;
  tag_name: string;
  name: string;
  body: string | null;
  draft: boolean;
  prerelease: boolean;
};

async function findRelease(
  repository: string,
  tag: string,
): Promise<GitHubRelease | undefined> {
  for (let page = 1; ; page += 1) {
    const response = await githubApi(
      `repos/${repository}/releases?per_page=100&page=${page}`,
    );

    if (!Array.isArray(response)) {
      throw new Error('Expected a release list from GitHub');
    }

    const releases = response as GitHubRelease[];
    const matching = releases.filter((release) => release.tag_name === tag);

    if (matching.length > 1) {
      throw new Error(`${tag}: multiple matching GitHub releases`);
    }

    if (matching[0]) return matching[0];
    if (releases.length < 100) return undefined;
  }
}

function readTag(root: string, remote: string, tag: string): string | null {
  const ref = `refs/tags/${tag}`;

  git({ cwd: root }, 'check-ref-format', ref);

  const output = git(
    { cwd: root },
    'ls-remote',
    remote,
    ref,
    `${ref}^{}`,
  ).trim();

  const refs = new Map(
    output
      .split('\n')
      .filter(Boolean)
      .map((line): [string, string] => {
        const [sha, name] = line.split('\t');

        if (!sha || !name) {
          throw new Error(`${tag}: invalid remote tag response`);
        }

        return [name, sha];
      }),
  );

  return refs.get(`${ref}^{}`) ?? refs.get(ref) ?? null;
}

function verifyRelease(existing: GitHubRelease, release: Release): void {
  if (
    existing.tag_name !== release.tag ||
    existing.name !== `${release.package} ${release.version}` ||
    existing.draft ||
    existing.prerelease ||
    (existing.body ?? '').trim() !== release.notes.trim()
  ) {
    throw new Error(
      `${release.tag}: existing GitHub release does not match the planned release`,
    );
  }
}

async function createGitHubRelease(
  repository: string,
  release: Release,
): Promise<void> {
  const child = Bun.spawn(
    [
      'gh',
      'api',
      '--hostname',
      'github.com',
      '--method',
      'POST',
      `repos/${repository}/releases`,
      '--input',
      '-',
    ],
    {
      stdin: Buffer.from(
        JSON.stringify({
          tag_name: release.tag,
          target_commitish: release.sha,
          name: `${release.package} ${release.version}`,
          body: release.notes,
          draft: false,
          prerelease: false,
          make_latest: 'false',
        }),
      ),
      stdout: 'pipe',
      stderr: 'pipe',
    },
  );

  const [, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);

  if (exitCode !== 0) {
    throw new Error(
      stderr.trim() || `${release.tag}: GitHub release creation failed`,
    );
  }
}

export async function publish(prepare = false, packageName?: string) {
  const repository = process.env.GH_REPO ?? process.env.GITHUB_REPOSITORY;

  if (!repository || !/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(repository)) {
    throw new Error('publish requires GH_REPO=owner/name');
  }

  const root = git('rev-parse', '--show-toplevel').trim();
  const remote = `https://github.com/${repository}.git`;
  const detected = await detectReleases(root, repository);

  const releases =
    packageName === undefined
      ? detected
      : detected.filter((release) => release.package === packageName);

  if (packageName !== undefined && releases.length !== 1) {
    throw new Error(`${packageName}: expected one matching merged release`);
  }

  for (const release of releases) {
    const existingTag = readTag(root, remote, release.tag);
    const existingRelease = await findRelease(repository, release.tag);

    if (existingTag !== null && existingTag !== release.sha) {
      throw new Error(
        `${release.tag}: existing tag points to a different commit`,
      );
    }

    if (existingRelease) {
      verifyRelease(existingRelease, release);

      if (existingTag === null) {
        throw new Error(
          `${release.tag}: GitHub release exists but its tag is missing`,
        );
      }
    }

    if (existingTag === null) {
      if (!prepare) {
        throw new Error(
          `${release.tag}: release tag is missing; run publish --prepare first`,
        );
      }

      const ref = `refs/tags/${release.tag}`;

      try {
        git(
          { cwd: root },
          'push',
          '--porcelain',
          `--force-with-lease=${ref}:`,
          remote,
          `${release.sha}:${ref}`,
        );
      } catch (error) {
        if (readTag(root, remote, release.tag) !== release.sha) {
          throw error;
        }
      }
    }

    if (readTag(root, remote, release.tag) !== release.sha) {
      throw new Error(`${release.tag}: remote tag does not match release`);
    }

    const ref = `refs/tags/${release.tag}`;

    git(
      { cwd: root },
      'fetch',
      '--no-tags',
      '--no-write-fetch-head',
      remote,
      `${ref}:${ref}`,
    );

    const localSha = git({ cwd: root }, 'rev-parse', `${ref}^{commit}`).trim();

    if (localSha !== release.sha) {
      throw new Error(`${release.tag}: local tag points to a different commit`);
    }

    if (prepare) continue;

    if (!existingRelease) {
      try {
        await createGitHubRelease(repository, release);
      } catch (error) {
        const concurrent = await findRelease(repository, release.tag);

        if (!concurrent) throw error;

        verifyRelease(concurrent, release);
      }
    }

    const confirmed = await findRelease(repository, release.tag);

    if (!confirmed) {
      throw new Error(`${release.tag}: GitHub release is missing`);
    }

    verifyRelease(confirmed, release);

    if (readTag(root, remote, release.tag) !== release.sha) {
      throw new Error(`${release.tag}: tag changed during publication`);
    }
  }

  console.log(JSON.stringify(releases, null, 2));
}
