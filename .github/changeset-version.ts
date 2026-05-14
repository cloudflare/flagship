/**
 * Release automation for the Flagship multi-language SDK monorepo.
 *
 * Two modes:
 *   - `validate`: Run in PR CI. Fails on malformed, non-SDK, or `none`-bumped SDK changesets.
 *   - `release`:  Run by changesets/action. Validates, expands every changeset to all SDKs,
 *                 calls `changeset version`, syncs native manifests, and refreshes the lockfile.
 *
 * An "SDK" is any direct subdirectory of `packages/` containing a `package.json`.
 */

import { execSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import readChangesets from '@changesets/read';
import type { NewChangeset, Release, VersionType } from '@changesets/types';
import { getPackagesSync, type Package } from '@manypkg/get-packages';

const ROOT = process.cwd();
const BUMP_RANK: Record<VersionType, number> = { none: 0, patch: 1, minor: 2, major: 3 };

// ---------- Workspace discovery ----------

function readSdkPackages(): Package[] {
	const { packages } = getPackagesSync(ROOT);
	return packages.filter((pkg) => pkg.relativeDir.startsWith('packages/') && !pkg.relativeDir.slice('packages/'.length).includes('/'));
}

// ---------- Validation ----------

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

// ---------- Expansion ----------

/**
 * Ensures every changeset that touches any SDK bumps *all* SDKs to the same level.
 * All SDKs share a version line, so any release moves them together.
 */
export async function expandChangesetsToAllSdks(): Promise<void> {
	const allSdkNames = readSdkPackages()
		.map((pkg) => pkg.packageJson.name)
		.sort();

	for (const changeset of await readChangesets(ROOT)) {
		const present = new Set(changeset.releases.map((release) => release.name));
		const missing = allSdkNames.filter((name) => !present.has(name));
		if (missing.length === 0) continue;

		const bump = highestBump(changeset.releases.map((release) => release.type));
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

// ---------- Native manifest sync ----------

/**
 * After `changeset version` updates each SDK's `package.json`, mirror the new version
 * into the native manifest beside it. Go modules are tag-only — no file sync.
 */
export function syncNativeManifests(): void {
	const errors: string[] = [];

	for (const pkg of readSdkPackages()) {
		const targetVersion = pkg.packageJson.version;
		syncTomlManifest(join(pkg.dir, 'pyproject.toml'), targetVersion, errors);
		syncTomlManifest(join(pkg.dir, 'Cargo.toml'), targetVersion, errors);
		noteGoModule(join(pkg.dir, 'go.mod'), pkg.packageJson.name, targetVersion);
	}

	if (errors.length > 0) {
		throw new Error(`Native manifest sync failed:\n  - ${errors.join('\n  - ')}`);
	}
}

function syncTomlManifest(manifestPath: string, targetVersion: string, errors: string[]): void {
	if (!existsSync(manifestPath)) return;

	const original = readFileSync(manifestPath, 'utf8');
	const versionLine = /^(\s*version\s*=\s*")([^"]+)(")/gm;
	const firstMatch = versionLine.exec(original);
	const relativePath = relative(ROOT, manifestPath);

	if (firstMatch === null) {
		errors.push(`${relativePath}: no top-level 'version = "..."' line found. Add one so release automation can sync it.`);
		return;
	}

	const previousVersion = firstMatch[2];
	if (previousVersion === targetVersion) {
		console.log(`${relativePath} already at ${targetVersion}.`);
		return;
	}

	writeFileSync(manifestPath, original.replace(versionLine, `$1${targetVersion}$3`));
	console.log(`Synced ${relativePath} ${previousVersion} -> ${targetVersion}.`);
}

function noteGoModule(manifestPath: string, packageName: string, version: string): void {
	if (!existsSync(manifestPath)) return;
	console.log(
		`${relative(ROOT, manifestPath)}: Go modules are versioned via git tags only; ${packageName}@${version} requires no file sync.`,
	);
}

// ---------- Entry points ----------

async function runRelease(): Promise<void> {
	await validatePendingChangesets();
	await expandChangesetsToAllSdks();
	execSync('pnpm changeset version', { stdio: 'inherit' });
	syncNativeManifests();
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
