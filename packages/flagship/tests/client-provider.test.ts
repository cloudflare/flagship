import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Logger } from '@openfeature/web-sdk';
import { ErrorCode } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '../src/client-provider.js';
import { FlagshipClient } from '../src/client.js';

const noopLogger: Logger = { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} };

// Mock FlagshipClient
vi.mock('../src/client.js', () => ({
	FlagshipClient: vi.fn().mockImplementation(function () {
		return { evaluate: vi.fn() };
	}),
}));

describe('FlagshipClientProvider', () => {
	beforeEach(() => {
		vi.clearAllMocks();
	});

	describe('constructor', () => {
		it('should create provider with valid options', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			expect(provider).toBeInstanceOf(FlagshipClientProvider);
			expect(provider.metadata.name).toBe('Flagship Client Provider');
			expect(provider.runsOn).toBe('client');
		});

		it('should accept prefetchFlags option', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['flag1', 'flag2'],
			});

			expect(provider).toBeInstanceOf(FlagshipClientProvider);
		});

		it('should accept custom timeout and retries', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				timeout: 10000,
				retries: 3,
			});

			expect(provider).toBeInstanceOf(FlagshipClientProvider);
		});

		it('resolves a relative endpoint against window.location.origin', () => {
			vi.stubGlobal('window', { location: { origin: 'https://app.example.com' } });

			new FlagshipClientProvider({ endpoint: '/api/flagship/evaluate' });

			expect(FlagshipClient).toHaveBeenCalledWith(expect.objectContaining({ endpoint: 'https://app.example.com/api/flagship/evaluate' }));

			vi.unstubAllGlobals();
		});

		it('leaves an absolute endpoint untouched', () => {
			vi.stubGlobal('window', { location: { origin: 'https://app.example.com' } });

			new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });

			expect(FlagshipClient).toHaveBeenCalledWith(expect.objectContaining({ endpoint: 'https://api.example.com/evaluate' }));

			vi.unstubAllGlobals();
		});

		it('throws when a relative endpoint is used without a browser context', () => {
			vi.stubGlobal('window', undefined);

			expect(() => new FlagshipClientProvider({ endpoint: '/api/flagship/evaluate' })).toThrow(/requires a browser context/);

			vi.unstubAllGlobals();
		});
	});

	describe('cache miss — FLAG_NOT_FOUND', () => {
		it('returns FLAG_NOT_FOUND for boolean flag not in cache', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.reason).toBe('ERROR');
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('returns FLAG_NOT_FOUND for string flag not in cache', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveStringEvaluation('my-flag', 'default', {}, noopLogger);

			expect(result.value).toBe('default');
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('returns FLAG_NOT_FOUND for number flag not in cache', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveNumberEvaluation('my-flag', 42, {}, noopLogger);

			expect(result.value).toBe(42);
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('returns FLAG_NOT_FOUND for object flag not in cache', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveObjectEvaluation('my-flag', { key: 'value' }, {}, noopLogger);

			expect(result.value).toEqual({ key: 'value' });
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('logs FLAG_NOT_FOUND warning via injected logger when logging is true', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				logging: true,
			});

			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
			provider.resolveBooleanEvaluation('missing-flag', false, {}, spyLogger);

			expect(spyLogger.warn).toHaveBeenCalledWith(expect.stringContaining('missing-flag'));
			expect(spyLogger.warn).toHaveBeenCalledWith(expect.stringContaining('prefetchFlags'));
		});

		it('does not log when logging is false (default)', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
			provider.resolveBooleanEvaluation('missing-flag', false, {}, spyLogger);

			expect(spyLogger.warn).not.toHaveBeenCalled();
		});
	});

	describe('initialize — prefetch', () => {
		it('fetches all prefetchFlags with the given context', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
				reason: 'TARGETING_MATCH',
				variant: 'on',
			});

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode'],
			});

			const context = { targetingKey: 'user-123' };
			await provider.initialize(context);

			expect(mockEvaluate).toHaveBeenCalledTimes(1);
			expect(mockEvaluate).toHaveBeenCalledWith('dark-mode', context);
		});

		it('resolves CACHED after successful initialize', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({
						flagKey: 'dark-mode',
						value: true,
						reason: 'TARGETING_MATCH',
						variant: 'on',
					}),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode'],
			});

			await provider.initialize({ targetingKey: 'user-123' });

			const result = provider.resolveBooleanEvaluation('dark-mode', false, {}, noopLogger);
			expect(result.value).toBe(true);
			expect(result.reason).toBe('CACHED');
			expect(result.variant).toBe('on');
		});

		it('skips fetching when no prefetchFlags configured', async () => {
			const mockEvaluate = vi.fn();
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await provider.initialize({ targetingKey: 'user-1' });

			expect(mockEvaluate).not.toHaveBeenCalled();
		});

		it('still reaches READY when some pre-fetches fail', async () => {
			const { ProviderStatus } = require('@openfeature/web-sdk');

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockRejectedValue(new Error('network')),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['flag1', 'flag2'],
			});

			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('emits ProviderEvents.Ready after initialize', async () => {
			const { ProviderEvents } = require('@openfeature/web-sdk');

			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			const handler = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, handler);
			await provider.initialize();
			expect(handler).toHaveBeenCalled();
		});

		it('does not log on failure when logging is false (default)', async () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: vi.fn().mockRejectedValue(new Error('network')) };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['flag1'],
			});

			await provider.initialize();
			expect(consoleSpy).not.toHaveBeenCalled();
			consoleSpy.mockRestore();
		});

		it('logs per-flag failure with flag key and error message when logging is true', async () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: vi.fn().mockRejectedValue(new Error('network failure')) };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['flag1', 'flag2'],
				logging: true,
			});

			await provider.initialize();

			expect(consoleSpy).toHaveBeenCalledTimes(2);
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('flag1'));
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('flag2'));
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('network failure'));
			consoleSpy.mockRestore();
		});
	});

	describe('onContextChange', () => {
		it('re-fetches all prefetchFlags with new context', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
				reason: 'TARGETING_MATCH',
				variant: 'on',
			});

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode', 'welcome-message'],
			});

			const newContext = { targetingKey: 'user-123' };
			await provider.onContextChange({}, newContext);

			expect(mockEvaluate).toHaveBeenCalledTimes(2);
			expect(mockEvaluate).toHaveBeenCalledWith('dark-mode', newContext);
			expect(mockEvaluate).toHaveBeenCalledWith('welcome-message', newContext);
		});

		it('invalidates entire cache before re-fetching', async () => {
			const mockEvaluate = vi
				.fn()
				.mockResolvedValueOnce({ flagKey: 'f', value: true, reason: 'DEFAULT', variant: 'on' })
				.mockRejectedValueOnce(new Error('network'));

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-1' });
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);

			// Second context change: fetch fails — stale value must NOT be served
			await provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' });
			const result = provider.resolveBooleanEvaluation('f', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('does not fetch when no prefetchFlags configured', async () => {
			const mockEvaluate = vi.fn();
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.onContextChange({}, { targetingKey: 'user-123' });

			expect(mockEvaluate).not.toHaveBeenCalled();
		});

		it('handles context change without argument', async () => {
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			await expect(provider.onContextChange({}, {})).resolves.not.toThrow();
		});

		it('logs per-flag failure during context change when logging is true', async () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: vi.fn().mockRejectedValue(new Error('timeout')) };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['flag1'],
				logging: true,
			});

			await provider.onContextChange({}, { targetingKey: 'user-1' });

			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('flag1'));
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('timeout'));
			consoleSpy.mockRestore();
		});
	});

	describe('cache hit resolution', () => {
		it('returns cached boolean value with CACHED reason', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({
						flagKey: 'dark-mode',
						value: true,
						reason: 'TARGETING_MATCH',
						variant: 'on',
					}),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveBooleanEvaluation('dark-mode', false, {}, noopLogger);
			expect(result.value).toBe(true);
			expect(result.reason).toBe('CACHED');
			expect(result.variant).toBe('on');
		});

		it('returns cached string value', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'msg', value: 'Hello!', reason: 'DEFAULT', variant: 'default' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['msg'],
			});

			await provider.onContextChange({}, {});
			const result = provider.resolveStringEvaluation('msg', 'fallback', {}, noopLogger);
			expect(result.value).toBe('Hello!');
			expect(result.reason).toBe('CACHED');
		});

		it('returns cached number value', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'limit', value: 10, reason: 'DEFAULT', variant: 'default' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['limit'],
			});

			await provider.onContextChange({}, {});
			const result = provider.resolveNumberEvaluation('limit', 5, {}, noopLogger);
			expect(result.value).toBe(10);
		});

		it('returns cached object value', async () => {
			const theme = { primary: '#007bff' };
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'theme', value: theme, reason: 'DEFAULT', variant: 'default' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['theme'],
			});

			await provider.onContextChange({}, {});
			const result = provider.resolveObjectEvaluation('theme', {}, {}, noopLogger);
			expect(result.value).toEqual(theme);
		});

		it('flagMetadata is always {} on a cache hit', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'f', value: true, reason: 'DEFAULT', variant: 'on' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, {});
			const result = provider.resolveBooleanEvaluation('f', false, {}, noopLogger);
			expect(result.flagMetadata).toEqual({});
		});
	});

	describe('type checking', () => {
		it('returns TYPE_MISMATCH when cached type does not match expected type', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'my-flag', value: 'string-value', reason: 'DEFAULT', variant: 'v' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected boolean, got string');
			expect(result.reason).toBe('ERROR');
		});

		it('calls logger.warn on type mismatch when logging is true', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'my-flag', value: 'string-value', reason: 'DEFAULT', variant: 'v' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
				logging: true,
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
			provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.warn).toHaveBeenCalledWith(expect.stringContaining('type mismatch'));
		});

		it('does not call logger.warn on type mismatch when logging is false (default)', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'my-flag', value: 'string-value', reason: 'DEFAULT', variant: 'v' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };
			provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.warn).not.toHaveBeenCalled();
		});

		it('null value from API is classified as object type', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'f', value: null, reason: 'DEFAULT', variant: 'default' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, {});
			const result = provider.resolveObjectEvaluation('f', {}, {}, noopLogger);
			expect(result.value).toBeNull();
			expect(result.errorCode).toBeUndefined();
		});
	});

	describe('lifecycle', () => {
		it('status is NOT_READY before initialize', () => {
			const { ProviderStatus } = require('@openfeature/web-sdk');
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('status is READY after initialize', async () => {
			const { ProviderStatus } = require('@openfeature/web-sdk');
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('status resets to NOT_READY after onClose', async () => {
			const { ProviderStatus } = require('@openfeature/web-sdk');
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.initialize();
			await provider.onClose();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('onClose clears the cache', async () => {
			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: vi.fn().mockResolvedValue({ flagKey: 'f', value: true, reason: 'DEFAULT', variant: 'on' }),
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, {});
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).reason).toBe('CACHED');

			await provider.onClose();
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('re-fetches flags on each context change', async () => {
			const mockEvaluate = vi
				.fn()
				.mockResolvedValueOnce({ flagKey: 'f', value: true, reason: 'DEFAULT', variant: 'on' })
				.mockResolvedValueOnce({ flagKey: 'f', value: false, reason: 'DEFAULT', variant: 'off' });

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-1' });
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);

			await provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' });
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(false);

			expect(mockEvaluate).toHaveBeenCalledTimes(2);
		});
	});

	describe('metadata', () => {
		it('has correct provider name', () => {
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			expect(provider.metadata.name).toBe('Flagship Client Provider');
		});

		it('specifies client runtime', () => {
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			expect(provider.runsOn).toBe('client');
		});
	});
});
