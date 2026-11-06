import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import assert from 'node:assert/strict';
import test from 'node:test';
import { classifySdkChanges, detectSdkChanges, publishCommands } from './release-sdks.js';

test('classifies changes by SDK directory', () => {
	assert.deepEqual(classifySdkChanges(['sdks/typescript/src/client.ts', 'sdks/go/client.go', 'sdks/rust/src/client.rs', 'README.md']), {
		typescript: true,
		python: false,
		go: true,
		rust: true,
		dotnet: false,
	});
});

test('ignores test-only changes', () => {
	assert.deepEqual(
		classifySdkChanges([
			'sdks/typescript/tests/client.test.ts',
			'sdks/python/tests/test_client.py',
			'sdks/go/client_test.go',
			'sdks/rust/tests/client.rs',
		]),
		{ typescript: false, python: false, go: false, rust: false, dotnet: false },
	);
});

test('ignores examples, documentation, and licenses', () => {
	assert.deepEqual(
		classifySdkChanges([
			'sdks/typescript/README.md',
			'sdks/python/LICENSE',
			'sdks/go/examples/basic/main.go',
			'sdks/rust/README.md',
			'sdks/rust/LICENSE',
		]),
		{
			typescript: false,
			python: false,
			go: false,
			rust: false,
			dotnet: false,
		},
	);
});

test('excludes generated changelogs', () => {
	assert.deepEqual(
		classifySdkChanges(['sdks/typescript/CHANGELOG.md', 'sdks/python/CHANGELOG.md', 'sdks/go/CHANGELOG.md', 'sdks/rust/CHANGELOG.md']),
		{
			typescript: false,
			python: false,
			go: false,
			rust: false,
			dotnet: false,
		},
	);
});

test('includes package and build configuration changes but excludes lockfiles', () => {
	assert.deepEqual(
		classifySdkChanges(['sdks/typescript/package.json', 'sdks/python/pyproject.toml', 'sdks/go/go.mod', 'sdks/rust/Cargo.toml']),
		{
			typescript: true,
			python: true,
			go: true,
			rust: true,
			dotnet: false,
		},
	);
	assert.deepEqual(classifySdkChanges(['sdks/python/uv.lock', 'sdks/go/go.sum', 'sdks/rust/Cargo.lock', 'sdks/rust/package.json']), {
		typescript: false,
		python: false,
		go: false,
		rust: false,
		dotnet: false,
	});
});

test('publishes npm only for TypeScript changes', () => {
	assert.deepEqual(publishCommands({ typescript: true, python: false, go: false, rust: false, dotnet: false }), [
		['changeset', 'publish'],
		['changeset', 'git-tag'],
	]);
	assert.deepEqual(publishCommands({ typescript: false, python: true, go: false, rust: false, dotnet: false }), [['changeset', 'git-tag']]);
	assert.deepEqual(publishCommands({ typescript: false, python: false, go: true, rust: false, dotnet: false }), [['changeset', 'git-tag']]);
	assert.deepEqual(publishCommands({ typescript: false, python: false, go: false, rust: true, dotnet: false }), [['changeset', 'git-tag']]);
	assert.deepEqual(publishCommands({ typescript: false, python: false, go: false, rust: false, dotnet: false }), []);
});

test('ignores mechanical SDK version changes in the release commit', () => {
	const repo = createRepository();
	write(repo, 'sdks/python/src/client.py', 'changed\n');
	commit(repo, 'change python');
	releaseCommit(repo, '0.2.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: true, go: false, rust: false, dotnet: false });
});

test('reports no SDK changes after the release is tagged', () => {
	const repo = createRepository();
	write(repo, 'sdks/go/client.go', 'changed\n');
	commit(repo, 'change go');
	releaseCommit(repo, '0.2.0');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	write(repo, 'README.md', 'docs\n');
	commit(repo, 'update docs');
	releaseCommit(repo, '0.3.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: false, rust: false, dotnet: false });
});

test('reports no changes when the head commit is not a release commit', () => {
	const repo = createRepository();
	write(repo, 'sdks/python/src/client.py', 'changed\n');
	write(repo, 'sdks/go/client.go', 'changed\n');
	write(repo, 'sdks/typescript/src/client.ts', 'changed\n');
	commit(repo, 'feat: change every SDK without a changeset');
	write(repo, 'README.md', 'unrelated\n');
	commit(repo, 'docs: unrelated follow-up');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: false, rust: false, dotnet: false });
});

test('does not publish native SDKs for documentation changes', () => {
	const repo = createRepository();
	git(repo, 'tag', 'sdks/go/v0.1.0');
	write(repo, 'sdks/go/README.md', 'changed\n');
	write(repo, 'sdks/go/LICENSE', 'changed\n');
	write(repo, 'sdks/python/README.md', 'changed\n');
	commit(repo, 'docs: update native SDK documentation');
	write(repo, 'sdks/typescript/src/client.ts', 'changed\n');
	commit(repo, 'feat: change typescript');
	releaseCommit(repo, '0.2.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: true, python: false, go: false, rust: false, dotnet: false });
});

test('uses the first parent of a merged release PR', () => {
	const repo = createRepository();
	const mainBranch = git(repo, 'branch', '--show-current');
	write(repo, 'sdks/go/client.go', 'changed\n');
	commit(repo, 'change go');
	git(repo, 'checkout', '-b', 'release');
	releaseCommit(repo, '0.2.0');
	git(repo, 'checkout', mainBranch);
	git(repo, 'merge', '--no-ff', 'release', '-m', 'merge release PR');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: true, rust: false, dotnet: false });
});

test('fails safely when the canonical baseline tag is missing', () => {
	const repo = createRepository();
	git(repo, 'tag', '-d', '@cloudflare/flagship@0.1.0');
	write(repo, 'README.md', 'changed\n');
	commit(repo, 'change docs');
	releaseCommit(repo, '0.2.0');

	assert.throws(() => detectSdkChanges('HEAD', repo));
});

test('retains unpublished SDK changes across canonical releases', () => {
	const repo = createRepository();
	git(repo, 'tag', 'sdks/go/v0.1.0');
	write(repo, 'sdks/go/client.go', 'changed\n');
	commit(repo, 'change go');
	releaseCommit(repo, '0.2.0');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	write(repo, 'README.md', 'next release\n');
	commit(repo, 'prepare next release');
	releaseCommit(repo, '0.3.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: true, rust: false, dotnet: false });
});

test('retains Python changes after an unsuccessful first publish without a Python tag', () => {
	const repo = createRepository();
	git(repo, 'tag', '-d', 'sdks/python/v0.1.0');
	git(repo, 'tag', '@cloudflare/flagship@0.4.0');
	write(repo, 'sdks/python/src/client.py', 'changed\n');
	commit(repo, 'change python');
	releaseCommit(repo, '0.5.0');
	git(repo, 'tag', '@cloudflare/flagship@0.5.0');
	write(repo, 'README.md', 'next release\n');
	commit(repo, 'prepare next release');
	releaseCommit(repo, '0.6.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: true, go: false, rust: false, dotnet: false });
});

test('retains Rust changes after an unsuccessful first publish without a Rust tag', () => {
	const repo = createRepository();
	git(repo, 'tag', '-d', 'sdks/rust/v0.1.0');
	releaseCommit(repo, '0.6.0');
	git(repo, 'tag', '@cloudflare/flagship@0.6.0');
	write(repo, 'sdks/rust/src/client.rs', 'changed\n');
	commit(repo, 'add rust sdk');
	releaseCommit(repo, '0.7.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: false, rust: true, dotnet: false });

	// The 0.7.0 crates.io publish failed, so no Rust tag exists; a docs-only release still publishes Rust.
	git(repo, 'tag', '@cloudflare/flagship@0.7.0');
	write(repo, 'README.md', 'next release\n');
	commit(repo, 'prepare next release');
	releaseCommit(repo, '0.8.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: false, rust: true, dotnet: false });

	git(repo, 'tag', '@cloudflare/flagship@0.8.0');
	git(repo, 'tag', 'sdks/rust/v0.8.0');
	write(repo, 'README.md', 'after rust publish\n');
	commit(repo, 'prepare release after rust publish');
	releaseCommit(repo, '0.9.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: false, rust: false, dotnet: false });
});

test('ignores release commits inside a stale SDK baseline window', () => {
	const repo = createRepository();
	git(repo, 'tag', 'sdks/go/v0.1.0');
	write(repo, 'sdks/typescript/src/client.ts', 'changed\n');
	commit(repo, 'change typescript');
	releaseCommit(repo, '0.2.0');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	write(repo, 'sdks/typescript/src/client.ts', 'changed again\n');
	commit(repo, 'change typescript again');
	releaseCommit(repo, '0.3.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: true, python: false, go: false, rust: false, dotnet: false });
});

test('ignores an SDK tag that is not reachable from the release parent', () => {
	const repo = createRepository();
	write(repo, 'sdks/python/src/client.py', 'changed\n');
	commit(repo, 'change python');
	releaseCommit(repo, '0.2.0');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	git(repo, 'tag', 'sdks/python/v0.2.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: true, go: false, rust: false, dotnet: false });
});

test('classifies .NET source and build changes without publishing tests or docs', () => {
	for (const path of [
		'src/Cloudflare.Flagship/FlagshipClient.cs',
		'src/Cloudflare.Flagship.OpenFeature/Cloudflare.Flagship.OpenFeature.csproj',
		'Directory.Build.props',
		'Directory.Build.targets',
		'global.json',
		'NuGet.Config',
	]) {
		assert.equal(classifySdkChanges([`sdks/dotnet/${path}`]).dotnet, true, path);
	}
	for (const path of [
		'tests/Cloudflare.Flagship.Tests/ClientTests.cs',
		'examples/ConsoleExample/Program.cs',
		'README.md',
		'PUBLISHING.md',
		'CHANGELOG.md',
		'LICENSE',
		'package.json',
		'Flagship.sln',
		'packages.lock.json',
	]) {
		assert.equal(classifySdkChanges([`sdks/dotnet/${path}`]).dotnet, false, path);
	}
	assert.deepEqual(publishCommands({ typescript: false, python: false, go: false, rust: false, dotnet: true }), [['changeset', 'git-tag']]);
});

test('keeps the first NuGet publication eligible across releases without credentials', () => {
	const repo = createRepository();
	write(repo, 'sdks/dotnet/src/Cloudflare.Flagship/Client.cs', 'initial\n');
	write(repo, 'sdks/dotnet/package.json', '{"version":"0.1.0"}\n');
	commit(repo, 'add dotnet SDK');

	assert.equal(detectSdkChanges('HEAD', repo).dotnet, false);
	releaseCommit(repo, '0.2.0');
	assert.equal(detectSdkChanges('HEAD', repo).dotnet, true);
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');

	write(repo, 'README.md', 'next release\n');
	commit(repo, 'update docs');
	releaseCommit(repo, '0.3.0');
	assert.equal(detectSdkChanges('HEAD', repo).dotnet, true);
});

test('only successful NuGet publication advances the .NET baseline', () => {
	const repo = createRepository();
	write(repo, 'sdks/dotnet/src/Cloudflare.Flagship/Client.cs', 'initial\n');
	commit(repo, 'add dotnet SDK');
	releaseCommit(repo, '0.2.0');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	git(repo, 'tag', 'sdks/dotnet/v0.2.0');

	// Re-running the same release remains safe and eligible for --skip-duplicate.
	assert.equal(detectSdkChanges('HEAD', repo).dotnet, true);
	write(repo, 'sdks/dotnet/README.md', 'docs\n');
	write(repo, 'sdks/dotnet/tests/ClientTests.cs', 'tests\n');
	commit(repo, 'update dotnet docs and tests');
	releaseCommit(repo, '0.3.0');
	assert.equal(detectSdkChanges('HEAD', repo).dotnet, false);
	git(repo, 'tag', '@cloudflare/flagship@0.3.0');

	write(repo, 'sdks/dotnet/src/Cloudflare.Flagship/Client.cs', 'changed\n');
	commit(repo, 'change dotnet');
	releaseCommit(repo, '0.4.0');
	assert.equal(detectSdkChanges('HEAD', repo).dotnet, true);
	git(repo, 'tag', '@cloudflare/flagship@0.4.0');

	// A failed or skipped push must survive a later canonical release.
	write(repo, 'README.md', 'later release\n');
	commit(repo, 'update docs');
	releaseCommit(repo, '0.5.0');
	assert.equal(detectSdkChanges('HEAD', repo).dotnet, true);
});

function createRepository(): string {
	const repo = mkdtempSync(join(tmpdir(), 'flagship-release-'));
	git(repo, 'init');
	git(repo, 'config', 'user.email', 'test@example.com');
	git(repo, 'config', 'user.name', 'Test');

	for (const sdk of ['typescript', 'python', 'go', 'rust']) {
		write(repo, `sdks/${sdk}/package.json`, '{"version":"0.1.0"}\n');
		write(repo, `sdks/${sdk}/src/initial`, 'initial\n');
	}
	write(repo, 'sdks/rust/Cargo.toml', '[package]\nversion = "0.1.0"\n');
	write(repo, 'sdks/rust/Cargo.lock', 'version = 4\n');
	commit(repo, 'initial release');
	git(repo, 'tag', '@cloudflare/flagship@0.1.0');
	git(repo, 'tag', 'sdks/python/v0.1.0');
	git(repo, 'tag', 'sdks/rust/v0.1.0');
	return repo;
}

/** Mirrors what `.github/changeset-version.ts` writes, including the release commit message. */
function releaseCommit(repo: string, version: string): void {
	for (const sdk of ['typescript', 'python', 'go', 'rust', 'dotnet']) {
		write(repo, `sdks/${sdk}/package.json`, `{"version":"${version}"}\n`);
		write(repo, `sdks/${sdk}/CHANGELOG.md`, `## ${version}\n`);
	}
	write(repo, 'sdks/python/pyproject.toml', `version = "${version}"\n`);
	write(repo, 'sdks/python/uv.lock', `version = "${version}"\n`);
	write(repo, 'sdks/rust/Cargo.toml', `[package]\nversion = "${version}"\n`);
	write(repo, 'sdks/rust/Cargo.lock', 'version = 4\n');
	commit(repo, `chore(release): version SDK packages (#${version.replaceAll('.', '')})`);
}

function write(repo: string, path: string, content: string): void {
	const file = join(repo, path);
	mkdirSync(dirname(file), { recursive: true });
	writeFileSync(file, content);
}

function commit(repo: string, message: string): void {
	git(repo, 'add', '.');
	git(repo, 'commit', '-m', message);
}

function git(repo: string, ...args: string[]): string {
	return execFileSync('git', args, { cwd: repo, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
