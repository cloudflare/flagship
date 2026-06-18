/**
 * Release automation for the Flagship multi-language SDK monorepo.
 *
 * Modes:
 *   - `validate`: rejects malformed changesets and `none`-bumped SDK changesets. Run in PR CI.
 *   - `release`:  validates, rewrites SDK changesets to the canonical npm package,
 *                 runs `changeset version`, syncs private SDK versions and native
 *                 manifests, refreshes native and pnpm lockfiles. Run by changesets/action.
 *
 * An SDK is any direct subdirectory of `sdks/` containing a `package.json`.
 */

import { execSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import readChangesets from '@changesets/read';
import type { NewChangeset, Release, VersionType } from '@changesets/types';
import { parse as parseToml, patch as patchToml } from '@decimalturn/toml-patch';
import { getPackagesSync, type Package } from '@manypkg/get-packages';

const ROOT = process.cwd();
const CANONICAL_PACKAGE_NAME = '@cloudflare/flagship';
const BUMP_RANK: Record<VersionType, number> = { none: 0, patch: 1, minor: 2, major: 3 };

function readSdkPackages(): Package[] {
	const { packages } = getPackagesSync(ROOT);
	return packages.filter((pkg) => pkg.relativeDir.startsWith('sdks/') && !pkg.relativeDir.slice('sdks/'.length).includes('/'));
}

export async function validatePendingChangesets(): Promise<void> {
	const sdkNames = new Set(readSdkPackages().map((pkg) => pkg.packageJson.name));
	const changesets = await readChangesets(ROOT);
	const errors: string[] = [];

	for (const { id, releases } of changesets) {
		const path = `.changeset/${id}.md`;

		if (releases.length === 0) {
			errors.push(`${path}: declares no package bumps.`);
			continue;
		}

		const nonSdkNames = releases.filter((release) => !sdkNames.has(release.name)).map((release) => release.name);
		if (nonSdkNames.length > 0) {
			errors.push(`${path}: references non-SDK package(s): ${nonSdkNames.join(', ')}. Every changeset must target SDK packages only.`);
		}

		const sdkReleases = releases.filter((release) => sdkNames.has(release.name));
		if (sdkReleases.length === 0) {
			errors.push(`${path}: does not bump any SDK package.`);
		}

		const noneBumps = sdkReleases.filter((release) => release.type === 'none').map((release) => release.name);
		if (noneBumps.length > 0) {
			errors.push(`${path}: SDK package(s) bumped as 'none': ${noneBumps.join(', ')}. Use 'patch', 'minor', or 'major'.`);
		}
	}

	if (errors.length > 0) {
		throw new Error(`Changeset validation failed:\n  - ${errors.join('\n  - ')}`);
	}

	console.log(changesets.length === 0 ? 'No pending changesets to validate.' : `Validated ${changesets.length} pending changeset(s).`);
}

function readCanonicalPackage(): Package {
	const canonical = readSdkPackages().find((pkg) => pkg.packageJson.name === CANONICAL_PACKAGE_NAME);
	if (canonical === undefined) {
		throw new Error(`Canonical SDK package not found: ${CANONICAL_PACKAGE_NAME}`);
	}
	return canonical;
}

export async function rewriteChangesetsToCanonicalPackage(): Promise<void> {
	const sdkNames = new Set(readSdkPackages().map((pkg) => pkg.packageJson.name));

	for (const changeset of await readChangesets(ROOT)) {
		const sdkReleases = changeset.releases.filter((release) => sdkNames.has(release.name));
		if (sdkReleases.length === 0) continue;

		const bump = highestBump(sdkReleases.map((release) => release.type));
		const rewritten: Release[] = [{ name: CANONICAL_PACKAGE_NAME, type: bump }];
		const alreadyCanonical =
			changeset.releases.length === rewritten.length &&
			changeset.releases[0]?.name === CANONICAL_PACKAGE_NAME &&
			changeset.releases[0]?.type === bump;
		if (alreadyCanonical) continue;

		writeChangesetFile({ id: changeset.id, summary: changeset.summary, releases: rewritten });
		console.log(`Rewrote .changeset/${changeset.id}.md to ${CANONICAL_PACKAGE_NAME} at bump '${bump}'.`);
	}
}

function highestBump(bumps: VersionType[]): VersionType {
	return bumps.reduce<VersionType>((highest, bump) => (BUMP_RANK[bump] > BUMP_RANK[highest] ? bump : highest), 'none');
}

function writeChangesetFile({ id, summary, releases }: NewChangeset): void {
	const frontmatter = releases.map((release) => `"${release.name}": ${release.type}`).join('\n');
	writeFileSync(join(ROOT, '.changeset', `${id}.md`), `---\n${frontmatter}\n---\n\n${summary}\n`);
}

export function syncPrivateSdkPackageVersions(): void {
	const targetVersion = readCanonicalPackage().packageJson.version;

	for (const pkg of readSdkPackages()) {
		if (pkg.packageJson.private !== true) continue;
		if (pkg.packageJson.version === targetVersion) continue;

		const packageJsonPath = join(pkg.dir, 'package.json');
		const packageJson = `${JSON.stringify({ ...pkg.packageJson, version: targetVersion }, null, '\t')}\n`;
		writeFileSync(packageJsonPath, packageJson);
		console.log(`Synced ${relative(ROOT, packageJsonPath)} -> ${targetVersion}.`);
	}
}

export function syncNativeManifests(): void {
	const errors: string[] = [];

	for (const pkg of readSdkPackages()) {
		const targetVersion = pkg.packageJson.version;
		syncPythonPackageVersion(pkg.dir, targetVersion, errors);
		syncRustPackageVersion(pkg.dir, targetVersion, errors);
	}

	if (errors.length > 0) {
		throw new Error(`Native manifest sync failed:\n  - ${errors.join('\n  - ')}`);
	}
}

export function refreshNativeLockfiles(): void {
	for (const pkg of readSdkPackages()) {
		const pyprojectPath = join(pkg.dir, 'pyproject.toml');
		const uvLockPath = join(pkg.dir, 'uv.lock');
		if (existsSync(pyprojectPath) && existsSync(uvLockPath)) {
			try {
				execSync('uv lock', { cwd: pkg.dir, stdio: 'inherit' });
				console.log(`Refreshed ${relative(ROOT, uvLockPath)}.`);
			} catch (error) {
				throw new Error(`Failed to refresh ${relative(ROOT, uvLockPath)}: ${error instanceof Error ? error.message : String(error)}`);
			}
		}
	}
}

type PyProjectToml = { project?: { version?: string }; tool?: { poetry?: { version?: string } } };
type CargoToml = { package?: { version?: string } };

function syncPythonPackageVersion(packageDir: string, targetVersion: string, errors: string[]): void {
	patchTomlField(join(packageDir, 'pyproject.toml'), targetVersion, errors, (parsed: PyProjectToml) => {
		const hasProjectVersion = parsed.project?.version !== undefined;
		const hasPoetryVersion = parsed.tool?.poetry?.version !== undefined;
		if (!hasProjectVersion && !hasPoetryVersion) {
			return '[project].version (PEP 621) or [tool.poetry].version (legacy Poetry)';
		}
		if (parsed.project?.version !== undefined) parsed.project.version = targetVersion;
		if (parsed.tool?.poetry?.version !== undefined) parsed.tool.poetry.version = targetVersion;
		return undefined;
	});
}

function syncRustPackageVersion(packageDir: string, targetVersion: string, errors: string[]): void {
	patchTomlField(join(packageDir, 'Cargo.toml'), targetVersion, errors, (parsed: CargoToml) => {
		if (parsed.package?.version === undefined) return '[package].version';
		parsed.package.version = targetVersion;
		return undefined;
	});
}

function patchTomlField<T>(manifestPath: string, targetVersion: string, errors: string[], mutate: (parsed: T) => string | undefined): void {
	if (!existsSync(manifestPath)) return;
	const relativePath = relative(ROOT, manifestPath);
	const original = readFileSync(manifestPath, 'utf8');
	const parsed = parseToml(original) as T;

	const missingField = mutate(parsed);
	if (missingField !== undefined) {
		errors.push(`${relativePath}: no ${missingField} field found. Add one so release automation can sync it.`);
		return;
	}

	const updated = patchToml(original, parsed);
	if (updated === original) {
		console.log(`${relativePath} already at ${targetVersion}.`);
		return;
	}

	writeFileSync(manifestPath, updated);
	console.log(`Synced ${relativePath} -> ${targetVersion}.`);
}

/**
 * `changesets/action` reads every changed package's `CHANGELOG.md` to build the
 * release PR body — including private packages whose version we bumped manually.
 * If the file is missing the action crashes with `ENOENT`.
 *
 * Write a minimal stub for every private SDK so the action can parse it. The
 * stub contains only the version header, so the parser returns empty release
 * content and the PR body has no per-language duplicate section.
 */
function stubPrivateSdkChangelogs(): void {
	const targetVersion = readCanonicalPackage().packageJson.version;
	for (const pkg of readSdkPackages()) {
		if (pkg.packageJson.private !== true) continue;
		const changelog = join(pkg.dir, 'CHANGELOG.md');
		const content = `# ${pkg.packageJson.name}\n\n## ${targetVersion}\n`;
		writeFileSync(changelog, content);
		console.log(`Stubbed ${relative(ROOT, changelog)} for ${pkg.packageJson.name}@${targetVersion}.`);
	}
}

async function runRelease(): Promise<void> {
	await validatePendingChangesets();

	const changesets = await readChangesets(ROOT);
	const snapshot = new Map(
		changesets.map(({ id }) => {
			const file = join(ROOT, '.changeset', `${id}.md`);
			return [file, readFileSync(file, 'utf8')];
		}),
	);

	try {
		await rewriteChangesetsToCanonicalPackage();
		execSync('pnpm changeset version', { stdio: 'inherit' });
	} catch (error) {
		for (const [file, content] of snapshot) {
			writeFileSync(file, content);
		}
		throw error;
	}

	syncPrivateSdkPackageVersions();
	syncNativeManifests();
	refreshNativeLockfiles();
	stubPrivateSdkChangelogs();
	execSync('pnpm install --no-frozen-lockfile', { stdio: 'inherit' });
}

const mode = process.argv[2] ?? 'release';
if (mode === 'release') {
	await runRelease();
} else if (mode === 'validate') {
	await validatePendingChangesets();
} else {
	throw new Error(`Unknown mode: ${mode}. Expected 'release' or 'validate'.`);
}
