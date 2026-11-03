export type Target = 'npm' | 'cargo' | 'binary';

export type ContainerRegistry = 'ghcr' | 'dockerhub';

export type PackageConfig = {
  language: 'typescript' | 'rust';
  path: string;
  scope: string;
  manifest: 'package.json' | 'Cargo.toml';
  tag_prefix: string;
  targets: Target[];
  containers: ContainerRegistry[];
};

export type Author = {
  login: string;
  isBot: boolean;
};

export type PullRequest = {
  number: number;
  url: string;
};

export type Commit = {
  sha: string;
  type: string;
  description: string;
  breaking: boolean;
  url?: string;
  author?: Author;
  pullRequest?: PullRequest;
};

export type Bump = 'patch' | 'minor' | 'major';

export type FileChange = {
  path: string;
  before: string | null;
  after: string;
};

export type ReleasePlan = {
  package: string;
  language: PackageConfig['language'];
  targets: Target[];
  containers: ContainerRegistry[];
  baseTag: string;
  baseSha: string;
  headSha: string;
  currentVersion: string;
  nextVersion: string | null;
  bump: Bump | null;
  commits: Commit[];
  branch?: BranchPlan | null;
  newContributors?: Author[];
  changes?: FileChange[];
  publications?: Publication[];
};

export type BranchPlan = {
  name: string;
  action: 'create' | 'update' | 'unchanged';
  expectedSha: string | null;
};

export type Publication = {
  category: 'packages' | 'binary' | 'container';
  label: string;
  url: string;
  logo: string;
  color: string;
};
