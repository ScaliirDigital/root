import { create } from './cli/create';
import { plan } from './cli/plan';
import { publish } from './cli/publish';

async function main() {
  const [command, ...flags] = Bun.argv.slice(2);
  const markdown = flags.includes('--markdown');
  const github = flags.includes('--github');
  const prepare = flags.includes('--prepare');

  const packageFlags = flags.filter((flag) => flag.startsWith('--package='));
  const packageName = packageFlags[0]?.slice('--package='.length);

  const valid =
    new Set(flags).size === flags.length &&
    (command === 'plan'
      ? flags.every((flag) => flag === '--markdown' || flag === '--github')
      : command === 'create'
        ? flags.every((flag) => flag === '--github')
        : command === 'publish'
          ? flags.every(
              (flag) =>
                flag === '--prepare' ||
                /^--package=[a-z][a-z0-9-]*$/.test(flag),
            ) &&
            packageFlags.length <= 1 &&
            !(prepare && packageName !== undefined)
          : false);

  if (!valid) {
    throw new Error(
      [
        'Usage:',
        '  release plan [--markdown] [--github]',
        '  release create [--github]',
        '  release publish --prepare',
        '  release publish [--package=name]',
        '',
        '--github and publish require GH_REPO=owner/name.',
        'create --github pushes branches and updates release PRs.',
        'publish --prepare creates and verifies release tags.',
        'publish creates and verifies GitHub releases after external publishing.',
      ].join('\n'),
    );
  }

  switch (command) {
    case 'plan':
      await plan(github, markdown);
      break;

    case 'create':
      await create(github);
      break;

    case 'publish':
      await publish(prepare, packageName);
      break;
  }
}

if (import.meta.main) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
