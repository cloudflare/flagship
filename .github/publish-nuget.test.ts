import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const workflow = readFileSync(new URL('./workflows/publish-nuget.yml', import.meta.url), 'utf8');

// Exercise the actual inline Bash used by Actions; workflow syntax is checked separately.
function stepScript(name: string): string {
	const step = workflow.split(`      - name: ${name}\n`)[1];
	assert.ok(step, `Missing workflow step: ${name}`);

	const lines = step.split('        run: |\n')[1]?.split('\n');
	assert.ok(lines, `Missing script: ${name}`);

	const script: string[] = [];

	for (const line of lines) {
		if (line.trim() && !line.startsWith('          ')) break;
		script.push(line.slice(10));
	}

	return script.join('\n');
}

for (const configured of [false, true]) {
	test(`NuGet credential guard: ${configured ? 'configured' : 'missing'}`, () => {
		const directory = mkdtempSync(join(tmpdir(), 'flagship-nuget-'));

		try {
			const output = join(directory, 'output');
			const summary = join(directory, 'summary');
			writeFileSync(summary, '');

			const result = spawnSync('bash', ['-e', '-o', 'pipefail', '-c', stepScript('Check optional API key')], {
				encoding: 'utf8',
				env: {
					...process.env,
					NUGET_API_KEY: configured ? 'test-only-placeholder' : '',
					GITHUB_OUTPUT: output,
					GITHUB_STEP_SUMMARY: summary,
				},
			});

			assert.equal(result.status, 0, result.stderr);
			assert.equal(readFileSync(output, 'utf8'), `configured=${configured}\n`);

			const message = readFileSync(summary, 'utf8');
			assert.equal(message.includes('NuGet publication skipped'), !configured);
			assert.ok(!`${result.stdout}${result.stderr}${message}`.includes('test-only-placeholder'));
			assert.ok(workflow.includes("if: ${{ needs.credentials.outputs.configured == 'true' }}"));
		} finally {
			rmSync(directory, { recursive: true, force: true });
		}
	});
}

for (const failAt of [0, 1, 2]) {
	test(`NuGet publication ${failAt ? `stops when package ${failAt} fails` : 'tags after both packages succeed'}`, () => {
		const directory = mkdtempSync(join(tmpdir(), 'flagship-nuget-'));

		try {
			const calls = join(directory, 'calls');
			const tags = join(directory, 'tags');
			writeFileSync(tags, '');

			const stub = `attempt=0
dotnet() {
  printf '%s\\n' "$*" >> "$CALLS"
  attempt=$((attempt + 1))
  test "$FAIL_AT" != "$attempt"
}
git() {
  if [ "$1" = rev-parse ]; then return 1; fi
  printf '%s\\n' "$*" >> "$TAGS"
}
`;
			const result = spawnSync(
				'bash',
				['-e', '-o', 'pipefail', '-c', stub + stepScript('Publish both packages to nuget.org') + '\n' + stepScript('Tag .NET SDK release')],
				{
					encoding: 'utf8',
					env: {
						...process.env,
						CALLS: calls,
						TAGS: tags,
						FAIL_AT: String(failAt),
						VERSION: '1.2.3',
						NUGET_API_KEY: 'test-only-placeholder',
					},
				},
			);

			assert.equal(result.status, failAt ? 1 : 0, result.stderr);

			const commands = readFileSync(calls, 'utf8').trim().split('\n');
			assert.equal(commands.length, failAt || 2);
			assert.ok(commands[0]?.includes('artifacts/Cloudflare.Flagship.1.2.3.nupkg'));
			if (failAt !== 1) assert.ok(commands[1]?.includes('artifacts/Cloudflare.Flagship.OpenFeature.1.2.3.nupkg'));
			assert.equal(readFileSync(tags, 'utf8'), failAt ? '' : 'tag sdks/dotnet/v1.2.3\npush origin sdks/dotnet/v1.2.3\n');

			for (const command of commands) {
				assert.ok(command.includes('--source https://api.nuget.org/v3/index.json'));
				assert.ok(command.includes('--skip-duplicate'));
			}
		} finally {
			rmSync(directory, { recursive: true, force: true });
		}
	});
}
