import { execFileSync, spawnSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const SDKS = ['typescript', 'python', 'go'] as const;
type Sdk = (typeof SDKS)[number];
export type SdkChanges = Record<Sdk, boolean>;

export function classifySdkChanges(paths: string[]): SdkChanges {
	const relative = (sdk: Sdk): string[] =>
		paths.filter((path) => path.startsWith(`sdks/${sdk}/`)).map((path) => path.slice(`sdks/${sdk}/`.length));
	const typescript = relative('typescript');
	const python = relative('python');
	const go = relative('go');
	const isDocumentation = (path: string): boolean =>
		path.startsWith('examples/') ||
		path.startsWith('docs/') ||
		path.endsWith('.md') ||
		path.slice(path.lastIndexOf('/') + 1).startsWith('LICENSE');

	return {
		typescript: typescript.some(
			(path) => isDocumentation(path) || path.startsWith('src/') || ['package.json', 'tsconfig.json', 'tsdown.config.ts'].includes(path),
		),
		python: python.some((path) => isDocumentation(path) || path.startsWith('src/') || path === 'pyproject.toml'),
		go: go.some(
			(path) => isDocumentation(path) || (!path.includes('/') && path.endsWith('.go') && !path.endsWith('_test.go')) || path === 'go.mod',
		),
	};
}

export function detectSdkChanges(releaseCommit = 'HEAD', cwd = process.cwd()): SdkChanges {
	const releaseParent = `${releaseCommit}^1`;
	const canonicalTag = describeTag(cwd, '@cloudflare/flagship@*', releaseParent);
	const baselines: Record<Sdk, string> = {
		typescript: canonicalTag,
		python: findSdkTag(cwd, 'sdks/python/v*', releaseParent) ?? canonicalTag,
		go: findSdkTag(cwd, 'sdks/go/v*', releaseParent) ?? canonicalTag,
	};

	return Object.fromEntries(
		SDKS.map((sdk) => {
			const paths = git(cwd, 'diff', '--name-only', `${baselines[sdk]}..${releaseParent}`).split('\n').filter(Boolean);
			return [sdk, classifySdkChanges(paths)[sdk]];
		}),
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
