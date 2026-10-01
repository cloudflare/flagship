/**
 * Generate shared local-evaluation conformance vectors.
 *
 * Imports the data-plane `evaluateFlag` by path from a sibling flagship checkout
 * and writes cases.json. Dev-only — not part of any package build.
 *
 * Usage (from repo root):
 *   FLAGSHIP_REPO=../flagship npx tsx spec/local-evaluation/generate.ts
 */
import { writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const flagshipRepo = process.env.FLAGSHIP_REPO ?? resolve(__dirname, '../../../flagship');
const evaluatePath = resolve(flagshipRepo, 'packages/data-plane/src/evaluate.ts');

const { evaluateFlag, FlagConfigError } = await import(pathToFileURL(evaluatePath).href);

// murmurhash3 is not exported from the data-plane module; keep a byte-identical
// local copy so raw hash vectors stay pinned to the same algorithm.
const encoder = new TextEncoder();
const MAX_RETAINED_HASH_BYTES = 32 * 1024;
let hashBuf = new Uint8Array(512);

function murmurhash3(str: string, seed: number): number {
	const requiredBytes = str.length * 3;
	let b: Uint8Array;
	let n: number;
	if (requiredBytes > MAX_RETAINED_HASH_BYTES) {
		b = encoder.encode(str);
		n = b.byteLength;
	} else {
		if (hashBuf.byteLength < requiredBytes) hashBuf = new Uint8Array(requiredBytes);
		b = hashBuf;
		n = encoder.encodeInto(str, b).written;
	}
	let h = seed >>> 0;
	let i = 0;
	while (i + 4 <= n) {
		let k = b[i]! | (b[i + 1]! << 8) | (b[i + 2]! << 16) | (b[i + 3]! << 24);
		k = Math.imul(k, 0xcc9e2d51) >>> 0;
		k = ((k << 15) | (k >>> 17)) >>> 0;
		k = Math.imul(k, 0x1b873593) >>> 0;
		h ^= k;
		h = ((h << 13) | (h >>> 19)) >>> 0;
		h = (Math.imul(h, 5) + 0xe6546b64) >>> 0;
		i += 4;
	}
	let k = 0;
	if (n - i >= 3) k ^= b[i + 2]! << 16;
	if (n - i >= 2) k ^= b[i + 1]! << 8;
	if (n > i) {
		k ^= b[i]!;
		k = Math.imul(k, 0xcc9e2d51) >>> 0;
		k = ((k << 15) | (k >>> 17)) >>> 0;
		k = Math.imul(k, 0x1b873593) >>> 0;
		h ^= k;
	}
	h ^= n;
	h ^= h >>> 16;
	h = Math.imul(h, 0x85ebca6b) >>> 0;
	h ^= h >>> 13;
	h = Math.imul(h, 0xc2b2ae35) >>> 0;
	h ^= h >>> 16;
	return h >>> 0;
}

type FlagValue = boolean | string | number | Record<string, unknown> | unknown[];
type EvalFlag = {
	key: string;
	enabled: boolean;
	default_variation: string;
	variations: Record<string, unknown>;
	rules: Array<{
		conditions: unknown[];
		serve_variation: string;
		rollout?: { percentage: number; attribute?: string };
	}>;
};

type CaseExpected = { value: FlagValue; variant: string; reason: string } | { error: string } | { reasonOneOf: string[] };

interface ConformanceCase {
	name: string;
	definitions: { flags: Record<string, EvalFlag> };
	accountId: string;
	flagKey: string;
	context: Record<string, unknown>;
	expected: CaseExpected;
}

const ACCOUNT = 'aaaabbbbccccdddd1111222233334444';

function flag(
	key: string,
	partial: Partial<EvalFlag> & Pick<EvalFlag, 'variations'> & { rules?: EvalFlag['rules']; enabled?: boolean; default_variation?: string },
): EvalFlag {
	return {
		key,
		enabled: partial.enabled ?? true,
		default_variation: partial.default_variation ?? 'off',
		variations: partial.variations,
		rules: partial.rules ?? [],
	};
}

function base(attr: string, operator: string, value: unknown) {
	return { attribute: attr, operator, value };
}

function and(...clauses: unknown[]) {
	return { logical_operator: 'AND' as const, clauses };
}

function or(...clauses: unknown[]) {
	return { logical_operator: 'OR' as const, clauses };
}

function caseOf(name: string, flagDef: EvalFlag, context: Record<string, unknown>, accountId = ACCOUNT): ConformanceCase {
	const definitions = { flags: { [flagDef.key]: flagDef } };
	try {
		const result = evaluateFlag(flagDef, context, accountId);
		return {
			name,
			definitions,
			accountId,
			flagKey: flagDef.key,
			context,
			expected: { value: result.value, variant: result.variant, reason: result.reason },
		};
	} catch (error) {
		if (error instanceof FlagConfigError || (error as Error)?.name === 'FlagConfigError') {
			return {
				name,
				definitions,
				accountId,
				flagKey: flagDef.key,
				context,
				expected: { error: 'PARSE_ERROR' },
			};
		}
		throw error;
	}
}

const cases: ConformanceCase[] = [];

// --- STATIC / DISABLED / DEFAULT ---
cases.push(
	caseOf('static-no-rules', flag('static', { variations: { off: false, on: true }, rules: [] }), { targetingKey: 'u1' }),
	caseOf(
		'disabled-flag',
		flag('disabled', {
			enabled: false,
			variations: { off: false, on: true },
			rules: [{ conditions: [base('x', 'equals', '1')], serve_variation: 'on' }],
		}),
		{ x: '1' },
	),
	caseOf(
		'default-no-match',
		flag('default', {
			variations: { off: 'control', on: 'treatment' },
			default_variation: 'off',
			rules: [{ conditions: [base('plan', 'equals', 'pro')], serve_variation: 'on' }],
		}),
		{ plan: 'free' },
	),
);

// --- string operators ---
const stringOps = flag('string_ops', {
	variations: { none: 'none', hit: 'hit' },
	default_variation: 'none',
	rules: [],
});

for (const [name, operator, attr, value, context, shouldMatch] of [
	['equals-match', 'equals', 'plan', 'pro', { plan: 'pro' }, true],
	['equals-mismatch', 'equals', 'plan', 'pro', { plan: 'free' }, false],
	['equals-bool-string', 'equals', 'flag', true, { flag: 'true' }, true],
	['equals-number-string', 'equals', 'n', 1, { n: '1' }, true],
	['equals-1e21', 'equals', 'n', 1e21, { n: '1e+21' }, true],
	['equals-neg-zero', 'equals', 'n', -0, { n: '0' }, true],
	['not-equals-match', 'not_equals', 'plan', 'pro', { plan: 'free' }, true],
	['not-equals-null', 'not_equals', 'plan', 'pro', { plan: null }, true],
	['not-equals-equal', 'not_equals', 'plan', 'pro', { plan: 'pro' }, false],
	['contains-match', 'contains', 'email', '@corp', { email: 'a@corp.com' }, true],
	['contains-miss', 'contains', 'email', '@corp', { email: 'a@gmail.com' }, false],
	['contains-null', 'contains', 'email', '@corp', { email: null }, false],
	['starts-with-match', 'starts_with', 'id', 'beta_', { id: 'beta_1' }, true],
	['starts-with-miss', 'starts_with', 'id', 'beta_', { id: 'prod_1' }, false],
	['ends-with-match', 'ends_with', 'email', '.edu', { email: 'a@mit.edu' }, true],
	['ends-with-miss', 'ends_with', 'email', '.edu', { email: 'a@ex.com' }, false],
] as const) {
	const f = flag('string_ops', {
		...stringOps,
		rules: [{ conditions: [base(attr, operator, value)], serve_variation: 'hit' }],
	});
	const c = caseOf(`string-${name}`, f, context as Record<string, unknown>);
	if (shouldMatch) {
		if (!('value' in c.expected) || c.expected.reason !== 'TARGETING_MATCH') {
			throw new Error(`expected match for ${name}`);
		}
	}
	cases.push(c);
}

// --- ordering operators ---
for (const [name, operator, attrValue, target, shouldMatch] of [
	['gt-num', 'greater_than', 15, 10, true],
	['gt-num-eq', 'greater_than', 10, 10, false],
	['lt-num', 'less_than', 5, 10, true],
	['lt-num-eq', 'less_than', 10, 10, false],
	['gte-num', 'greater_than_or_equals', 10, 10, true],
	['lte-num', 'less_than_or_equals', 10, 10, true],
	['gt-string-num', 'greater_than', '15', 10, true],
	['gt-hex', 'greater_than', '0x10', 15, true],
	['gt-whitespace-empty', 'greater_than', '   ', 0, false],
	['gt-bool-true', 'greater_than', true, 0, true],
	// Infinity is a JS Number() edge case but cannot round-trip through JSON vectors.
	['iso-lt', 'less_than', '2025-01-01T00:00:00Z', '2030-01-01T00:00:00Z', true],
	['iso-gt', 'greater_than', '2031-01-01T00:00:00Z', '2030-01-01T00:00:00Z', true],
	['iso-eq', 'greater_than_or_equals', '2030-01-01T00:00:00Z', '2030-01-01T00:00:00Z', true],
	['iso-offset', 'less_than', '2025-01-01T00:00:00+00:00', '2030-01-01T00:00:00Z', true],
	['iso-ms', 'less_than', '2025-01-01T00:00:00.123Z', '2030-01-01T00:00:00Z', true],
] as const) {
	const f = flag('order_ops', {
		variations: { none: 'none', hit: 'hit' },
		default_variation: 'none',
		rules: [{ conditions: [base('v', operator, target)], serve_variation: 'hit' }],
	});
	const c = caseOf(`order-${name}`, f, { v: attrValue });
	if (shouldMatch !== ('value' in c.expected && c.expected.reason === 'TARGETING_MATCH')) {
		// still include; generator truth comes from evaluateFlag
	}
	cases.push(c);
}

// non-scalar ordering attr
cases.push(
	caseOf(
		'order-array-attr',
		flag('order_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('v', 'greater_than', 1)], serve_variation: 'hit' }],
		}),
		{ v: [5] },
	),
	caseOf(
		'order-array-target',
		flag('order_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('v', 'greater_than', [1])], serve_variation: 'hit' }],
		}),
		{ v: 5 },
	),
);

// --- in / not_in / has / not_has ---
cases.push(
	caseOf(
		'in-match',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'in', ['US', 'CA', 1, true])], serve_variation: 'hit' }],
		}),
		{ country: 'US' },
	),
	caseOf(
		'in-mixed-number',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'in', ['US', 'CA', 1, true])], serve_variation: 'hit' }],
		}),
		{ country: 1 },
	),
	caseOf(
		'in-mixed-bool',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'in', ['US', 'CA', 1, true])], serve_variation: 'hit' }],
		}),
		{ country: true },
	),
	caseOf(
		'in-miss',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'in', ['US', 'CA'])], serve_variation: 'hit' }],
		}),
		{ country: 'GB' },
	),
	caseOf(
		'in-null',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'in', ['US'])], serve_variation: 'hit' }],
		}),
		{ country: null },
	),
	caseOf(
		'not-in-null',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'not_in', ['US'])], serve_variation: 'hit' }],
		}),
		{ country: null },
	),
	caseOf(
		'not-in-match',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'not_in', ['US', 'CA'])], serve_variation: 'hit' }],
		}),
		{ country: 'GB' },
	),
	caseOf(
		'in-non-array-target',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('country', 'in', 'US')], serve_variation: 'hit' }],
		}),
		{ country: 'US' },
	),
	caseOf(
		'has-match',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('tags', 'has', 'beta')], serve_variation: 'hit' }],
		}),
		{ tags: ['alpha', 'beta', 3] },
	),
	caseOf(
		'has-miss',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('tags', 'has', 'gamma')], serve_variation: 'hit' }],
		}),
		{ tags: ['alpha', 'beta'] },
	),
	caseOf(
		'not-has-match',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('tags', 'not_has', 'gamma')], serve_variation: 'hit' }],
		}),
		{ tags: ['alpha'] },
	),
	caseOf(
		'has-non-array-attr',
		flag('set_ops', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('tags', 'has', 'beta')], serve_variation: 'hit' }],
		}),
		{ tags: 'beta' },
	),
);

// --- missing / dotted paths ---
cases.push(
	caseOf(
		'missing-attr',
		flag('path', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('missing', 'equals', 'x')], serve_variation: 'hit' }],
		}),
		{},
	),
	caseOf(
		'dotted-path-match',
		flag('path', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('user.profile.plan', 'equals', 'pro')], serve_variation: 'hit' }],
		}),
		{ user: { profile: { plan: 'pro' } } },
	),
	caseOf(
		'exact-key-wins-over-dotted',
		flag('path', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('user.plan', 'equals', 'exact')], serve_variation: 'hit' }],
		}),
		{ 'user.plan': 'exact', user: { plan: 'nested' } },
	),
	caseOf(
		'dotted-path-too-deep',
		flag('path', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('a.b.c.d.e.f', 'equals', 'x')], serve_variation: 'hit' }],
		}),
		{ a: { b: { c: { d: { e: { f: 'x' } } } } } },
	),
	caseOf(
		'dotted-path-depth-5',
		flag('path', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [base('a.b.c.d.e', 'equals', 'x')], serve_variation: 'hit' }],
		}),
		{ a: { b: { c: { d: { e: 'x' } } } } },
	),
);

// --- AND / OR nesting + rule order ---
cases.push(
	caseOf(
		'and-match',
		flag('logic', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [
				{
					conditions: [base('role', 'equals', 'admin'), base('region', 'equals', 'us')],
					serve_variation: 'hit',
				},
			],
		}),
		{ role: 'admin', region: 'us' },
	),
	caseOf(
		'and-partial',
		flag('logic', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [
				{
					conditions: [base('role', 'equals', 'admin'), base('region', 'equals', 'us')],
					serve_variation: 'hit',
				},
			],
		}),
		{ role: 'admin', region: 'eu' },
	),
	caseOf(
		'or-match',
		flag('logic', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [{ conditions: [or(base('a', 'equals', '1'), base('b', 'equals', '2'))], serve_variation: 'hit' }],
		}),
		{ b: '2' },
	),
	caseOf(
		'nested-and-or',
		flag('logic', {
			variations: { none: 'none', hit: 'hit' },
			default_variation: 'none',
			rules: [
				{
					conditions: [and(base('role', 'equals', 'admin'), or(base('region', 'equals', 'us'), base('region', 'equals', 'eu')))],
					serve_variation: 'hit',
				},
			],
		}),
		{ role: 'admin', region: 'eu' },
	),
	caseOf(
		'rule-order-first-wins',
		flag('order', {
			variations: { a: 'a', b: 'b', def: 'def' },
			default_variation: 'def',
			rules: [
				{ conditions: [base('tier', 'equals', 'vip')], serve_variation: 'a' },
				{ conditions: [base('tier', 'equals', 'vip')], serve_variation: 'b' },
			],
		}),
		{ tier: 'vip' },
	),
);

// --- SPLIT / percentage ---
const rolloutFlag = (key: string, percentage: number, attribute?: string): EvalFlag =>
	flag(key, {
		variations: { on: true, off: false },
		default_variation: 'off',
		rules: [{ conditions: [], serve_variation: 'on', rollout: attribute ? { percentage, attribute } : { percentage } }],
	});

// deterministic buckets using known account seed
for (const targetingKey of ['0', '1', '2', 'user-a', '日本語', 'héllo', '🚀', '']) {
	cases.push(caseOf(`split-50-key-${JSON.stringify(targetingKey)}`, rolloutFlag('rollout_test', 50, 'targetingKey'), { targetingKey }));
}

cases.push(
	caseOf('split-100-is-targeting-match', rolloutFlag('rollout_full', 100), { targetingKey: 'anyone' }),
	caseOf('split-0-never', rolloutFlag('rollout_none', 0), { targetingKey: 'anyone' }),
	caseOf(
		'split-custom-attr',
		flag('rollout_custom', {
			variations: { on: true, off: false },
			default_variation: 'off',
			rules: [{ conditions: [], serve_variation: 'on', rollout: { percentage: 50, attribute: 'userId' } }],
		}),
		{ userId: '0', targetingKey: 'ignored' },
	),
	caseOf(
		'split-non-scalar-attr',
		flag('rollout_obj', {
			variations: { on: true, off: false },
			default_variation: 'off',
			rules: [{ conditions: [], serve_variation: 'on', rollout: { percentage: 100, attribute: 'obj' } }],
		}),
		{ obj: { a: 1 } },
	),
	// random bucket — only assert reasonOneOf
	(() => {
		const flagDef = rolloutFlag('rollout_random', 50);
		return {
			name: 'split-missing-attr-random',
			definitions: { flags: { [flagDef.key]: flagDef } },
			accountId: ACCOUNT,
			flagKey: flagDef.key,
			context: {},
			expected: { reasonOneOf: ['SPLIT', 'DEFAULT'] },
		} satisfies ConformanceCase;
	})(),
	(() => {
		const flagDef = rolloutFlag('rollout_null', 50);
		return {
			name: 'split-null-attr-random',
			definitions: { flags: { [flagDef.key]: flagDef } },
			accountId: ACCOUNT,
			flagKey: flagDef.key,
			context: { targetingKey: null },
			expected: { reasonOneOf: ['SPLIT', 'DEFAULT'] },
		} satisfies ConformanceCase;
	})(),
	caseOf(
		'split-fractional-percentage',
		flag('rollout_frac', {
			variations: { on: true, off: false },
			default_variation: 'off',
			rules: [{ conditions: [], serve_variation: 'on', rollout: { percentage: 0.5, attribute: 'targetingKey' } }],
		}),
		{ targetingKey: '0' },
	),
	caseOf(
		'split-with-conditions',
		flag('rollout_cond', {
			variations: { on: true, off: false },
			default_variation: 'off',
			rules: [
				{
					conditions: [base('plan', 'equals', 'pro')],
					serve_variation: 'on',
					rollout: { percentage: 50, attribute: 'targetingKey' },
				},
			],
		}),
		{ plan: 'pro', targetingKey: '0' },
	),
);

// --- missing variation ---
cases.push(
	caseOf(
		'missing-serve-variation',
		flag('bad_serve', {
			variations: { off: false },
			default_variation: 'off',
			rules: [{ conditions: [], serve_variation: 'missing' }],
		}),
		{},
	),
	caseOf(
		'missing-default-variation',
		flag('bad_default', {
			variations: { on: true },
			default_variation: 'off',
			rules: [],
		}),
		{},
	),
);

// --- value types ---
cases.push(
	caseOf(
		'value-string',
		flag('types', {
			variations: { a: 'hello', b: 'world' },
			default_variation: 'a',
			rules: [{ conditions: [base('x', 'equals', '1')], serve_variation: 'b' }],
		}),
		{ x: '1' },
	),
	caseOf(
		'value-number',
		flag('types', {
			variations: { a: 1, b: 2.5 },
			default_variation: 'a',
			rules: [{ conditions: [base('x', 'equals', '1')], serve_variation: 'b' }],
		}),
		{ x: '1' },
	),
	caseOf(
		'value-object',
		flag('types', {
			variations: { a: { k: 1 }, b: { k: 2, nested: true } },
			default_variation: 'a',
			rules: [{ conditions: [base('x', 'equals', '1')], serve_variation: 'b' }],
		}),
		{ x: '1' },
	),
	caseOf(
		'value-array',
		flag('types', {
			variations: { a: [1], b: [1, 2, 'x'] },
			default_variation: 'a',
			rules: [{ conditions: [base('x', 'equals', '1')], serve_variation: 'b' }],
		}),
		{ x: '1' },
	),
);

// --- raw murmur vectors ---
const murmurVectors: Array<{ name: string; input: string; seed: number }> = [
	{ name: 'murmur-empty', input: '', seed: 0 },
	{ name: 'murmur-hello', input: 'hello', seed: 0 },
	{ name: 'murmur-hello-seed', input: 'hello', seed: 42 },
	{ name: 'murmur-emoji', input: '🚀🎉', seed: 0 },
	{ name: 'murmur-jp', input: '日本語', seed: 45 },
	{ name: 'murmur-account-flag', input: `${ACCOUNT}:rollout_test`, seed: 0 },
	{ name: 'murmur-long', input: 'a'.repeat(1000), seed: 7 },
];

const murmurCases = murmurVectors.map((v) => ({
	name: v.name,
	kind: 'murmur' as const,
	input: v.input,
	seed: v.seed,
	expected: murmurhash3(v.input, v.seed) >>> 0,
}));

const out = {
	version: 1,
	generatedBy: 'spec/local-evaluation/generate.ts',
	accountId: ACCOUNT,
	cases,
	murmur: murmurCases,
};

const outPath = resolve(__dirname, 'cases.json');
writeFileSync(outPath, `${JSON.stringify(out, null, '\t')}\n`);
console.log(`Wrote ${cases.length} evaluation cases + ${murmurCases.length} murmur vectors → ${outPath}`);
