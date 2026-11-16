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
  if (!Number.isSafeInteger(existing.id) || existing.id <= 0) {
    throw new Error(`${release.tag}: invalid GitHub release ID`);
  }

  if (
    existing.tag_name !== release.tag ||
    existing.name !== `${release.package} ${release.version}` ||
    typeof existing.draft !== 'boolean' ||
    existing.prerelease !== false ||
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
  draft: boolean,
): Promise<GitHubRelease> {
  return (await githubApi(`repos/${repository}/releases`, 'POST', {
    tag_name: release.tag,
    target_commitish: release.sha,
    name: `${release.package} ${release.version}`,
    body: release.notes,
    draft,
    prerelease: false,
    make_latest: 'false',
  })) as GitHubRelease;
}

async function finalizeGitHubRelease(
  repository: string,
  existing: GitHubRelease,
): Promise<GitHubRelease> {
  return (await githubApi(
    `repos/${repository}/releases/${existing.id}`,
    'PATCH',
    {
      draft: false,
      make_latest: 'false',
    },
  )) as GitHubRelease;
}

async function confirmRelease(
  repository: string,
  release: Release,
  id: number | undefined,
  requirePublished: boolean,
): Promise<GitHubRelease | undefined> {
  for (const delay of [0, 200, 500, 1_000, 2_000]) {
    if (delay > 0) {
      await Bun.sleep(delay);
    }

    let existing: GitHubRelease | undefined;

    if (id === undefined) {
      existing = await findRelease(repository, release.tag);
    } else {
      try {
        existing = (await githubApi(
          `repos/${repository}/releases/${id}`,
        )) as GitHubRelease;
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);

        // Retry a temporarily missing release, but propagate other API errors.
        if (!/\(HTTP 404\)/.test(message)) {
          throw error;
        }

        continue;
      }
    }

    if (!existing) continue;

    verifyRelease(existing, release);

    if (id !== undefined && existing.id !== id) {
      throw new Error(`${release.tag}: GitHub release ID changed`);
    }

    if (!requirePublished || !existing.draft) {
      return existing;
    }
  }

  return undefined;
}

export async function publish(
  prepare = false,
  packageName?: string,
  draft = false,
) {
  if (prepare && draft) {
    throw new Error('--prepare and --draft cannot be combined');
  }

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

    let current = existingRelease;

    if (!current) {
      try {
        current = await createGitHubRelease(repository, release, draft);
      } catch (error) {
        // Recover an uncertain POST result without repeating the mutation.
        const recovered = await confirmRelease(
          repository,
          release,
          undefined,
          false,
        );

        if (!recovered) throw error;

        current = recovered;
      }
    }

    verifyRelease(current, release);

    if (!draft && current.draft) {
      const id = current.id;

      try {
        current = await finalizeGitHubRelease(repository, current);
      } catch (error) {
        // Recover an uncertain PATCH result without repeating the mutation.
        const recovered = await confirmRelease(repository, release, id, true);

        if (!recovered) throw error;

        current = recovered;
      }

      verifyRelease(current, release);

      if (current.id !== id) {
        throw new Error(`${release.tag}: GitHub release ID changed`);
      }
    }

    if (!draft && current.draft) {
      throw new Error(`${release.tag}: GitHub release is still a draft`);
    }

    if (readTag(root, remote, release.tag) !== release.sha) {
      throw new Error(`${release.tag}: tag changed during publication`);
    }
  }

  console.log(JSON.stringify(releases, null, 2));
}
