import { describe, expect, test } from 'bun:test';
import type { Commit } from '../src/types';
import { calculateBump, nextVersion } from '../src/version';

function commit(type: string, breaking = false): Commit {
  return {
    sha: 'test',
    type,
    description: 'Example change',
    breaking,
  };
}

describe('release policy', () => {
  test('no commits produce no release', () => {
    expect(calculateBump('0.1.2', [])).toBeNull();
  });

  test('maintenance changes alone produce no release', () => {
    const commits = ['build', 'test', 'ci', 'docs', 'chore', 'refactor'].map(
      (type) => commit(type),
    );

    expect(calculateBump('0.1.2', commits)).toBeNull();
  });

  test('fixes produce one patch regardless of their number', () => {
    const commits = [commit('fix'), commit('fix'), commit('test')];
    const bump = calculateBump('0.1.2', commits);

    expect(bump).toBe('patch');
    expect(bump === null ? null : nextVersion('0.1.2', bump)).toBe('0.1.3');
  });

  test('features remain patch releases before 1.0', () => {
    const bump = calculateBump('0.1.2', [commit('feat')]);

    expect(bump).toBe('patch');
    expect(bump === null ? null : nextVersion('0.1.2', bump)).toBe('0.1.3');
  });

  test('breaking changes produce minor releases before 1.0', () => {
    const bump = calculateBump('0.1.2', [commit('refactor', true)]);

    expect(bump).toBe('minor');
    expect(bump === null ? null : nextVersion('0.1.2', bump)).toBe('0.2.0');
  });

  test('fixes produce patch releases after 1.0', () => {
    expect(calculateBump('1.2.3', [commit('fix')])).toBe('patch');
    expect(nextVersion('1.2.3', 'patch')).toBe('1.2.4');
  });

  test('features take priority over fixes after 1.0', () => {
    for (const commits of [
      [commit('fix'), commit('feat')],
      [commit('feat'), commit('fix')],
    ]) {
      const bump = calculateBump('1.2.3', commits);

      expect(bump).toBe('minor');
      expect(bump === null ? null : nextVersion('1.2.3', bump)).toBe('1.3.0');
    }
  });

  test('breaking changes take priority regardless of order', () => {
    const commits = [commit('fix'), commit('docs', true), commit('feat')];

    expect(calculateBump('1.2.3', commits)).toBe('major');
    expect(calculateBump('1.2.3', [...commits].reverse())).toBe('major');
    expect(nextVersion('1.2.3', 'major')).toBe('2.0.0');
  });

  test('unsupported version formats are rejected', () => {
    for (const version of [
      'v1.2.3',
      '1.2',
      '01.2.3',
      '1.2.3-beta.1',
      'invalid',
    ]) {
      expect(() => nextVersion(version, 'patch')).toThrow();
    }
  });
});
