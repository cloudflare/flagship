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

		it('should accept prefetchFlags and cacheTTL options', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['flag1', 'flag2'],
				cacheTTL: 60000,
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
	});

	describe('cache behavior - cache miss', () => {
		it('should return default value for boolean flags when cache is empty', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.reason).toBe('DEFAULT');
			expect(result.errorCode).toBeUndefined();
		});

		it('should return default value for string flags when cache is empty', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveStringEvaluation('my-flag', 'default', {}, noopLogger);

			expect(result.value).toBe('default');
			expect(result.reason).toBe('DEFAULT');
		});

		it('should return default value for number flags when cache is empty', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = provider.resolveNumberEvaluation('my-flag', 42, {}, noopLogger);

			expect(result.value).toBe(42);
			expect(result.reason).toBe('DEFAULT');
		});

		it('should return default value for object flags when cache is empty', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const defaultValue = { key: 'value' };
			const result = provider.resolveObjectEvaluation('my-flag', defaultValue, {}, noopLogger);

			expect(result.value).toEqual(defaultValue);
			expect(result.reason).toBe('DEFAULT');
		});
	});

	describe('onContextChange - pre-fetching', () => {
		it('should pre-fetch configured flags during initialize with context', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
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

		it('should not throw during initialize without context', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode'],
			});

			await expect(provider.initialize()).resolves.not.toThrow();
			expect(mockEvaluate).toHaveBeenCalledWith('dark-mode', {});
		});

		it('should pre-fetch configured flags on context change', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode', 'welcome-message'],
			});

			const newContext = {
				targetingKey: 'user-123',
				email: 'user@example.com',
			};

			await provider.onContextChange({}, newContext);

			// Should have called evaluate for each prefetch flag
			expect(mockEvaluate).toHaveBeenCalledTimes(2);
			expect(mockEvaluate).toHaveBeenCalledWith('dark-mode', newContext);
			expect(mockEvaluate).toHaveBeenCalledWith('welcome-message', newContext);
		});

		it('should not pre-fetch if no flags configured', async () => {
			const mockEvaluate = vi.fn();

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			expect(mockEvaluate).not.toHaveBeenCalled();
		});

		it('should handle pre-fetch errors gracefully', async () => {
			const mockEvaluate = vi.fn().mockRejectedValue(new Error('Network error'));

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			// Should not throw
			await expect(provider.onContextChange({}, { targetingKey: 'user-123' })).resolves.not.toThrow();
		});

		it('should handle empty context changes', async () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await expect(provider.onContextChange({}, {})).resolves.not.toThrow();
		});
	});

	describe('cache behavior - cache hit', () => {
		it('should return cached value after pre-fetch', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
				reason: 'TARGETING_MATCH',
				variant: 'on',
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['dark-mode'],
			});

			// Pre-fetch the flag
			await provider.onContextChange({}, { targetingKey: 'user-123' });

			// Resolve from cache
			const result = provider.resolveBooleanEvaluation('dark-mode', false, {}, noopLogger);

			expect(result.value).toBe(true);
			expect(result.reason).toBe('CACHED');
			expect(result.variant).toBe('on');
		});

		it('should return cached string value', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'welcome-message',
				value: 'Hello, user!',
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['welcome-message'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveStringEvaluation('welcome-message', 'default', {}, noopLogger);

			expect(result.value).toBe('Hello, user!');
			expect(result.reason).toBe('CACHED');
		});

		it('should return cached number value', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'max-uploads',
				value: 10,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['max-uploads'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveNumberEvaluation('max-uploads', 5, {}, noopLogger);

			expect(result.value).toBe(10);
			expect(result.reason).toBe('CACHED');
		});

		it('should return cached object value', async () => {
			const themeConfig = {
				primary: '#007bff',
				secondary: '#6c757d',
			};

			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'theme-config',
				value: themeConfig,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['theme-config'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveObjectEvaluation('theme-config', {}, {}, noopLogger);

			expect(result.value).toEqual(themeConfig);
			expect(result.reason).toBe('CACHED');
		});
	});

	describe('type checking', () => {
		it('should return TYPE_MISMATCH when cached type does not match expected type', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'my-flag',
				value: 'string-value', // String value
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			// Try to resolve as boolean (but cache has string)
			const result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false); // Default value
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('type mismatch');
			expect(result.errorMessage).toContain('expected boolean');
			expect(result.errorMessage).toContain('got string');
			expect(result.reason).toBe('ERROR');
		});

		it('should return TYPE_MISMATCH for boolean when expecting number', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'my-flag',
				value: true,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveNumberEvaluation('my-flag', 0, {}, noopLogger);

			expect(result.value).toBe(0);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});

		it('should return TYPE_MISMATCH for object when expecting string', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'my-flag',
				value: { key: 'value' },
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveStringEvaluation('my-flag', 'default', {}, noopLogger);

			expect(result.value).toBe('default');
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});
	});

	describe('cache TTL', () => {
		it('should expire cached values after TTL', async () => {
			vi.useFakeTimers();

			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'my-flag',
				value: true,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
				cacheTTL: 60000, // 1 minute
			});

			// Pre-fetch flag
			await provider.onContextChange({}, { targetingKey: 'user-123' });

			// Should return cached value
			let result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);
			expect(result.value).toBe(true);
			expect(result.reason).toBe('CACHED');

			// Advance time by 61 seconds (past TTL)
			vi.advanceTimersByTime(61000);

			// Should return default value (cache expired)
			result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);
			expect(result.value).toBe(false);
			expect(result.reason).toBe('DEFAULT');

			vi.useRealTimers();
		});

		it('should not expire cached values when TTL is 0', async () => {
			vi.useFakeTimers();

			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'my-flag',
				value: true,
			});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
				cacheTTL: 0, // No expiry
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			// Should return cached value
			let result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);
			expect(result.value).toBe(true);

			// Advance time by a long time
			vi.advanceTimersByTime(1000000000);

			// Should still return cached value (no expiry)
			result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);
			expect(result.value).toBe(true);
			expect(result.reason).toBe('CACHED');

			vi.useRealTimers();
		});
	});

	describe('metadata', () => {
		it('flagMetadata is always empty (API does not return metadata)', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({
				flagKey: 'my-flag',
				value: true,
				variant: 'on',
				reason: 'DEFAULT',
			});

			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			await provider.onContextChange({}, { targetingKey: 'user-123' });

			const result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.flagMetadata).toEqual({});
		});

		it('should have correct provider name', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			expect(provider.metadata.name).toBe('Flagship Client Provider');
		});

		it('should specify client runtime', () => {
			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			expect(provider.runsOn).toBe('client');
		});
	});

	describe('multiple context changes', () => {
		it('should re-fetch flags on each context change', async () => {
			const mockEvaluate = vi
				.fn()
				.mockResolvedValueOnce({
					flagKey: 'my-flag',
					value: true,
				})
				.mockResolvedValueOnce({
					flagKey: 'my-flag',
					value: false,
				});

			(FlagshipClient as any).mockImplementation(function () {
				return {
					evaluate: mockEvaluate,
				};
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['my-flag'],
			});

			// First context change
			await provider.onContextChange({}, { targetingKey: 'user-123' });
			let result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);
			expect(result.value).toBe(true);

			// Second context change
			await provider.onContextChange({ targetingKey: 'user-123' }, { targetingKey: 'user-456' });
			result = provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);
			expect(result.value).toBe(false);

			expect(mockEvaluate).toHaveBeenCalledTimes(2);
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

		it('initialize with no prefetchFlags skips evaluate entirely', async () => {
			const mockEvaluate = vi.fn();
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.initialize({ targetingKey: 'user-1' });

			expect(mockEvaluate).not.toHaveBeenCalled();
		});

		it('initialize emits ProviderEvents.Ready', async () => {
			const { ProviderEvents } = require('@openfeature/web-sdk');
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			const handler = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, handler);
			await provider.initialize();
			expect(handler).toHaveBeenCalled();
		});

		it('initialize still reaches READY when some pre-fetches fail', async () => {
			const { ProviderStatus } = require('@openfeature/web-sdk');
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

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
			expect(consoleSpy).toHaveBeenCalled();

			consoleSpy.mockRestore();
		});

		it('onClose clears the cache', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({ flagKey: 'f', value: true });
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, {});
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).reason).toBe('CACHED');

			await provider.onClose();
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).reason).toBe('DEFAULT');
		});

		it('onClose resets status to NOT_READY', async () => {
			const { ProviderStatus } = require('@openfeature/web-sdk');
			const provider = new FlagshipClientProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.initialize();
			await provider.onClose();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('onContextChange invalidates cache before re-fetching so a failed fetch yields DEFAULT', async () => {
			const mockEvaluate = vi.fn().mockResolvedValueOnce({ flagKey: 'f', value: true }).mockRejectedValueOnce(new Error('network'));

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
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).reason).toBe('DEFAULT');
		});
	});

	describe('resolution edge cases', () => {
		it('flagMetadata defaults to {} when cached metadata is undefined', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({ flagKey: 'f', value: true });
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, {});
			const result = provider.resolveBooleanEvaluation('f', false, {}, noopLogger);
			expect(result.flagMetadata).toEqual({});
		});

		it('null value from API is classified as object type', async () => {
			const mockEvaluate = vi.fn().mockResolvedValue({ flagKey: 'f', value: null });
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
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

		it('cacheTTL defaults to 0 (no expiry) when not specified', async () => {
			vi.useFakeTimers();

			const mockEvaluate = vi.fn().mockResolvedValue({ flagKey: 'f', value: true });
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await provider.onContextChange({}, {});
			vi.advanceTimersByTime(9_999_999);
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).reason).toBe('CACHED');

			vi.useRealTimers();
		});

		it('INVALID_CONTEXT from complex context on onContextChange is absorbed by allSettled', async () => {
			const mockEvaluate = vi.fn().mockRejectedValue(Object.assign(new Error('ctx'), { code: 'INVALID_CONTEXT' }));
			(FlagshipClient as any).mockImplementation(function () {
				return { evaluate: mockEvaluate };
			});

			const provider = new FlagshipClientProvider({
				endpoint: 'https://api.example.com/evaluate',
				prefetchFlags: ['f'],
			});

			await expect(provider.onContextChange({}, { nested: {} as any })).resolves.not.toThrow();
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).reason).toBe('DEFAULT');
		});
	});
});
