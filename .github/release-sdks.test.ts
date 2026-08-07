import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import assert from 'node:assert/strict';
import test from 'node:test';
import { classifySdkChanges, detectSdkChanges, publishCommands } from './release-sdks.js';

test('classifies changes by SDK directory', () => {
	assert.deepEqual(classifySdkChanges(['sdks/typescript/src/client.ts', 'sdks/go/client.go', 'sdks/python/README.md']), {
		typescript: true,
		python: false,
		go: true,
	});
});

test('ignores tests, examples, and package documentation', () => {
	assert.deepEqual(
		classifySdkChanges([
			'sdks/typescript/tests/client.test.ts',
			'sdks/typescript/README.md',
			'sdks/python/tests/test_client.py',
			'sdks/python/LICENSE',
			'sdks/go/client_test.go',
			'sdks/go/examples/basic/main.go',
		]),
		{ typescript: false, python: false, go: false },
	);
});

test('includes package and build configuration changes but excludes lockfiles', () => {
	assert.deepEqual(classifySdkChanges(['sdks/typescript/package.json', 'sdks/python/pyproject.toml', 'sdks/go/go.mod']), {
		typescript: true,
		python: true,
		go: true,
	});
	assert.deepEqual(classifySdkChanges(['sdks/python/uv.lock', 'sdks/go/go.sum']), {
		typescript: false,
		python: false,
		go: false,
	});
});

test('publishes npm only for TypeScript changes', () => {
	assert.deepEqual(publishCommands({ typescript: true, python: false, go: false }), [
		['changeset', 'publish'],
		['changeset', 'tag'],
	]);
	assert.deepEqual(publishCommands({ typescript: false, python: true, go: false }), [['changeset', 'tag']]);
	assert.deepEqual(publishCommands({ typescript: false, python: false, go: true }), [['changeset', 'tag']]);
	assert.deepEqual(publishCommands({ typescript: false, python: false, go: false }), []);
});

test('ignores mechanical SDK version changes in the release commit', () => {
	const repo = createRepository();
	write(repo, 'sdks/python/src/client.py', 'changed\n');
	commit(repo, 'change python');

	for (const sdk of ['typescript', 'python', 'go']) write(repo, `sdks/${sdk}/package.json`, '{"version":"0.2.0"}\n');
	commit(repo, 'version SDKs');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: true, go: false });
});

test('reports no SDK changes after the release is tagged', () => {
	const repo = createRepository();
	write(repo, 'sdks/go/client.go', 'changed\n');
	commit(repo, 'change go');
	write(repo, 'sdks/go/package.json', '{"version":"0.2.0"}\n');
	commit(repo, 'version SDKs');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	write(repo, 'README.md', 'docs\n');
	commit(repo, 'update docs');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: false });
});

test('uses the first parent of a merged release PR', () => {
	const repo = createRepository();
	const mainBranch = git(repo, 'branch', '--show-current');
	write(repo, 'sdks/go/client.go', 'changed\n');
	commit(repo, 'change go');
	git(repo, 'checkout', '-b', 'release');
	for (const sdk of ['typescript', 'python', 'go']) write(repo, `sdks/${sdk}/package.json`, '{"version":"0.2.0"}\n');
	commit(repo, 'version SDKs');
	git(repo, 'checkout', mainBranch);
	git(repo, 'merge', '--no-ff', 'release', '-m', 'merge release PR');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: true });
});

test('fails safely when the canonical baseline tag is missing', () => {
	const repo = createRepository();
	git(repo, 'tag', '-d', '@cloudflare/flagship@0.1.0');
	write(repo, 'README.md', 'changed\n');
	commit(repo, 'change docs');

	assert.throws(() => detectSdkChanges('HEAD', repo));
});

test('retains unpublished SDK changes across canonical releases', () => {
	const repo = createRepository();
	git(repo, 'tag', 'sdks/go/v0.1.0');
	write(repo, 'sdks/go/client.go', 'changed\n');
	commit(repo, 'change go');
	write(repo, 'sdks/go/package.json', '{"version":"0.2.0"}\n');
	commit(repo, 'version SDKs');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	write(repo, 'README.md', 'next release\n');
	commit(repo, 'prepare next release');
	write(repo, 'sdks/typescript/package.json', '{"version":"0.3.0"}\n');
	commit(repo, 'version SDKs again');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: false, go: true });
});

test('ignores an SDK tag that is not reachable from the release parent', () => {
	const repo = createRepository();
	write(repo, 'sdks/python/src/client.py', 'changed\n');
	commit(repo, 'change python');
	write(repo, 'sdks/python/package.json', '{"version":"0.2.0"}\n');
	commit(repo, 'version SDKs');
	git(repo, 'tag', '@cloudflare/flagship@0.2.0');
	git(repo, 'tag', 'sdks/python/v0.2.0');

	assert.deepEqual(detectSdkChanges('HEAD', repo), { typescript: false, python: true, go: false });
});

function createRepository(): string {
	const repo = mkdtempSync(join(tmpdir(), 'flagship-release-'));
	git(repo, 'init');
	git(repo, 'config', 'user.email', 'test@example.com');
	git(repo, 'config', 'user.name', 'Test');

	for (const sdk of ['typescript', 'python', 'go']) {
		write(repo, `sdks/${sdk}/package.json`, '{"version":"0.1.0"}\n');
		write(repo, `sdks/${sdk}/src/initial`, 'initial\n');
	}
	commit(repo, 'initial release');
	git(repo, 'tag', '@cloudflare/flagship@0.1.0');
	return repo;
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
