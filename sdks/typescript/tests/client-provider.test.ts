import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Logger } from '@openfeature/web-sdk';
import { ErrorCode, ProviderEvents, ProviderStatus } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '../src/client-provider.js';
import { FlagshipClient } from '../src/client.js';
import { FlagshipError, FlagshipErrorCode, type FlagshipBulkEvaluationResult } from '../src/types.js';

const noopLogger: Logger = { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} };
const spyLogger = (): Logger => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() });
const endpoint = 'https://api.example.com/evaluate';

vi.mock('../src/client.js', async (importOriginal) => ({
	...(await importOriginal<typeof import('../src/client.js')>()),
	FlagshipClient: vi.fn().mockImplementation(function () {
		return { evaluateAll: vi.fn().mockResolvedValue([]) };
	}),
}));

function mockBulk(...responses: Array<FlagshipBulkEvaluationResult[] | Error>) {
	const evaluateAll = vi.fn();
	for (const response of responses) {
		if (response instanceof Error) evaluateAll.mockRejectedValueOnce(response);
		else evaluateAll.mockResolvedValueOnce(response);
	}
	(FlagshipClient as any).mockImplementation(function () {
		return { evaluateAll };
	});
	return evaluateAll;
}

async function initialized(flags: FlagshipBulkEvaluationResult[], options: { logging?: boolean } = {}) {
	mockBulk(flags);
	const provider = new FlagshipClientProvider({ endpoint, ...options });
	await provider.initialize({});
	return provider;
}

describe('FlagshipClientProvider', () => {
	beforeEach(() => {
		vi.clearAllMocks();
	});

	describe('constructor', () => {
		it('creates a client runtime provider', () => {
			const provider = new FlagshipClientProvider({ endpoint });

			expect(provider.metadata.name).toBe('Flagship Client Provider');
			expect(provider.runsOn).toBe('client');
		});

		it('resolves a relative endpoint against window.location.origin', () => {
			vi.stubGlobal('window', { location: { origin: 'https://app.example.com' } });

			new FlagshipClientProvider({ endpoint: '/api/flagship/evaluate' });

			expect(FlagshipClient).toHaveBeenCalledWith(expect.objectContaining({ endpoint: 'https://app.example.com/api/flagship/evaluate' }));

			vi.unstubAllGlobals();
		});

		it('leaves an absolute endpoint untouched', () => {
			vi.stubGlobal('window', { location: { origin: 'https://app.example.com' } });

			new FlagshipClientProvider({ endpoint });

			expect(FlagshipClient).toHaveBeenCalledWith(expect.objectContaining({ endpoint }));

			vi.unstubAllGlobals();
		});

		it('throws when a relative endpoint is used without a browser context', () => {
			vi.stubGlobal('window', undefined);

			expect(() => new FlagshipClientProvider({ endpoint: '/api/flagship/evaluate' })).toThrow(/requires a browser context/);

			vi.unstubAllGlobals();
		});
	});

	describe('initialize', () => {
		it('evaluates every flag once with the given context', async () => {
			const evaluateAll = mockBulk([{ key: 'dark-mode', value: true, reason: 'TARGETING_MATCH', variant: 'on' }]);
			const provider = new FlagshipClientProvider({ endpoint });

			await provider.initialize({ targetingKey: 'user-123' });

			expect(evaluateAll).toHaveBeenCalledTimes(1);
			expect(evaluateAll).toHaveBeenCalledWith({ targetingKey: 'user-123' });
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('emits ProviderEvents.Ready', async () => {
			mockBulk([]);
			const provider = new FlagshipClientProvider({ endpoint });
			const handler = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, handler);

			await provider.initialize();

			expect(handler).toHaveBeenCalled();
		});

		it('rejects when the request fails', async () => {
			mockBulk(new Error('network'));
			const provider = new FlagshipClientProvider({ endpoint });

			await expect(provider.initialize()).rejects.toThrow('network');
			expect(provider.status).toBe(ProviderStatus.ERROR);
		});

		it.each([401, 403])('rejects with PROVIDER_FATAL on %i', async (status) => {
			mockBulk(new FlagshipError(`HTTP ${status}`, FlagshipErrorCode.NETWORK_ERROR, { status }));
			const provider = new FlagshipClientProvider({ endpoint });

			await expect(provider.initialize()).rejects.toMatchObject({ code: ErrorCode.PROVIDER_FATAL });
			expect(provider.status).toBe(ProviderStatus.FATAL);
		});

		it('logs the failure only when logging is enabled', async () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			mockBulk(new Error('timeout'), new Error('timeout'));
			await expect(new FlagshipClientProvider({ endpoint }).initialize()).rejects.toThrow();
			expect(consoleSpy).not.toHaveBeenCalled();

			await expect(new FlagshipClientProvider({ endpoint, logging: true }).initialize()).rejects.toThrow();
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('timeout'));

			consoleSpy.mockRestore();
		});
	});

	describe('onContextChange', () => {
		it('re-evaluates every flag for the new context', async () => {
			const evaluateAll = mockBulk(
				[{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }],
				[{ key: 'f', value: false, reason: 'DEFAULT', variant: 'off' }],
			);
			const provider = new FlagshipClientProvider({ endpoint });

			await provider.onContextChange({}, { targetingKey: 'user-1' });
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);

			await provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' });
			expect(provider.resolveBooleanEvaluation('f', true, {}, noopLogger).value).toBe(false);
			expect(evaluateAll).toHaveBeenLastCalledWith({ targetingKey: 'user-2' });
		});

		it('rejects without serving values from the previous context', async () => {
			mockBulk([{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }], new Error('network'));
			const provider = new FlagshipClientProvider({ endpoint });

			await provider.onContextChange({}, { targetingKey: 'user-1' });
			await expect(provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' })).rejects.toThrow('network');

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
			expect(provider.status).toBe(ProviderStatus.ERROR);
		});

		it('returns to READY once a later context change succeeds', async () => {
			mockBulk(new Error('network'), [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }]);
			const provider = new FlagshipClientProvider({ endpoint });

			await expect(provider.onContextChange({}, { targetingKey: 'user-1' })).rejects.toThrow('network');
			await provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' });

			expect(provider.status).toBe(ProviderStatus.READY);
		});
	});

	describe('resolution', () => {
		it('returns the evaluated value, variant and reason for each type', async () => {
			const theme = { primary: '#007bff' };
			const provider = await initialized([
				{ key: 'bool', value: true, reason: 'TARGETING_MATCH', variant: 'on' },
				{ key: 'str', value: 'Hello!', reason: 'DEFAULT', variant: 'default' },
				{ key: 'num', value: 10, reason: 'SPLIT', variant: 'ten' },
				{ key: 'obj', value: theme, reason: 'STATIC', variant: 'default' },
				{ key: 'nil', value: null, reason: 'DEFAULT', variant: 'default' },
			]);

			expect(provider.resolveBooleanEvaluation('bool', false, {}, noopLogger)).toMatchObject({
				value: true,
				reason: 'TARGETING_MATCH',
				variant: 'on',
			});
			expect(provider.resolveStringEvaluation('str', 'fallback', {}, noopLogger)).toMatchObject({ value: 'Hello!', reason: 'DEFAULT' });
			expect(provider.resolveNumberEvaluation('num', 5, {}, noopLogger)).toMatchObject({ value: 10, reason: 'SPLIT' });
			expect(provider.resolveObjectEvaluation('obj', {}, {}, noopLogger).value).toEqual(theme);
			const nil = provider.resolveObjectEvaluation('nil', {}, {}, noopLogger);
			expect(nil.value).toBeNull();
			expect(nil.errorCode).toBeUndefined();
		});

		it('passes flag metadata through', async () => {
			const provider = await initialized([
				{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on', metadata: { owner: 'web', tier: 2 } },
			]);

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).flagMetadata).toEqual({ owner: 'web', tier: 2 });
		});

		it('returns the caller default for a disabled flag', async () => {
			const provider = await initialized([{ key: 'f', reason: 'DISABLED', metadata: { owner: 'web' } }]);

			expect(provider.resolveBooleanEvaluation('f', true, {}, noopLogger)).toEqual({
				value: true,
				reason: 'DISABLED',
				flagMetadata: { owner: 'web' },
			});
		});

		it('maps a per-flag failure to its OpenFeature error code', async () => {
			const provider = await initialized([{ key: 'f', errorCode: 'PARSE_ERROR', errorDetails: 'bad config' }]);

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger)).toEqual({
				value: false,
				reason: 'ERROR',
				errorCode: ErrorCode.PARSE_ERROR,
				errorMessage: 'bad config',
			});
		});

		it('maps an unknown per-flag error code to GENERAL', async () => {
			const provider = await initialized([{ key: 'f', errorCode: 'SOMETHING_NEW' }]);

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.GENERAL);
		});

		it('returns TYPE_MISMATCH when the value type differs from the request', async () => {
			const provider = await initialized([{ key: 'f', value: 'string-value', reason: 'DEFAULT', variant: 'v' }]);

			const result = provider.resolveBooleanEvaluation('f', false, {}, noopLogger);

			expect(result).toMatchObject({ value: false, errorCode: ErrorCode.TYPE_MISMATCH, reason: 'ERROR' });
			expect(result.errorMessage).toContain('expected boolean, got string');
		});

		it('returns FLAG_NOT_FOUND for a flag missing from the response', async () => {
			const provider = await initialized([]);

			expect(provider.resolveBooleanEvaluation('missing', false, {}, noopLogger)).toMatchObject({
				value: false,
				reason: 'ERROR',
				errorCode: ErrorCode.FLAG_NOT_FOUND,
			});
		});

		it('logs warnings through the injected logger only when logging is enabled', async () => {
			const entries: FlagshipBulkEvaluationResult[] = [{ key: 'f', value: 'x', reason: 'DEFAULT', variant: 'v' }];
			const quiet = spyLogger();
			const loud = spyLogger();

			const silent = await initialized(entries);
			silent.resolveBooleanEvaluation('missing', false, {}, quiet);
			silent.resolveBooleanEvaluation('f', false, {}, quiet);
			expect(quiet.warn).not.toHaveBeenCalled();

			const verbose = await initialized(entries, { logging: true });
			verbose.resolveBooleanEvaluation('missing', false, {}, loud);
			verbose.resolveBooleanEvaluation('f', false, {}, loud);
			expect(loud.warn).toHaveBeenCalledWith(expect.stringContaining('missing'));
			expect(loud.warn).toHaveBeenCalledWith(expect.stringContaining('type mismatch'));
		});
	});

	describe('lifecycle', () => {
		it('is NOT_READY before initialize and after onClose', async () => {
			const provider = await initialized([]);
			expect(provider.status).toBe(ProviderStatus.READY);

			await provider.onClose();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('clears the cache on close', async () => {
			const provider = await initialized([{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }]);

			await provider.onClose();

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('discards an in-flight initialize on close without emitting READY', async () => {
			let resolveEvaluate!: (flags: FlagshipBulkEvaluationResult[]) => void;
			const evaluateAll = mockBulk();
			evaluateAll.mockReturnValueOnce(new Promise((resolve) => (resolveEvaluate = resolve)));
			const provider = new FlagshipClientProvider({ endpoint });
			const onReady = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, onReady);

			const initializing = provider.initialize({});
			await provider.onClose();
			resolveEvaluate([{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }]);
			await initializing;

			expect(onReady).not.toHaveBeenCalled();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('onClose is idempotent', async () => {
			const provider = await initialized([]);
			await provider.onClose();
			await expect(provider.onClose()).resolves.toBeUndefined();
		});
	});
});
