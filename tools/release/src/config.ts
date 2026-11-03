import { join } from 'node:path';
import type { ContainerRegistry, PackageConfig, Target } from './types';
import { parseVersion } from './version';

const policy: Record<
  PackageConfig['language'],
  {
    manifest: PackageConfig['manifest'];
    targets: readonly Target[];
    containers: readonly ContainerRegistry[];
  }
> = {
  typescript: {
    manifest: 'package.json',
    targets: ['npm'],
    containers: [],
  },
  rust: {
    manifest: 'Cargo.toml',
    targets: ['cargo', 'binary'],
    containers: ['ghcr', 'dockerhub'],
  },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function validateList<T extends string>(
  name: string,
  value: unknown,
  allowed: readonly T[],
): T[] {
  if (value === undefined) return [];

  if (!Array.isArray(value)) {
    throw new Error(`${name}: expected an array`);
  }

  const choices: readonly string[] = allowed;
  const seen = new Set<string>();

  for (const entry of value) {
    if (typeof entry !== 'string' || !choices.includes(entry)) {
      throw new Error(`${name}: entry is not allowed: ${String(entry)}`);
    }

    if (seen.has(entry)) {
      throw new Error(`${name}: duplicate entry: ${entry}`);
    }

    seen.add(entry);
  }

  return value as T[];
}

export function validatePackage(name: string, value: unknown): PackageConfig {
  if (!/^[a-z][a-z0-9-]*$/.test(name)) {
    throw new Error(`Invalid package name: ${name}`);
  }

  if (!isRecord(value)) {
    throw new Error(`${name}: expected a package table`);
  }

  const fields = ['language', 'targets', 'containers'];

  for (const field of Object.keys(value)) {
    if (!fields.includes(field)) {
      throw new Error(`${name}: unknown configuration field: ${field}`);
    }
  }

  const language = value.language;

  if (language !== 'typescript' && language !== 'rust') {
    throw new Error(`${name}: language must be typescript or rust`);
  }

  const rules = policy[language];

  const targets = validateList(`${name}.targets`, value.targets, rules.targets);

  const containers = validateList(
    `${name}.containers`,
    value.containers,
    rules.containers,
  );

  if (targets.length === 0 && containers.length === 0) {
    throw new Error(
      `${name}: at least one publication destination is required`,
    );
  }

  return {
    language,
    path: `packages/${name}`,
    scope: name,
    manifest: rules.manifest,
    tag_prefix: `${name}-v`,
    targets,
    containers,
  };
}

export async function readPackages(
  root: string,
): Promise<Record<string, PackageConfig>> {
  const config: unknown = Bun.TOML.parse(
    await Bun.file(join(root, 'tools/release/packages.toml')).text(),
  );

  if (
    !isRecord(config) ||
    !isRecord(config.packages) ||
    Object.keys(config.packages).length === 0
  ) {
    throw new Error('Configuration must contain at least one package');
  }

  return Object.fromEntries(
    Object.entries(config.packages).map(([name, value]) => [
      name,
      validatePackage(name, value),
    ]),
  );
}

export async function readVersion(
  root: string,
  pkg: PackageConfig,
): Promise<string> {
  const file = Bun.file(join(root, pkg.path, pkg.manifest));

  const data: unknown =
    pkg.manifest === 'package.json'
      ? await file.json()
      : Bun.TOML.parse(await file.text());

  let version: unknown;

  if (isRecord(data)) {
    if (pkg.manifest === 'package.json') {
      version = data.version;
    } else if (isRecord(data.package)) {
      version = data.package.version;
    }
  }

  if (typeof version !== 'string') {
    throw new Error(`Missing version in ${pkg.path}/${pkg.manifest}`);
  }

  parseVersion(version);
  return version;
}
