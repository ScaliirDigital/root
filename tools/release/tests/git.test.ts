import { describe, expect, test } from 'bun:test';
import { parseCommit } from '../src/git';

describe('commit parsing', () => {
  test('extracts a scoped fix', () => {
    expect(
      parseCommit('abc123', 'fix(document): handle empty input', 'document'),
    ).toEqual({
      sha: 'abc123',
      type: 'fix',
      description: 'handle empty input',
      breaking: false,
    });
  });

  test('supports scopes containing hyphens', () => {
    const commit = parseCommit(
      'abc123',
      'feat(astro-bun): support streaming',
      'astro-bun',
    );

    expect(commit?.type).toBe('feat');
    expect(commit?.description).toBe('support streaming');
  });

  test('recognizes breaking changes in the subject', () => {
    const commit = parseCommit(
      'abc123',
      'refactor(document)!: change request format',
      'document',
    );

    expect(commit?.breaking).toBe(true);
  });

  test('recognizes both breaking-change footer spellings', () => {
    for (const footer of ['BREAKING CHANGE', 'BREAKING-CHANGE']) {
      const commit = parseCommit(
        'abc123',
        `fix(document): change request format\n\n${footer}: data is required`,
        'document',
      );

      expect(commit?.breaking).toBe(true);
    }
  });

  test('excludes unscoped commits, root and other packages', () => {
    for (const message of [
      'fix: update shared tooling',
      'fix(root): update shared tooling',
      'fix(astro-bun): update adapter',
      'fix(document-extra): update another package',
    ]) {
      expect(parseCommit('abc123', message, 'document')).toBeNull();
    }
  });

  test('excludes non-conventional commit subjects', () => {
    for (const message of [
      'Initial Commit!',
      'Revert "fix(document): handle empty input"',
      'fix(document) missing colon',
    ]) {
      expect(parseCommit('abc123', message, 'document')).toBeNull();
    }
  });

  test('does not treat a conventional message in the body as the subject', () => {
    expect(
      parseCommit(
        'abc123',
        'Update documentation\n\nfix(document): example message',
        'document',
      ),
    ).toBeNull();
  });

  test('ordinary commit details do not imply a breaking change', () => {
    const commit = parseCommit(
      'abc123',
      'fix(document): handle empty input\n\nPreserves the request format.',
      'document',
    );

    expect(commit?.breaking).toBe(false);
  });
});
