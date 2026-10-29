import { describe, expect, test } from 'bun:test';
import { bumpManifest } from '../src/manifest';

describe('manifest version changes', () => {
  test('updates the package version without changing dependency versions', () => {
    const source = {
      name: '@scale.digital/astro-bun',
      version: '0.1.2',
      dependencies: {
        example: '0.1.2',
      },
      scripts: {
        build: 'bun build src/index.ts',
      },
    };

    const updated = JSON.parse(
      bumpManifest(JSON.stringify(source), 'package.json', '0.1.2', '0.1.3'),
    );

    expect(updated).toEqual({
      ...source,
      version: '0.1.3',
    });
  });

  test('preserves Cargo comments and dependency versions', () => {
    const source = [
      '# Document service',
      '[package]',
      'name = "document"',
      'version = "0.1.0" # Published version',
      'edition = "2024"',
      '',
      '[dependencies]',
      'example = { version = "0.1.0" }',
      '',
    ].join('\n');

    const updated = bumpManifest(source, 'Cargo.toml', '0.1.0', '0.1.1');

    expect(updated).toBe(
      source.replace(
        'version = "0.1.0" # Published version',
        'version = "0.1.1" # Published version',
      ),
    );
  });

  test('preserves Cargo single quotes and Windows line endings', () => {
    const source = "[package]\r\nversion = '0.1.0'\r\n";

    expect(bumpManifest(source, 'Cargo.toml', '0.1.0', '0.1.1')).toBe(
      "[package]\r\nversion = '0.1.1'\r\n",
    );
  });

  test('rejects a manifest that differs from the planned version', () => {
    expect(() =>
      bumpManifest('{"version":"0.2.0"}', 'package.json', '0.1.2', '0.1.3'),
    ).toThrow('expected version 0.1.2');
  });

  test('rejects unsupported target versions', () => {
    expect(() =>
      bumpManifest('{"version":"0.1.2"}', 'package.json', '0.1.2', 'invalid'),
    ).toThrow('Expected a stable version');
  });
});
