import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';
import { toFlagMetadata } from '../src/metadata.js';
import type { FlagshipBinding } from '../src/types.js';
import { evaluateFlag, type EvalFlag } from '../src/local/evaluate.js';

const metadata = { owner: 'team-payments', 'jira.id': 'PROJ-123', 'experiment.active': false, weight: 2 };
const logger = { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} };

describe('toFlagMetadata', () => {
	it('returns an empty frozen record for absent or non-object input', () => {
		for (const input of [undefined, null, 'x', 1, true, ['a']]) {
			const result = toFlagMetadata(input);
			expect(result).toEqual({});
			expect(Object.isFrozen(result)).toBe(true);
		}
	});

	it('copies primitive entries and freezes the result', () => {
		const result = toFlagMetadata(metadata);
		expect(result).toEqual(metadata);
		expect(result).not.toBe(metadata);
		expect(Object.isFrozen(result)).toBe(true);
	});

	it('drops non-primitive and non-finite entries', () => {
		expect(toFlagMetadata({ ok: 'yes', nested: { a: 1 }, list: [1], nil: null, nan: NaN, inf: Infinity })).toEqual({ ok: 'yes' });
	});

	it('keeps __proto__ as an own key without polluting the prototype', () => {
		const result = toFlagMetadata(JSON.parse('{"__proto__":"x","a":1}'));
		expect(Object.getPrototypeOf(result)).toBe(Object.prototype);
		expect(Object.keys(result).sort()).toEqual(['__proto__', 'a']);
	});
});

describe('local evaluation metadata', () => {
	const flag: EvalFlag = { key: 'f', enabled: true, default_variation: 'on', variations: { on: true }, rules: [], metadata };

	it('includes flag metadata in the result', () => {
		expect(evaluateFlag(flag, {}, 'acct').metadata).toEqual(metadata);
	});

	it('includes metadata for disabled flags', () => {
		expect(evaluateFlag({ ...flag, enabled: false }, {}, 'acct')).toMatchObject({ reason: 'DISABLED', metadata });
	});
});

describe('FlagshipServerProvider flag metadata', () => {
	beforeEach(() => {
		OpenFeature.clearHandlers();
		OpenFeature.clearProviders();
	});

	describe('HTTP mode', () => {
		const respond = (body: unknown) => vi.fn(async () => Response.json(body));
		const make = (fetch: typeof globalThis.fetch, extra: object = {}) =>
			new FlagshipServerProvider({ appId: 'app-1', accountId: 'acct-1', fetch, retries: 0, ...extra });

		it('maps response metadata to flagMetadata', async () => {
			const provider = make(respond({ flagKey: 'f', value: true, variant: 'on', reason: 'STATIC', metadata }));
			const result = await provider.resolveBooleanEvaluation('f', false, {}, logger);
			expect(result.flagMetadata).toEqual(metadata);
			expect(Object.isFrozen(result.flagMetadata)).toBe(true);
		});

		it('maps metadata for disabled flags', async () => {
			const provider = make(respond({ flagKey: 'f', reason: 'DISABLED', value: false, metadata }));
			const result = await provider.resolveBooleanEvaluation('f', true, {}, logger);
			expect(result).toMatchObject({ value: true, reason: 'DISABLED', flagMetadata: metadata });
		});

		it('ignores malformed metadata', async () => {
			const provider = make(respond({ flagKey: 'f', value: true, reason: 'STATIC', metadata: 'oops' }));
			expect((await provider.resolveBooleanEvaluation('f', false, {}, logger)).flagMetadata).toEqual({});
		});

		it('preserves metadata on cache hits', async () => {
			const fetch = respond({ flagKey: 'f', value: true, variant: 'on', reason: 'STATIC', metadata });
			const provider = make(fetch, { cacheTtl: 60_000 });
			await provider.resolveBooleanEvaluation('f', false, {}, logger);
			const hit = await provider.resolveBooleanEvaluation('f', false, {}, logger);
			expect(fetch).toHaveBeenCalledTimes(1);
			expect(hit).toMatchObject({ reason: 'CACHED', flagMetadata: metadata });
		});

		it('exposes metadata through the OpenFeature client', async () => {
			await OpenFeature.setProviderAndWait(make(respond({ flagKey: 'f', value: 'x', variant: 'v', reason: 'STATIC', metadata })));
			const details = await OpenFeature.getClient().getStringDetails('f', 'd');
			expect(details.flagMetadata).toEqual(metadata);
		});
	});

	describe('binding mode', () => {
		const bindingWith = (details: object) =>
			({
				getBooleanDetails: vi.fn().mockResolvedValue({ flagKey: 'f', value: true, variant: 'on', reason: 'STATIC', ...details }),
			}) as unknown as FlagshipBinding;

		it('maps binding metadata to flagMetadata', async () => {
			const provider = new FlagshipServerProvider({ binding: bindingWith({ metadata }) });
			expect((await provider.resolveBooleanEvaluation('f', false, {}, logger)).flagMetadata).toEqual(metadata);
		});

		it('returns {} when the binding omits metadata', async () => {
			const provider = new FlagshipServerProvider({ binding: bindingWith({}) });
			expect((await provider.resolveBooleanEvaluation('f', false, {}, logger)).flagMetadata).toEqual({});
		});
	});

	describe('local evaluation mode', () => {
		const flag = { key: 'f', enabled: true, default_variation: 'on', variations: { on: true }, rules: [], metadata };

		it('maps definition metadata to flagMetadata', async () => {
			const fetch = vi.fn(async () => Response.json({ flags: { f: flag, g: { ...flag, key: 'g', metadata: undefined } } }));
			const provider = new FlagshipServerProvider({ appId: 'app-1', accountId: 'acct-1', localEvaluation: true, fetch, retries: 0 });
			await provider.initialize();
			expect((await provider.resolveBooleanEvaluation('f', false, {}, logger)).flagMetadata).toEqual(metadata);
			expect((await provider.resolveBooleanEvaluation('g', false, {}, logger)).flagMetadata).toEqual({});
		});
	});
});
