import type { PackageConfig, Publication } from './types';

export function publicationLinks(
  packageName: string,
  pkg: PackageConfig,
  manifest: string,
  version: string,
  repository?: string,
): Publication[] {
  const publications: Publication[] = [];

  if (pkg.targets.includes('npm')) {
    const data = JSON.parse(manifest);

    if (typeof data.name !== 'string' || !data.name) {
      throw new Error(`${packageName}: missing npm package name`);
    }

    const name = data.name
      .split('/')
      .map((part: string) => encodeURIComponent(part))
      .join('/');

    publications.push({
      category: 'packages',
      label: 'npm',
      url: `https://www.npmjs.com/package/${name}/v/${version}`,
      logo: 'npm',
      color: 'CB3837',
    });
  }

  if (pkg.targets.includes('cargo')) {
    const data = Bun.TOML.parse(manifest);
    const entry = 'package' in data ? data.package : undefined;

    if (
      typeof entry !== 'object' ||
      entry === null ||
      !('name' in entry) ||
      typeof entry.name !== 'string' ||
      !entry.name
    ) {
      throw new Error(`${packageName}: missing Cargo package name`);
    }

    publications.push({
      category: 'packages',
      label: 'crates.io',
      url: `https://crates.io/crates/${encodeURIComponent(entry.name)}/${version}`,
      logo: 'rust',
      color: 'F46623',
    });
  }

  if (repository && pkg.targets.includes('binary')) {
    publications.push({
      category: 'binary',
      label: 'Downloads',
      url: `https://github.com/${repository}/releases/tag/${pkg.tag_prefix}${version}`,
      logo: 'github',
      color: '357EC7',
    });
  }

  if (repository && pkg.containers.includes('ghcr')) {
    const [owner, repo] = repository.split('/');

    publications.push({
      category: 'container',
      label: 'GHCR',
      url: `https://github.com/${owner.toLowerCase()}/${repo}/pkgs/container/${packageName}`,
      logo: 'github',
      color: '181717',
    });
  }

  if (pkg.containers.includes('dockerhub')) {
    publications.push({
      category: 'container',
      label: 'Docker Hub',
      url: `https://hub.docker.com/r/scaledigital/${packageName}/tags?name=${version}`,
      logo: 'docker',
      color: '2496ED',
    });
  }

  return publications;
}
