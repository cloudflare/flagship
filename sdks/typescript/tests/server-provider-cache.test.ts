import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Logger } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';
import type { FlagshipBinding, FlagshipBindingEvaluationDetails } from '../src/types.js';

global.fetch = vi.fn();

const noopLogger: Logger = {
	debug: () => {},
	info: () => {},
	warn: () => {},
	error: () => {},
};

function mockResponse(value: unknown, reason = 'TARGETING_MATCH', variant = 'on'): void {
	(global.fetch as any).mockResolvedValueOnce({
		ok: true,
		json: async () => ({ flagKey: 'flag', value, variant, reason }),
	});
}

function createMockBinding(): FlagshipBinding {
	return {
		get: vi.fn((_flagKey: string, defaultValue?: unknown) => Promise.resolve(defaultValue)),
		getBooleanValue: vi.fn((_flagKey: string, defaultValue: boolean) => Promise.resolve(defaultValue)),
		getStringValue: vi.fn((_flagKey: string, defaultValue: string) => Promise.resolve(defaultValue)),
		getNumberValue: vi.fn((_flagKey: string, defaultValue: number) => Promise.resolve(defaultValue)),
		getObjectValue: vi.fn(<T extends object>(_flagKey: string, defaultValue: T) => Promise.resolve(defaultValue)),
		getBooleanDetails: vi.fn((flagKey: string): Promise<FlagshipBindingEvaluationDetails<boolean>> =>
			Promise.resolve({ flagKey, value: true, variant: 'on', reason: 'TARGETING_MATCH' }),
		),
		getStringDetails: vi.fn((flagKey: string, defaultValue: string): Promise<FlagshipBindingEvaluationDetails<string>> =>
			Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
		getNumberDetails: vi.fn((flagKey: string, defaultValue: number): Promise<FlagshipBindingEvaluationDetails<number>> =>
			Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
		getObjectDetails: vi.fn(<T extends object>(flagKey: string, defaultValue: T): Promise<FlagshipBindingEvaluationDetails<T>> =>
			Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
	};
}

describe('FlagshipServerProvider caching', () => {
	beforeEach(() => {
		(global.fetch as any).mockReset();
	});

	it('does not cache when cacheTtl is unset (default)', async () => {
		mockResponse(true);
		mockResponse(true);
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });

		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(2);
	});

	it('serves a cache hit without a second fetch and marks it CACHED', async () => {
		mockResponse(true, 'TARGETING_MATCH', 'on');
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', cacheTtl: 60_000 });

		const first = await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		const second = await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(1);
		expect(first.reason).toBe('TARGETING_MATCH');
		expect(second.value).toBe(true);
		expect(second.variant).toBe('on');
		expect(second.reason).toBe('CACHED');
	});

	it('keeps a separate entry per evaluation context', async () => {
		mockResponse(true);
		mockResponse(false);
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', cacheTtl: 60_000 });

		const a = await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		const b = await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u2' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(2);
		expect(a.value).toBe(true);
		expect(b.value).toBe(false);
	});

	it('re-fetches after the TTL expires', async () => {
		mockResponse(true);
		mockResponse(true);
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', cacheTtl: 20 });

		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		await new Promise((resolve) => setTimeout(resolve, 40));
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(2);
	});

	it('evicts least-recently-used entries beyond cacheMaxSize', async () => {
		mockResponse(true); // u1
		mockResponse(true); // u2
		mockResponse(true); // u1 again (evicted)
		const provider = new FlagshipServerProvider({
			endpoint: 'https://api.example.com/evaluate',
			cacheTtl: 60_000,
			cacheMaxSize: 1,
		});

		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u2' }, noopLogger);
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(3);
	});

	it('does not cache DISABLED results', async () => {
		mockResponse(true, 'DISABLED');
		mockResponse(true, 'DISABLED');
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', cacheTtl: 60_000 });

		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(2);
	});

	it('does not cache error results', async () => {
		(global.fetch as any).mockResolvedValue(new Response(null, { status: 404 }));
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', cacheTtl: 60_000 });

		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(2);
	});

	it('clears the cache on close', async () => {
		mockResponse(true);
		mockResponse(true);
		const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', cacheTtl: 60_000 });

		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		await provider.onClose();
		await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(global.fetch).toHaveBeenCalledTimes(2);
	});

	it('caches binding-mode evaluations', async () => {
		const binding = createMockBinding();
		const provider = new FlagshipServerProvider({ binding, cacheTtl: 60_000 });

		const first = await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);
		const second = await provider.resolveBooleanEvaluation('flag', false, { targetingKey: 'u1' }, noopLogger);

		expect(binding.getBooleanDetails).toHaveBeenCalledTimes(1);
		expect(first.reason).toBe('TARGETING_MATCH');
		expect(second.value).toBe(true);
		expect(second.reason).toBe('CACHED');
	});
});
