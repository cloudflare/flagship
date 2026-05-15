/**
 * Release automation for the Flagship multi-language SDK monorepo.
 *
 * Modes:
 *   - `validate`: rejects malformed changesets and `none`-bumped SDK changesets. Run in PR CI.
 *   - `release`:  validates, expands every changeset to all SDKs, runs `changeset version`,
 *                 syncs native manifests, refreshes the lockfile. Run by changesets/action.
 *
 * An SDK is any direct subdirectory of `sdks/` containing a `package.json`.
 */

import { execSync } from 'node:child_process';
import { existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import readChangesets from '@changesets/read';
import type { NewChangeset, Release, VersionType } from '@changesets/types';
import { parse as parseToml, patch as patchToml } from '@decimalturn/toml-patch';
import { getPackagesSync, type Package } from '@manypkg/get-packages';

const ROOT = process.cwd();
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

/** Every release-bound changeset is rewritten to bump every SDK at the highest SDK bump present. */
export async function expandChangesetsToAllSdks(): Promise<void> {
	const allSdkNames = readSdkPackages()
		.map((pkg) => pkg.packageJson.name)
		.sort();
	const sdkNames = new Set(allSdkNames);

	for (const changeset of await readChangesets(ROOT)) {
		const sdkReleases = changeset.releases.filter((release) => sdkNames.has(release.name));
		if (sdkReleases.length === 0) continue;

		const present = new Set(changeset.releases.map((release) => release.name));
		const missing = allSdkNames.filter((name) => !present.has(name));
		if (missing.length === 0) continue;

		const bump = highestBump(sdkReleases.map((release) => release.type));
		const expanded: Release[] = [...changeset.releases, ...missing.map((name) => ({ name, type: bump }))];

		writeChangesetFile({ id: changeset.id, summary: changeset.summary, releases: expanded });
		console.log(`Expanded .changeset/${changeset.id}.md to all SDKs at bump '${bump}'.`);
	}
}

function highestBump(bumps: VersionType[]): VersionType {
	return bumps.reduce<VersionType>((highest, bump) => (BUMP_RANK[bump] > BUMP_RANK[highest] ? bump : highest), 'none');
}

function writeChangesetFile({ id, summary, releases }: NewChangeset): void {
	const frontmatter = releases.map((release) => `"${release.name}": ${release.type}`).join('\n');
	writeFileSync(join(ROOT, '.changeset', `${id}.md`), `---\n${frontmatter}\n---\n\n${summary}\n`);
}

/** Mirrors each SDK's `package.json` version into the native manifest beside it, if present. */
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

type PyProjectToml = { project?: { version?: string }; tool?: { poetry?: { version?: string } } };
type CargoToml = { package?: { version?: string } };

/**
 * Syncs the version for any PEP 621 pyproject (uv, hatch, flit, pdm, setuptools, Poetry 2.0+)
 * via `[project].version`, and also `[tool.poetry].version` for legacy Poetry 1.x layouts.
 */
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

/** Mutates parsed TOML and patches the file in place, preserving comments and formatting. */
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
 * Changesets generates a CHANGELOG.md for every versioned package, including private ones.
 * Private SDK changelogs are duplicates of the TypeScript SDK changelog — delete them so
 * they don't appear in the release PR diff or clutter the repository.
 */
function deletePrivateSdkChangelogs(): void {
	for (const pkg of readSdkPackages()) {
		if (pkg.packageJson.private !== true) continue;
		const changelog = join(pkg.dir, 'CHANGELOG.md');
		if (!existsSync(changelog)) continue;
		rmSync(changelog);
		console.log(`Deleted ${relative(ROOT, changelog)} (private SDK — changelog not needed).`);
	}
}

async function runRelease(): Promise<void> {
	await validatePendingChangesets();

	// Snapshot changeset files so they can be restored if expansion or versioning fails,
	// leaving the working tree clean for a retry.
	const changesets = await readChangesets(ROOT);
	const snapshot = new Map(
		changesets.map(({ id }) => {
			const file = join(ROOT, '.changeset', `${id}.md`);
			return [file, readFileSync(file, 'utf8')];
		}),
	);

	try {
		await expandChangesetsToAllSdks();
		execSync('pnpm changeset version', { stdio: 'inherit' });
	} catch (error) {
		for (const [file, content] of snapshot) {
			writeFileSync(file, content);
		}
		throw error;
	}

	syncNativeManifests();
	deletePrivateSdkChangelogs();
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
