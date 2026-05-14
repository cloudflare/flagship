import { execSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';

type PackageJson = {
	name?: string;
	version?: string;
	private?: boolean;
	flagship?: {
		language?: string;
	};
};

type WorkspacePackage = {
	name: string;
	directory: string;
	version: string;
	isSdk: boolean;
};

type PnpmListPackage = {
	name?: string;
	version?: string;
	path?: string;
};

type ChangesetEntry = {
	name: string;
	bump: 'major' | 'minor' | 'patch' | 'none';
};

const root = process.cwd();
const changesetDirectory = join(root, '.changeset');
const bumpOrder = new Map<ChangesetEntry['bump'], number>([
	['none', 0],
	['patch', 1],
	['minor', 2],
	['major', 3],
]);
const versionSyncedManifestFiles = ['Cargo.toml', 'pyproject.toml'] as const;
const tagOnlyManifestFiles = ['go.mod'] as const;

function readJson(path: string): PackageJson {
	return JSON.parse(readFileSync(path, 'utf8')) as PackageJson;
}

function readWorkspacePackages(): WorkspacePackage[] {
	const output = execSync('pnpm list --recursive --depth -1 --json', { encoding: 'utf8' });
	let packages: PnpmListPackage[];

	try {
		packages = JSON.parse(output) as PnpmListPackage[];
	} catch (cause) {
		throw new Error('Could not parse pnpm workspace package list. Run `pnpm list --recursive --depth -1 --json` to inspect the output.', {
			cause,
		});
	}

	return packages.flatMap((packageInfo) => {
		if (
			packageInfo.path === undefined ||
			packageInfo.path === root ||
			packageInfo.name === undefined ||
			packageInfo.version === undefined
		) {
			return [];
		}

		const packageJson = readJson(join(packageInfo.path, 'package.json'));

		return [
			{
				name: packageInfo.name,
				directory: packageInfo.path,
				version: packageInfo.version,
				isSdk: packageJson.flagship?.language !== undefined,
			},
		];
	});
}

function readSdkPackages(): WorkspacePackage[] {
	return readWorkspacePackages().filter((packageInfo) => packageInfo.isSdk);
}

function parseChangesetEntries(frontmatter: string): ChangesetEntry[] {
	return frontmatter
		.split(/\r?\n/)
		.map((line) => line.replace(/\s+#.*$/, ''))
		.map((line) => /^['"]?([^'":]+)['"]?\s*:\s*(major|minor|patch|none)\s*$/.exec(line.trim()))
		.filter((match): match is RegExpExecArray => match !== null)
		.map((match) => ({ name: match[1], bump: match[2] as ChangesetEntry['bump'] }));
}

function highestBump(entries: ChangesetEntry[]): ChangesetEntry['bump'] {
	return entries.reduce<ChangesetEntry['bump']>((highest, entry) => {
		return (bumpOrder.get(entry.bump) ?? 0) > (bumpOrder.get(highest) ?? 0) ? entry.bump : highest;
	}, 'none');
}

function readChangesetFiles(): string[] {
	return readdirSync(changesetDirectory)
		.filter((file) => file.endsWith('.md') && file !== 'README.md')
		.map((file) => join(changesetDirectory, file));
}

function parseChangeset(file: string): { entries: ChangesetEntry[]; frontmatter: string; body: string } | undefined {
	const content = readFileSync(file, 'utf8');
	const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(content);

	if (match === null) {
		return undefined;
	}

	return { entries: parseChangesetEntries(match[1]), frontmatter: match[1], body: match[2] };
}

function replaceFirstVersionLine(content: string, targetVersion: string): { content: string; previousVersion: string } | undefined {
	const versionLinePattern = /^(\s*version\s*=\s*")([^"]+)(")/m;
	const match = versionLinePattern.exec(content);

	if (match === null) {
		return undefined;
	}

	return {
		content: `${content.slice(0, match.index)}${match[1]}${targetVersion}${match[3]}${content.slice(match.index + match[0].length)}`,
		previousVersion: match[2],
	};
}

export function validatePendingChangesets(): void {
	const workspacePackages = readWorkspacePackages();
	const workspacePackagesByName = new Map(workspacePackages.map((packageInfo) => [packageInfo.name, packageInfo]));
	const sdkPackageNameSet = new Set(workspacePackages.filter((packageInfo) => packageInfo.isSdk).map((packageInfo) => packageInfo.name));

	const errors: string[] = [];

	for (const file of readChangesetFiles()) {
		const parsed = parseChangeset(file);

		if (parsed === undefined) {
			errors.push(`Changeset ${relative(root, file)} has no valid frontmatter.`);
			continue;
		}

		if (parsed.entries.length === 0) {
			errors.push(`Changeset ${relative(root, file)} declares no package bumps.`);
			continue;
		}

		const unknownPackageNames = parsed.entries
			.map((entry) => entry.name)
			.filter((packageName) => !workspacePackagesByName.has(packageName));

		if (unknownPackageNames.length > 0) {
			errors.push(`Changeset ${relative(root, file)} references unknown package(s): ${unknownPackageNames.join(', ')}.`);
		}

		const sdkEntries = parsed.entries.filter((entry) => sdkPackageNameSet.has(entry.name));
		const noneSDKNames = sdkEntries.filter((entry) => entry.bump === 'none').map((entry) => entry.name);

		if (noneSDKNames.length > 0) {
			errors.push(
				`Changeset ${relative(root, file)} sets bump 'none' for SDK package(s): ${noneSDKNames.join(', ')}. Use 'patch', 'minor', or 'major' for SDK entries.`,
			);
		}
	}

	if (errors.length > 0) {
		throw new Error(`Changeset validation failed:\n  - ${errors.join('\n  - ')}`);
	}

	const count = readChangesetFiles().length;
	console.log(count === 0 ? 'No pending changesets to validate.' : `Validated ${count} pending changeset(s).`);
}

export function expandChangesetsToAllSdks(): void {
	const sdkPackages = readSdkPackages();
	const sdkPackageNames = sdkPackages.map((packageInfo) => packageInfo.name).sort();
	const sdkPackageNameSet = new Set(sdkPackageNames);

	for (const file of readChangesetFiles()) {
		const parsed = parseChangeset(file);

		if (parsed === undefined) {
			continue;
		}

		const touchesSdk = parsed.entries.some((entry) => sdkPackageNameSet.has(entry.name));

		if (!touchesSdk) {
			continue;
		}

		const existingPackageNames = new Set(parsed.entries.map((entry) => entry.name));
		const missingSdkPackageNames = sdkPackageNames.filter((packageName) => !existingPackageNames.has(packageName));

		if (missingSdkPackageNames.length === 0) {
			continue;
		}

		const bump = highestBump(parsed.entries.filter((entry) => sdkPackageNameSet.has(entry.name)));
		const expandedFrontmatter = [
			parsed.frontmatter.trimEnd(),
			...missingSdkPackageNames.map((packageName) => `"${packageName}": ${bump}`),
		].join('\n');
		writeFileSync(file, `---\n${expandedFrontmatter}\n---\n${parsed.body}`);
		console.log(`Expanded ${relative(root, file)} to version all SDK packages with bump '${bump}'.`);
	}
}

export function syncNativeManifests(): void {
	const errors: string[] = [];

	for (const packageInfo of readSdkPackages()) {
		const packageJson = readJson(join(packageInfo.directory, 'package.json'));
		const targetVersion = packageJson.version;

		if (targetVersion === undefined) {
			errors.push(`Package ${packageInfo.name} has no version in package.json.`);
			continue;
		}

		for (const manifest of versionSyncedManifestFiles) {
			const manifestPath = join(packageInfo.directory, manifest);

			if (!existsSync(manifestPath)) {
				continue;
			}

			const original = readFileSync(manifestPath, 'utf8');
			const updated = replaceFirstVersionLine(original, targetVersion);

			if (updated === undefined) {
				errors.push(
					`Could not find a top-level 'version = "..."' line in ${relative(root, manifestPath)}. Add one so release automation can sync it.`,
				);
				continue;
			}

			if (updated.previousVersion === targetVersion) {
				console.log(`${relative(root, manifestPath)} already at ${targetVersion}.`);
				continue;
			}

			writeFileSync(manifestPath, updated.content);
			console.log(`Synced ${relative(root, manifestPath)} ${updated.previousVersion} -> ${targetVersion}.`);
		}

		for (const manifest of tagOnlyManifestFiles) {
			const manifestPath = join(packageInfo.directory, manifest);

			if (!existsSync(manifestPath)) {
				continue;
			}

			console.log(
				`${relative(root, manifestPath)} detected — Go modules are versioned via git tags only; no file sync needed for ${packageInfo.name}@${targetVersion}.`,
			);
		}
	}

	if (errors.length > 0) {
		throw new Error(`Native manifest sync failed:\n  - ${errors.join('\n  - ')}`);
	}
}

function runRelease(): void {
	validatePendingChangesets();
	const changesetSnapshot = new Map(readChangesetFiles().map((file) => [file, readFileSync(file, 'utf8')]));

	try {
		expandChangesetsToAllSdks();
		execSync('pnpm changeset version', {
			stdio: 'inherit',
		});
	} catch (error) {
		for (const [file, content] of changesetSnapshot) {
			writeFileSync(file, content);
		}

		throw error;
	}

	syncNativeManifests();
	execSync('pnpm install --no-frozen-lockfile', {
		stdio: 'inherit',
	});
}

function runValidate(): void {
	validatePendingChangesets();
}

const mode = process.argv[2] ?? 'release';

if (mode === 'validate') {
	runValidate();
} else if (mode === 'release') {
	runRelease();
} else {
	throw new Error(`Unknown mode: ${mode}. Expected 'release' or 'validate'.`);
}
