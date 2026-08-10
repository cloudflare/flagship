import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const SDKS = ['typescript', 'python', 'go'] as const;
const RELEASE_COMMIT_SUBJECT = 'chore(release): version SDK packages';
const RELEASE_COMMIT_PATTERN = `^${RELEASE_COMMIT_SUBJECT}`;
type Sdk = (typeof SDKS)[number];
export type SdkChanges = Record<Sdk, boolean>;
const NO_CHANGES: SdkChanges = { typescript: false, python: false, go: false };

export function classifySdkChanges(paths: string[]): SdkChanges {
	const relative = (sdk: Sdk): string[] =>
		paths.filter((path) => path.startsWith(`sdks/${sdk}/`)).map((path) => path.slice(`sdks/${sdk}/`.length));
	const typescript = relative('typescript');
	const python = relative('python');
	const go = relative('go');

	return {
		typescript: typescript.some((path) => path.startsWith('src/') || ['package.json', 'tsconfig.json', 'tsdown.config.ts'].includes(path)),
		python: python.some((path) => path.startsWith('src/') || path === 'pyproject.toml'),
		go: go.some((path) => (!path.includes('/') && path.endsWith('.go') && !path.endsWith('_test.go')) || path === 'go.mod'),
	};
}

export function detectSdkChanges(releaseCommit = 'HEAD', cwd = process.cwd()): SdkChanges {
	if (!isReleaseCommit(cwd, releaseCommit)) return { ...NO_CHANGES };

	const releaseParent = `${releaseCommit}^1`;
	const canonicalTag = describeTag(cwd, '@cloudflare/flagship@*', releaseParent);
	const baselines: Record<Sdk, string> = {
		typescript: canonicalTag,
		python: findSdkTag(cwd, 'sdks/python/v*', releaseParent) ?? canonicalTag,
		go: findSdkTag(cwd, 'sdks/go/v*', releaseParent) ?? canonicalTag,
	};

	return Object.fromEntries(
		SDKS.map((sdk) => [sdk, classifySdkChanges(changedPaths(cwd, baselines[sdk], releaseParent))[sdk]]),
	) as SdkChanges;
}

type ChangesetCommand = ['changeset', 'publish' | 'tag'];

export function publishCommands(changes: SdkChanges): ChangesetCommand[] {
	if (changes.typescript)
		return [
			['changeset', 'publish'],
			['changeset', 'tag'],
		];
	if (changes.python || changes.go) return [['changeset', 'tag']];
	return [];
}

/**
 * Reports whether a commit merged the release PR. `has_changesets == false` holds
 * for every push to `main` with an empty changeset queue, so without this gate an
 * ordinary push would publish an unbumped version and stamp an SDK tag on a
 * non-release commit, poisoning the baseline for later changes.
 *
 * Squash merges and direct pushes carry the subject; merge commits carry it on the
 * second parent.
 */
function isReleaseCommit(cwd: string, commit: string): boolean {
	const subjects = [subject(cwd, commit)];
	if (revisionExists(cwd, `${commit}^2`)) subjects.push(subject(cwd, `${commit}^2`));

	return subjects.some((value) => value.startsWith(RELEASE_COMMIT_SUBJECT));
}

function subject(cwd: string, commit: string): string {
	return git(cwd, 'log', '-1', '--format=%s', commit);
}

function revisionExists(cwd: string, revision: string): boolean {
	try {
		git(cwd, 'rev-parse', '--verify', '--quiet', `${revision}^{commit}`);
		return true;
	} catch {
		return false;
	}
}

/**
 * Collects the files touched between two commits, skipping the mechanical version
 * bumps produced by `.github/changeset-version.ts`. An SDK that is skipped by one
 * release keeps a stale baseline tag, so later windows span earlier release commits
 * and would otherwise republish on generated churn alone.
 *
 * The pattern must match the `commit` input of `changesets/action` in `release.yml`.
 * This is a union of per-commit changes rather than a net diff, so a change that is
 * later reverted still counts — deliberately conservative.
 */
function changedPaths(cwd: string, base: string, head: string): string[] {
	const output = git(cwd, 'log', '--format=', '--name-only', '--invert-grep', '--grep', RELEASE_COMMIT_PATTERN, `${base}..${head}`);
	return [...new Set(output.split('\n').filter(Boolean))];
}

function git(cwd: string, ...args: string[]): string {
	return execFileSync('git', args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function describeTag(cwd: string, pattern: string, commit: string): string {
	return git(cwd, 'describe', '--first-parent', '--tags', '--match', pattern, '--exclude', '*-*', '--abbrev=0', commit);
}

function findSdkTag(cwd: string, pattern: string, commit: string): string | undefined {
	try {
		return describeTag(cwd, pattern, commit);
	} catch {
		return undefined;
	}
}

function writeChanges(changes: SdkChanges): void {
	const lines = [...SDKS.map((sdk) => `${sdk}=${changes[sdk]}`), `any=${Object.values(changes).some(Boolean)}`];
	const output = process.env.GITHUB_OUTPUT;

	if (output) appendFileSync(output, `${lines.join('\n')}\n`);
	console.log(lines.join('\n'));
}

function publish(): void {
	const changes = Object.fromEntries(SDKS.map((sdk) => [sdk, process.env[`${sdk.toUpperCase()}_SDK_CHANGED`] === 'true'])) as SdkChanges;
	const commands = publishCommands(changes);

	if (commands.length === 0) {
		console.log('No SDK source changes detected; skipping release.');
		return;
	}

	if (!changes.typescript) console.log('TypeScript SDK unchanged; creating the canonical release tag without publishing to npm.');
	for (const command of commands) {
		const result = spawnSync('pnpm', command, { stdio: 'inherit' });
		if (result.error) throw result.error;
		if (result.status !== 0) process.exit(result.status ?? 1);
	}
}

async function main(): Promise<void> {
	switch (process.argv[2]) {
		case 'detect':
			writeChanges(detectSdkChanges(process.env.GITHUB_SHA));
			break;
		case 'publish':
			publish();
			break;
		default:
			throw new Error('Expected mode: detect or publish');
	}
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) await main();
