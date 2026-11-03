import { describe, expect, test } from 'bun:test';
import { validatePackage } from '../src/config';

describe('package policy', () => {
  const rust = {
    language: 'rust',
    targets: ['cargo', 'binary'],
    containers: ['ghcr', 'dockerhub'],
  };

  test('derives Rust package conventions', () => {
    expect(validatePackage('document', rust)).toEqual({
      language: 'rust',
      path: 'packages/document',
      scope: 'document',
      manifest: 'Cargo.toml',
      tag_prefix: 'document-v',
      targets: ['cargo', 'binary'],
      containers: ['ghcr', 'dockerhub'],
    });
  });

  test('derives TypeScript package conventions', () => {
    expect(
      validatePackage('astro-bun', {
        language: 'typescript',
        targets: ['npm'],
      }),
    ).toEqual({
      language: 'typescript',
      path: 'packages/astro-bun',
      scope: 'astro-bun',
      manifest: 'package.json',
      tag_prefix: 'astro-bun-v',
      targets: ['npm'],
      containers: [],
    });
  });

  test('accepts container-only packages', () => {
    expect(
      validatePackage('document', {
        language: 'rust',
        containers: ['ghcr'],
      }).targets,
    ).toEqual([]);
  });

  test('rejects unsupported configuration fields', () => {
    for (const field of ['path', 'scope', 'manifest', 'tag_prefix', 'typo']) {
      expect(() =>
        validatePackage('document', { ...rust, [field]: 'override' }),
      ).toThrow('unknown configuration field');
    }
  });

  test('rejects invalid package names', () => {
    for (const name of ['', '../document', 'Document', 'document/sub']) {
      expect(() => validatePackage(name, rust)).toThrow('Invalid package name');
    }
  });

  test('rejects unknown languages', () => {
    expect(() =>
      validatePackage('document', { ...rust, language: 'unknown' }),
    ).toThrow('language');
  });

  test('rejects target objects instead of destination names', () => {
    expect(() =>
      validatePackage('document', {
        ...rust,
        targets: [{ type: 'cargo' }],
      }),
    ).toThrow('not allowed');
  });

  test('rejects targets incompatible with the language', () => {
    expect(() =>
      validatePackage('document', { ...rust, targets: ['npm'] }),
    ).toThrow('not allowed');
  });

  test('rejects container registries incompatible with the language', () => {
    expect(() =>
      validatePackage('astro-bun', {
        language: 'typescript',
        targets: ['npm'],
        containers: ['ghcr'],
      }),
    ).toThrow('not allowed');
  });

  test('rejects unknown container registries', () => {
    expect(() =>
      validatePackage('document', { ...rust, containers: ['typo'] }),
    ).toThrow('not allowed');
  });

  test('rejects duplicate destinations', () => {
    expect(() =>
      validatePackage('document', { ...rust, targets: ['cargo', 'cargo'] }),
    ).toThrow('duplicate');

    expect(() =>
      validatePackage('document', { ...rust, containers: ['ghcr', 'ghcr'] }),
    ).toThrow('duplicate');
  });

  test('rejects packages without publication destinations', () => {
    expect(() => validatePackage('document', { language: 'rust' })).toThrow(
      'at least one publication destination',
    );
  });

  test('rejects non-array destinations', () => {
    expect(() =>
      validatePackage('document', { ...rust, targets: 'cargo' }),
    ).toThrow('expected an array');
  });
});
