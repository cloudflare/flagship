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

type ChangesetEntry = {
	name: string;
	bump: 'major' | 'minor' | 'patch' | 'none';
};

const root = process.cwd();
const changesetDirectory = join(root, '.changeset');
const ignoredDirectories = new Set(['.git', '.husky', 'node_modules', 'dist', 'coverage', '.wrangler']);
const bumpOrder = new Map<ChangesetEntry['bump'], number>([
	['none', 0],
	['patch', 1],
	['minor', 2],
	['major', 3],
]);
const nativeManifestFiles = ['Cargo.toml', 'pyproject.toml'] as const;

function readJson(path: string): PackageJson {
	return JSON.parse(readFileSync(path, 'utf8')) as PackageJson;
}

function isSdkDirectory(directory: string): boolean {
	const parts = relative(root, directory).split('/');
	return parts[0] === 'packages' && parts.length === 2;
}

function readWorkspacePackages(directory = root): WorkspacePackage[] {
	const packageJsonPath = join(directory, 'package.json');
	const packages: WorkspacePackage[] = [];

	if (directory !== root && existsSync(packageJsonPath)) {
		const packageJson = readJson(packageJsonPath);

		if (packageJson.name !== undefined && packageJson.version !== undefined) {
			packages.push({
				name: packageJson.name,
				directory,
				version: packageJson.version,
				isSdk: isSdkDirectory(directory),
			});
		}
	}

	for (const entry of readdirSync(directory, { withFileTypes: true })) {
		if (!entry.isDirectory() || ignoredDirectories.has(entry.name)) {
			continue;
		}

		packages.push(...readWorkspacePackages(join(directory, entry.name)));
	}

	return packages;
}

function readSdkPackages(): WorkspacePackage[] {
	return readWorkspacePackages().filter((packageInfo) => packageInfo.isSdk);
}

function parseChangesetEntries(frontmatter: string): ChangesetEntry[] {
	return frontmatter
		.split('\n')
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
	const match = /^---\n([\s\S]*?)\n---\n?([\s\S]*)$/.exec(content);

	if (match === null) {
		return undefined;
	}

	return { entries: parseChangesetEntries(match[1]), frontmatter: match[1], body: match[2] };
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
		const touchesSdk = sdkEntries.length > 0;

		if (!touchesSdk) {
			errors.push(`Changeset ${relative(root, file)} does not bump any SDK package. Every release changeset must target at least one SDK.`);
		}

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

		for (const manifest of nativeManifestFiles) {
			const manifestPath = join(packageInfo.directory, manifest);

			if (!existsSync(manifestPath)) {
				continue;
			}

			const original = readFileSync(manifestPath, 'utf8');
			const versionLinePattern = /^(\s*version\s*=\s*")([^"]+)(")/m;
			const match = versionLinePattern.exec(original);

			if (match === null) {
				errors.push(
					`Could not find a top-level 'version = "..."' line in ${relative(root, manifestPath)}. Add one so release automation can sync it.`,
				);
				continue;
			}

			if (match[2] === targetVersion) {
				console.log(`${relative(root, manifestPath)} already at ${targetVersion}.`);
				continue;
			}

			const updated = original.replace(versionLinePattern, `$1${targetVersion}$3`);
			writeFileSync(manifestPath, updated);
			console.log(`Synced ${relative(root, manifestPath)} ${match[2]} -> ${targetVersion}.`);
		}
	}

	if (errors.length > 0) {
		throw new Error(`Native manifest sync failed:\n  - ${errors.join('\n  - ')}`);
	}
}

function runRelease(): void {
	validatePendingChangesets();
	expandChangesetsToAllSdks();
	execSync('pnpm changeset version', {
		stdio: 'inherit',
	});
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
