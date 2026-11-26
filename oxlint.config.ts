import cloudflare from '@cloudflare/oxlint-config';
import { defineConfig } from 'oxlint';

export default defineConfig({
	...cloudflare,
	// Keep the correctness category already enforced in CI.
	categories: { ...cloudflare.categories, correctness: 'error' },
	ignorePatterns: [...(cloudflare.ignorePatterns ?? []), '**/dist/**', '**/build/**', '**/.wrangler/**'],
	rules: {
		...cloudflare.rules,
		eqeqeq: 'error',
		'no-console': 'off',
		'typescript/no-deprecated': 'warn',
		'typescript/no-explicit-any': 'warn',
		'no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_' }],
	},
	overrides: [
		...(cloudflare.overrides ?? []),
		{
			files: ['**/test/**', '**/*.test.ts', '**/*.spec.ts', '**/examples/**'],
			rules: { 'typescript/no-explicit-any': 'off' },
		},
		// Existing SDK boundaries, examples, and test fixtures with violations.
		{
			files: [
				'.github/changeset-version.ts',
				'.github/release-sdks.ts',
				'sdks/typescript/src/client-provider.ts',
				'sdks/typescript/src/client.ts',
				'sdks/typescript/src/local/evaluate.ts',
				'sdks/typescript/src/server-provider.ts',
				'sdks/typescript/tests/**/*.test.ts',
				'spec/local-evaluation/generate.ts',
			],
			rules: { 'typescript/no-unsafe-type-assertion': 'off' },
		},
		{ files: ['.github/release-sdks.ts'], rules: { 'quality/no-array-filter-map': 'off' } },
		{ files: ['sdks/typescript/examples/browser.ts'], rules: { 'typescript/switch-exhaustiveness-check': 'off' } },
		{
			files: [
				'sdks/typescript/examples/browser.ts',
				'sdks/typescript/examples/cloudflare-worker-binding.ts',
				'sdks/typescript/examples/cloudflare-worker.ts',
				'sdks/typescript/examples/server-local.ts',
				'sdks/typescript/examples/server.ts',
				'sdks/typescript/src/client.ts',
				'sdks/typescript/tests/**/*.test.ts',
				'spec/local-evaluation/generate.ts',
			],
			rules: { 'typescript/no-unsafe-call': 'off' },
		},
		{
			files: [
				'sdks/typescript/examples/browser.ts',
				'sdks/typescript/examples/cloudflare-worker-binding.ts',
				'sdks/typescript/examples/cloudflare-worker.ts',
				'sdks/typescript/examples/server-local.ts',
				'sdks/typescript/examples/server.ts',
				'spec/local-evaluation/generate.ts',
			],
			rules: { 'typescript/no-unsafe-argument': 'off' },
		},
		{
			files: [
				'sdks/typescript/examples/browser.ts',
				'sdks/typescript/examples/cloudflare-worker-binding.ts',
				'spec/local-evaluation/generate.ts',
			],
			rules: { 'typescript/no-unsafe-assignment': 'off' },
		},
		{
			files: ['sdks/typescript/examples/browser.ts', 'sdks/typescript/src/client.ts', 'sdks/typescript/src/server-provider.ts'],
			rules: { 'typescript/no-non-null-assertion': 'off' },
		},
		{
			files: ['sdks/typescript/examples/browser.ts', 'spec/local-evaluation/generate.ts'],
			rules: { 'typescript/no-unsafe-member-access': 'off' },
		},
		{
			files: ['sdks/typescript/examples/server-local.ts', 'sdks/typescript/examples/server.ts'],
			rules: { 'unicorn/prefer-top-level-await': 'off' },
		},
		{
			files: ['sdks/typescript/src/client.ts', 'sdks/typescript/src/context.ts', 'sdks/typescript/tests/context.test.ts'],
			rules: { 'quality/no-known-value-widening': 'off' },
		},
		{
			files: [
				'sdks/typescript/src/client.ts',
				'sdks/typescript/src/hooks/telemetry-hook.ts',
				'sdks/typescript/src/local/evaluate.ts',
				'sdks/typescript/tests/client.test.ts',
				'sdks/typescript/tests/context.test.ts',
				'sdks/typescript/tests/local-evaluation.test.ts',
				'spec/local-evaluation/generate.ts',
			],
			rules: { 'quality/no-unsafe-dictionary-type': 'off' },
		},
		{
			files: ['sdks/typescript/src/client.ts', 'sdks/typescript/src/local/evaluate.ts', 'sdks/typescript/tests/flag-metadata.test.ts'],
			rules: { 'quality/no-object-parameters': 'off' },
		},
		{
			files: ['sdks/typescript/src/context.ts', 'sdks/typescript/src/server-provider.ts', 'sdks/typescript/tests/client.test.ts'],
			rules: { '@nkzw/no-instanceof': 'off' },
		},
		{ files: ['sdks/typescript/src/local/evaluate.ts'], rules: { 'quality/no-unknown-returns': 'off' } },
		{
			// Object.hasOwn requires ES2022. The SDK compiles against ES2021.
			files: ['sdks/typescript/src/local/evaluate.ts', 'sdks/typescript/src/server-provider.ts', 'sdks/typescript/tests/client.test.ts'],
			rules: { 'prefer-object-has-own': 'off' },
		},
		{
			files: ['sdks/typescript/src/local/evaluate.ts', 'sdks/typescript/tests/local-evaluation.test.ts'],
			rules: { 'typescript/no-base-to-string': 'off' },
		},
		{
			files: ['sdks/typescript/src/server-provider.ts', 'sdks/typescript/tests/**/*.test.ts'],
			rules: { 'quality/no-chained-type-assertions': 'off' },
		},
		{
			files: ['sdks/typescript/tests/client.test.ts', 'sdks/typescript/tests/server-provider.test.ts'],
			rules: { 'typescript/no-unsafe-return': 'off' },
		},
		{
			files: ['sdks/typescript/tests/**/*.test.ts'],
			rules: { 'unicorn/consistent-function-scoping': 'off', 'vitest/require-mock-type-parameters': 'off' },
		},
		{
			files: [
				'sdks/typescript/tests/binding-provider.test.ts',
				'sdks/typescript/tests/local-evaluation.test.ts',
				'sdks/typescript/tests/types.test.ts',
			],
			rules: { 'vitest/no-conditional-expect': 'off' },
		},
		{
			files: ['sdks/typescript/tests/client-provider.test.ts', 'sdks/typescript/tests/context.test.ts'],
			rules: { 'vitest/require-to-throw-message': 'off' },
		},
		{ files: ['sdks/typescript/tests/client.test.ts'], rules: { 'typescript/use-unknown-in-catch-callback-variable': 'off' } },
		{ files: ['sdks/typescript/tests/local-evaluation.test.ts'], rules: { 'vitest/valid-title': 'off' } },
		{ files: ['spec/local-evaluation/generate.ts'], rules: { 'unicorn/prefer-single-call': 'off' } },
	],
});
