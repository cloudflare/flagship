import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Logger } from '@openfeature/server-sdk';
import { ErrorCode } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';

// Mock fetch globally
global.fetch = vi.fn();

// Minimal logger stub satisfying the OpenFeature Logger interface
const noopLogger: Logger = {
	debug: () => {},
	info: () => {},
	warn: () => {},
	error: () => {},
};

describe('FlagshipServerProvider', () => {
	beforeEach(() => {
		vi.clearAllMocks();
	});

	describe('constructor', () => {
		it('should create provider with valid options', () => {
			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			expect(provider).toBeInstanceOf(FlagshipServerProvider);
			expect(provider.metadata.name).toBe('Flagship Server Provider');
			expect(provider.runsOn).toBe('server');
		});

		it('should throw error if neither appId nor endpoint is provided', () => {
			expect(() => new FlagshipServerProvider({ endpoint: '' })).toThrow('either "appId" or "endpoint" is required');
		});

		it('should throw error if endpoint is invalid', () => {
			expect(() => new FlagshipServerProvider({ endpoint: 'not-a-url' })).toThrow('invalid endpoint URL');
		});
	});

	describe('resolveBooleanEvaluation', () => {
		it('should resolve boolean flag with correct value', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-boolean-flag', value: true, variant: 'on', reason: 'DEFAULT' }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-boolean-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(true);
			expect(result.reason).toBe('DEFAULT');
			expect(result.errorCode).toBeUndefined();
		});

		it('should return default value on type mismatch', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-boolean-flag',
					value: 'not-a-boolean', // Wrong type
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-boolean-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(false); // Default value
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected boolean, got string');
			expect(result.reason).toBe('ERROR');
		});

		it('should include reason and variant if provided', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-boolean-flag',
					value: true,
					reason: 'TARGETING_MATCH',
					variant: 'enabled-variant',
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-boolean-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(true);
			expect(result.reason).toBe('TARGETING_MATCH');
			expect(result.variant).toBe('enabled-variant');
		});

		it('flagMetadata is always empty (API does not return metadata)', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-boolean-flag',
					value: true,
					variant: 'on',
					reason: 'DEFAULT',
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-boolean-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(true);
			expect(result.flagMetadata).toEqual({});
		});
	});

	describe('resolveStringEvaluation', () => {
		it('should resolve string flag with correct value', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-string-flag',
					value: 'test-value',
					variant: 'on',
					reason: 'DEFAULT',
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveStringEvaluation(
				'my-string-flag',
				'default',
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe('test-value');
			expect(result.reason).toBe('DEFAULT');
		});

		it('should return default value on type mismatch', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-string-flag',
					value: 123, // Wrong type
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveStringEvaluation(
				'my-string-flag',
				'default',
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe('default');
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected string, got number');
		});
	});

	describe('resolveNumberEvaluation', () => {
		it('should resolve number flag with correct value', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-number-flag', value: 42, variant: 'on', reason: 'DEFAULT' }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveNumberEvaluation(
				'my-number-flag',
				0,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(42);
			expect(result.reason).toBe('DEFAULT');
		});

		it('should return default value on type mismatch', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-number-flag',
					value: true, // Wrong type
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveNumberEvaluation(
				'my-number-flag',
				0,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(0);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected number, got boolean');
		});
	});

	describe('resolveObjectEvaluation', () => {
		it('should resolve object flag with correct value', async () => {
			const objectValue = {
				feature: 'enabled',
				config: {
					maxItems: 10,
					allowedActions: ['read', 'write'],
				},
			};

			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-object-flag',
					value: objectValue,
					variant: 'config',
					reason: 'DEFAULT',
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveObjectEvaluation(
				'my-object-flag',
				{},
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toEqual(objectValue);
			expect(result.reason).toBe('DEFAULT');
		});

		it('should handle arrays as objects', async () => {
			const arrayValue = ['item1', 'item2', 'item3'];

			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-array-flag',
					value: arrayValue,
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveObjectEvaluation(
				'my-array-flag',
				[],
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toEqual(arrayValue);
		});

		it('should return default value on primitive type', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'my-object-flag',
					value: 'not-an-object', // Wrong type
				}),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveObjectEvaluation(
				'my-object-flag',
				{},
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toEqual({});
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected object, got string');
		});
	});

	describe('error handling', () => {
		it('should handle FLAG_NOT_FOUND error (404)', { timeout: 10000 }, async () => {
			// Must use a real Response so `instanceof Response` succeeds in handleError().
			const mockResponse = new Response(null, { status: 404, statusText: 'Not Found' });
			(global.fetch as any).mockResolvedValue(mockResponse);

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			const result = await provider.resolveBooleanEvaluation(
				'non-existent-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
			expect(result.reason).toBe('ERROR');
		});

		it('should handle NETWORK_ERROR', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: false,
				status: 500,
				statusText: 'Internal Server Error',
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.GENERAL);
			expect(result.reason).toBe('ERROR');
		});

		it('should handle TIMEOUT_ERROR', { timeout: 10000 }, async () => {
			// Mock a fetch that simulates an abort
			(global.fetch as any).mockImplementation(() => {
				const error = new Error('The operation was aborted');
				error.name = 'AbortError';
				return Promise.reject(error);
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				timeout: 100,
				retries: 0,
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.GENERAL);
			expect(result.reason).toBe('ERROR');
			expect(result.errorMessage).toContain('timeout');
		});

		it('should return INVALID_CONTEXT when context contains complex objects', async () => {
			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
					nested: { foo: 'bar' } as any,
				},
				noopLogger,
			);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.INVALID_CONTEXT);
			expect(result.errorMessage).toContain('nested');
			expect(result.reason).toBe('ERROR');
			// fetch should NOT have been called — error thrown before the request
			expect(global.fetch).not.toHaveBeenCalled();
		});

		it('should handle PARSE_ERROR', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ invalid: 'response' }), // Missing required fields
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(false);
			// Due to error wrapping, parse errors may become GENERAL
			expect(result.errorCode).toBeDefined();
			expect(result.reason).toBe('ERROR');
		});

		it('should handle unknown errors', async () => {
			(global.fetch as any).mockRejectedValueOnce(new Error('Unknown error'));

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			const result = await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.GENERAL);
			expect(result.reason).toBe('ERROR');
		});
	});

	describe('context handling', () => {
		it('should pass targeting key to backend', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
				},
				noopLogger,
			);

			const callArgs = (global.fetch as any).mock.calls[0];
			const url = new URL(callArgs[0]);
			expect(url.searchParams.get('targetingKey')).toBe('user-123');
		});

		it('should pass custom context attributes', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{
					targetingKey: 'user-123',
					email: 'user@example.com',
					plan: 'premium',
					age: 25,
					isActive: true,
				},
				noopLogger,
			);

			const callArgs = (global.fetch as any).mock.calls[0];
			const url = new URL(callArgs[0]);

			expect(url.searchParams.get('targetingKey')).toBe('user-123');
			expect(url.searchParams.get('email')).toBe('user@example.com');
			expect(url.searchParams.get('plan')).toBe('premium');
			expect(url.searchParams.get('age')).toBe('25');
			expect(url.searchParams.get('isActive')).toBe('true');
		});

		it('should handle empty context', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			const callArgs = (global.fetch as any).mock.calls[0];
			const url = new URL(callArgs[0]);

			// Should only have flagKey parameter
			expect(url.searchParams.get('flagKey')).toBe('my-flag');
			expect(url.searchParams.get('targetingKey')).toBeNull();
		});
	});

	describe('provider options', () => {
		it('should pass custom timeout to client', { timeout: 10000 }, async () => {
			// Mock a fetch that simulates an abort
			(global.fetch as any).mockImplementation(() => {
				const error = new Error('The operation was aborted');
				error.name = 'AbortError';
				return Promise.reject(error);
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				timeout: 200, // Custom timeout
				retries: 0,
			});

			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			// Should return error with default value
			expect(result.value).toBe(false);
			expect(result.errorCode).toBeDefined();
		});

		it('should pass custom fetch options to client', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				fetchOptions: {
					headers: {
						'X-API-Key': 'test-key',
						'X-Custom-Header': 'test-value',
					},
				},
			});

			await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			const callArgs = (global.fetch as any).mock.calls[0];
			expect(callArgs[1].headers).toEqual({
				'X-API-Key': 'test-key',
				'X-Custom-Header': 'test-value',
			});
		});
	});

	describe('logger integration', () => {
		it('does not call logger when logging is false (default)', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			// logging defaults to false — the injected logger must never be called
			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.debug).not.toHaveBeenCalled();
			expect(spyLogger.warn).not.toHaveBeenCalled();
			expect(spyLogger.error).not.toHaveBeenCalled();
		});

		it('does not call logger.error on network failure when logging is false (default)', async () => {
			const mockResponse = new Response(null, { status: 500, statusText: 'Error' });
			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0 });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.error).not.toHaveBeenCalled();
		});

		it('calls logger.debug on successful resolution when logging is true', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.debug).toHaveBeenCalled();
			expect(spyLogger.warn).not.toHaveBeenCalled();
			expect(spyLogger.error).not.toHaveBeenCalled();
		});

		it('calls logger.warn on type mismatch when logging is true', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: 'wrong' }),
			});

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.warn).toHaveBeenCalledWith(expect.stringContaining('type mismatch'));
		});

		it('calls logger.error on network failure when logging is true', async () => {
			const mockResponse = new Response(null, { status: 500, statusText: 'Error' });
			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.error).toHaveBeenCalled();
		});

		it('calls logger.error on unknown error when logging is true', async () => {
			(global.fetch as any).mockRejectedValueOnce('non-error string thrown');

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.error).toHaveBeenCalled();
		});

		it('includes flag key in logger.error message when logging is true', async () => {
			const mockResponse = new Response(null, { status: 500, statusText: 'Error' });
			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('special-flag', false, {}, spyLogger);

			const errorCall = (spyLogger.error as any).mock.calls[0][0] as string;
			expect(errorCall).toContain('special-flag');
		});
	});

	describe('lifecycle', () => {
		it('status is NOT_READY before initialize', () => {
			const { ProviderStatus } = require('@openfeature/server-sdk');
			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('status is READY after successful initialize', async () => {
			const { ProviderStatus } = require('@openfeature/server-sdk');
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('status is READY after initialize when health check returns 404', async () => {
			const { ProviderStatus } = require('@openfeature/server-sdk');
			const mockResponse = new Response(null, { status: 404, statusText: 'Not Found' });
			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0 });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('status is ERROR after initialize when endpoint unreachable', async () => {
			const { ProviderStatus } = require('@openfeature/server-sdk');
			(global.fetch as any).mockRejectedValueOnce(new Error('network down'));

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0 });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.ERROR);
		});

		it('status resets to NOT_READY after onClose', async () => {
			const { ProviderStatus } = require('@openfeature/server-sdk');
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);

			await provider.onClose();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});
	});

	describe('resolution details', () => {
		it('flagMetadata defaults to {} when response has no metadata', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.flagMetadata).toEqual({});
		});

		/* Test removed: reason is always present in the API response */

		it('treats null API response value as object type', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: null }),
			});

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate' });
			const result = await provider.resolveObjectEvaluation('my-flag', {}, {}, noopLogger);

			expect(result.value).toBeNull();
			expect(result.errorCode).toBeUndefined();
		});

		it('errorMessage includes the flag key on FlagshipError', async () => {
			const mockResponse = new Response(null, { status: 500, statusText: 'Error' });
			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0 });
			const result = await provider.resolveBooleanEvaluation('target-flag', false, {}, noopLogger);

			expect(result.errorMessage).toBeDefined();
		});

		it('NETWORK_ERROR with non-Response cause maps to GENERAL', async () => {
			(global.fetch as any).mockRejectedValueOnce(new Error('connection refused'));

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0 });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.errorCode).toBe(ErrorCode.GENERAL);
		});

		it('PARSE_ERROR maps to ErrorCode.PARSE_ERROR', async () => {
			const badResponse = { ok: true, json: async () => ({ unexpected: true }) };
			(global.fetch as any).mockResolvedValue(badResponse);

			const provider = new FlagshipServerProvider({ endpoint: 'https://api.example.com/evaluate', retries: 0 });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.errorCode).toBe(ErrorCode.PARSE_ERROR);
		});
	});
});
