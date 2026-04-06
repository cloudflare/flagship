import { execSync } from 'node:child_process';

// This script is used by the release workflow to update package versions.
// The standard step is only to run `changeset version` but this does not
// update the lockfile. So we also run `pnpm install` to keep it in sync.
// See https://github.com/changesets/changesets/issues/421.
execSync('pnpm changeset version', {
	stdio: 'inherit',
});
execSync('pnpm install --no-frozen-lockfile', {
	stdio: 'inherit',
});
