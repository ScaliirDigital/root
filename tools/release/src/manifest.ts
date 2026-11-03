import type { PackageConfig } from './types';
import { parseVersion } from './version';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function bumpManifest(
  source: string,
  manifest: PackageConfig['manifest'],
  currentVersion: string,
  nextVersion: string,
): string {
  parseVersion(currentVersion);
  parseVersion(nextVersion);

  const data: unknown =
    manifest === 'package.json' ? JSON.parse(source) : Bun.TOML.parse(source);

  if (!isRecord(data)) {
    throw new Error(`${manifest}: expected a manifest object`);
  }

  const actualVersion =
    manifest === 'package.json'
      ? data.version
      : isRecord(data.package)
        ? data.package.version
        : undefined;

  if (actualVersion !== currentVersion) {
    throw new Error(
      `${manifest}: expected version ${currentVersion}, found ${String(actualVersion)}`,
    );
  }

  if (manifest === 'package.json') {
    return `${JSON.stringify({ ...data, version: nextVersion }, null, 2)}\n`;
  }

  const section =
    /^([ \t]*\[package\][ \t]*(?:#[^\r\n]*)?\r?\n)([\s\S]*?)(?=^[ \t]*\[|(?![\s\S]))/m;

  const match = section.exec(source);

  if (!match) {
    throw new Error('Cargo.toml: could not locate the [package] section');
  }

  const versionLine =
    /^([ \t]*version[ \t]*=[ \t]*)(["'])([^"'\r\n]*)\2([ \t]*(?:#[^\r\n]*)?)(\r?)$/m;

  let replaced = false;

  const body = match[2].replace(
    versionLine,
    (
      _line: string,
      prefix: string,
      quote: string,
      _version: string,
      suffix: string,
      carriageReturn: string,
    ) => {
      replaced = true;
      return `${prefix}${quote}${nextVersion}${quote}${suffix}${carriageReturn}`;
    },
  );

  if (!replaced) {
    throw new Error(
      'Cargo.toml: expected a single-line version assignment under [package]',
    );
  }

  return (
    source.slice(0, match.index) +
    match[1] +
    body +
    source.slice(match.index + match[0].length)
  );
}
