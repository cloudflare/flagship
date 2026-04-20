import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { Logger } from '@openfeature/server-sdk';
import { ErrorCode, ProviderStatus, ProviderEvents, OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';
import type { FlagshipBinding, FlagshipBindingEvaluationDetails } from '../src/types.js';

/**
 * Creates a mock FlagshipBinding where every method is a vi.fn() that can be
 * configured per-test. By default, all *Value methods return the defaultValue
 * and all *Details methods return a DEFAULT-reason response.
 */
function createMockBinding(): FlagshipBinding {
	return {
		get: vi.fn((_flagKey: string, defaultValue?: unknown) => Promise.resolve(defaultValue)),
		getBooleanValue: vi.fn((_flagKey: string, defaultValue: boolean) => Promise.resolve(defaultValue)),
		getStringValue: vi.fn((_flagKey: string, defaultValue: string) => Promise.resolve(defaultValue)),
		getNumberValue: vi.fn((_flagKey: string, defaultValue: number) => Promise.resolve(defaultValue)),
		getObjectValue: vi.fn(<T extends object>(_flagKey: string, defaultValue: T) => Promise.resolve(defaultValue)),
		getBooleanDetails: vi.fn(
			(flagKey: string, defaultValue: boolean): Promise<FlagshipBindingEvaluationDetails<boolean>> =>
				Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
		getStringDetails: vi.fn(
			(flagKey: string, defaultValue: string): Promise<FlagshipBindingEvaluationDetails<string>> =>
				Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
		getNumberDetails: vi.fn(
			(flagKey: string, defaultValue: number): Promise<FlagshipBindingEvaluationDetails<number>> =>
				Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
		getObjectDetails: vi.fn(
			<T extends object>(flagKey: string, defaultValue: T): Promise<FlagshipBindingEvaluationDetails<T>> =>
				Promise.resolve({ flagKey, value: defaultValue, reason: 'DEFAULT' }),
		),
	};
}

// Minimal logger stub satisfying the OpenFeature Logger interface
const noopLogger: Logger = {
	debug: () => {},
	info: () => {},
	warn: () => {},
	error: () => {},
};

describe('FlagshipServerProvider (binding mode)', () => {
	beforeEach(() => {
		vi.clearAllMocks();
	});

	// -----------------------------------------------------------------------
	// Constructor validation
	// -----------------------------------------------------------------------

	describe('constructor', () => {
		it('should create provider with a binding', () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });

			expect(provider).toBeInstanceOf(FlagshipServerProvider);
			expect(provider.metadata.name).toBe('Flagship Server Provider');
			expect(provider.runsOn).toBe('server');
		});

		it('should accept binding + logging option', () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding, logging: true });

			expect(provider).toBeInstanceOf(FlagshipServerProvider);
		});

		it('should throw when binding and appId are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						appId: 'app-123',
					} as any),
			).toThrow('must not be provided');
		});

		it('should throw when binding and endpoint are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						endpoint: 'https://api.example.com/evaluate',
					} as any),
			).toThrow('must not be provided');
		});

		it('should throw when binding and authToken are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						authToken: 'my-token',
					} as any),
			).toThrow('must not be provided');
		});

		it('should throw when binding and accountId are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						accountId: 'acc-123',
					} as any),
			).toThrow('must not be provided');
		});

		it('should throw when binding and fetchOptions are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						fetchOptions: { headers: {} },
					} as any),
			).toThrow('must not be provided');
		});

		it('should throw when binding and timeout are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						timeout: 5000,
					} as any),
			).toThrow('must not be provided');
		});

		it('should throw when binding and multiple HTTP fields are both provided', () => {
			const binding = createMockBinding();
			expect(
				() =>
					new FlagshipServerProvider({
						binding,
						appId: 'app-123',
						accountId: 'acc-123',
						authToken: 'tok',
					} as any),
			).toThrow('appId');
		});

		it('error message lists all conflicting fields', () => {
			const binding = createMockBinding();
			try {
				new FlagshipServerProvider({
					binding,
					appId: 'app-123',
					retries: 3,
				} as any);
				expect.fail('should have thrown');
			} catch (e: any) {
				expect(e.message).toContain('appId');
				expect(e.message).toContain('retries');
			}
		});
	});

	// -----------------------------------------------------------------------
	// Lifecycle
	// -----------------------------------------------------------------------

	describe('lifecycle', () => {
		it('status is NOT_READY before initialize', () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('status is READY immediately after initialize (no health check)', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('does not call any binding methods during initialize', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			await provider.initialize();

			expect(binding.get).not.toHaveBeenCalled();
			expect(binding.getBooleanDetails).not.toHaveBeenCalled();
			expect(binding.getStringDetails).not.toHaveBeenCalled();
			expect(binding.getNumberDetails).not.toHaveBeenCalled();
			expect(binding.getObjectDetails).not.toHaveBeenCalled();
		});

		it('emits READY event on initialize', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			const readyHandler = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, readyHandler);

			await provider.initialize();

			expect(readyHandler).toHaveBeenCalled();
		});

		it('status resets to NOT_READY after onClose', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			await provider.initialize();
			expect(provider.status).toBe(ProviderStatus.READY);

			await provider.onClose();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});
	});

	// -----------------------------------------------------------------------
	// Boolean evaluation
	// -----------------------------------------------------------------------

	describe('resolveBooleanEvaluation', () => {
		it('should resolve boolean flag via binding', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: true,
				variant: 'on',
				reason: 'TARGETING_MATCH',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, { targetingKey: 'user-1' }, noopLogger);

			expect(result.value).toBe(true);
			expect(result.variant).toBe('on');
			expect(result.reason).toBe('TARGETING_MATCH');
			expect(result.errorCode).toBeUndefined();
			expect(result.flagMetadata).toEqual({});
		});

		it('should return default value when binding returns type mismatch', async () => {
			const binding = createMockBinding();
			// Binding returns a string instead of boolean
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: 'not-a-boolean',
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected boolean, got string');
			expect(result.reason).toBe('ERROR');
		});

		it('should pass context to binding.getBooleanDetails', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });

			await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{ targetingKey: 'user-123', plan: 'premium', age: 30, isActive: true },
				noopLogger,
			);

			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {
				targetingKey: 'user-123',
				plan: 'premium',
				age: 30,
				isActive: true,
			});
		});
	});

	// -----------------------------------------------------------------------
	// String evaluation
	// -----------------------------------------------------------------------

	describe('resolveStringEvaluation', () => {
		it('should resolve string flag via binding', async () => {
			const binding = createMockBinding();
			(binding.getStringDetails as any).mockResolvedValueOnce({
				flagKey: 'variant',
				value: 'treatment-b',
				variant: 'v2',
				reason: 'SPLIT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveStringEvaluation('variant', 'control', {}, noopLogger);

			expect(result.value).toBe('treatment-b');
			expect(result.variant).toBe('v2');
			expect(result.reason).toBe('SPLIT');
		});

		it('should return default on type mismatch', async () => {
			const binding = createMockBinding();
			(binding.getStringDetails as any).mockResolvedValueOnce({
				flagKey: 'variant',
				value: 42,
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveStringEvaluation('variant', 'control', {}, noopLogger);

			expect(result.value).toBe('control');
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});
	});

	// -----------------------------------------------------------------------
	// Number evaluation
	// -----------------------------------------------------------------------

	describe('resolveNumberEvaluation', () => {
		it('should resolve number flag via binding', async () => {
			const binding = createMockBinding();
			(binding.getNumberDetails as any).mockResolvedValueOnce({
				flagKey: 'rate-limit',
				value: 200,
				reason: 'TARGETING_MATCH',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveNumberEvaluation('rate-limit', 100, {}, noopLogger);

			expect(result.value).toBe(200);
			expect(result.reason).toBe('TARGETING_MATCH');
		});

		it('should return default on type mismatch', async () => {
			const binding = createMockBinding();
			(binding.getNumberDetails as any).mockResolvedValueOnce({
				flagKey: 'rate-limit',
				value: true,
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveNumberEvaluation('rate-limit', 100, {}, noopLogger);

			expect(result.value).toBe(100);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
			expect(result.errorMessage).toContain('expected number, got boolean');
		});
	});

	// -----------------------------------------------------------------------
	// Object evaluation
	// -----------------------------------------------------------------------

	describe('resolveObjectEvaluation', () => {
		it('should resolve object flag via binding', async () => {
			const objectValue = { theme: 'dark', beta: true };
			const binding = createMockBinding();
			(binding.getObjectDetails as any).mockResolvedValueOnce({
				flagKey: 'config',
				value: objectValue,
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveObjectEvaluation('config', {}, {}, noopLogger);

			expect(result.value).toEqual(objectValue);
			expect(result.reason).toBe('DEFAULT');
			expect(result.flagMetadata).toEqual({});
		});

		it('should handle null value as object type', async () => {
			const binding = createMockBinding();
			(binding.getObjectDetails as any).mockResolvedValueOnce({
				flagKey: 'nullable',
				value: null,
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveObjectEvaluation('nullable', {}, {}, noopLogger);

			expect(result.value).toBeNull();
			expect(result.errorCode).toBeUndefined();
		});

		it('should return default on type mismatch (string instead of object)', async () => {
			const binding = createMockBinding();
			(binding.getObjectDetails as any).mockResolvedValueOnce({
				flagKey: 'config',
				value: 'not-an-object',
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveObjectEvaluation('config', { fallback: true }, {}, noopLogger);

			expect(result.value).toEqual({ fallback: true });
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});
	});

	// -----------------------------------------------------------------------
	// Error handling from binding
	// -----------------------------------------------------------------------

	describe('error handling', () => {
		it('should map FLAG_NOT_FOUND errorCode from binding', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'missing-flag',
				value: false,
				errorCode: 'FLAG_NOT_FOUND',
				errorMessage: 'Flag not found',
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('missing-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
			expect(result.errorMessage).toBe('Flag not found');
		});

		it('should map PARSE_ERROR errorCode from binding', async () => {
			const binding = createMockBinding();
			(binding.getStringDetails as any).mockResolvedValueOnce({
				flagKey: 'bad-flag',
				value: 'default',
				errorCode: 'PARSE_ERROR',
				errorMessage: 'Could not parse flag value',
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveStringEvaluation('bad-flag', 'default', {}, noopLogger);

			expect(result.value).toBe('default');
			expect(result.errorCode).toBe(ErrorCode.PARSE_ERROR);
		});

		it('should map TYPE_MISMATCH errorCode from binding', async () => {
			const binding = createMockBinding();
			(binding.getNumberDetails as any).mockResolvedValueOnce({
				flagKey: 'typed-flag',
				value: 0,
				errorCode: 'TYPE_MISMATCH',
				errorMessage: 'Expected number',
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveNumberEvaluation('typed-flag', 0, {}, noopLogger);

			expect(result.value).toBe(0);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});

		it('should map INVALID_CONTEXT errorCode from binding', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'ctx-flag',
				value: false,
				errorCode: 'INVALID_CONTEXT',
				errorMessage: 'Bad context',
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('ctx-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.INVALID_CONTEXT);
		});

		it('should map unknown errorCode from binding to GENERAL', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'unknown-err',
				value: false,
				errorCode: 'SOMETHING_WEIRD',
				errorMessage: 'Unexpected',
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('unknown-err', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.GENERAL);
		});

		it('should handle binding method throwing an error', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockRejectedValueOnce(new Error('RPC connection lost'));

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false);
			expect(result.errorCode).toBe(ErrorCode.GENERAL);
			expect(result.errorMessage).toContain('RPC connection lost');
			expect(result.reason).toBe('ERROR');
		});

		it('should handle binding method throwing a non-Error value', async () => {
			const binding = createMockBinding();
			(binding.getStringDetails as any).mockRejectedValueOnce('raw string error');

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveStringEvaluation('my-flag', 'default', {}, noopLogger);

			expect(result.value).toBe('default');
			expect(result.errorCode).toBe(ErrorCode.GENERAL);
			expect(result.errorMessage).toContain('raw string error');
		});

		it('should provide synthetic errorMessage when binding errorCode present but no errorMessage', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: false,
				errorCode: 'FLAG_NOT_FOUND',
				// no errorMessage
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
			expect(result.errorMessage).toContain('FLAG_NOT_FOUND');
		});
	});

	// -----------------------------------------------------------------------
	// DISABLED flag — falls back to SDK default
	// -----------------------------------------------------------------------

	describe('DISABLED flag — falls back to SDK default', () => {
		it('returns SDK defaultValue (not the flag variation) for a boolean flag', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: true, // flag's stored default variation — should NOT be used
				variant: 'on',
				reason: 'DISABLED',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(result.value).toBe(false); // SDK caller's default
			expect(result.reason).toBe('DISABLED');
			expect(result.errorCode).toBeUndefined();
			expect(result.variant).toBeUndefined();
		});

		it('returns SDK defaultValue for a string flag', async () => {
			const binding = createMockBinding();
			(binding.getStringDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: 'flag-default',
				variant: 'flag-variant',
				reason: 'DISABLED',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveStringEvaluation('my-flag', 'sdk-default', {}, noopLogger);

			expect(result.value).toBe('sdk-default');
			expect(result.reason).toBe('DISABLED');
			expect(result.errorCode).toBeUndefined();
		});

		it('returns SDK defaultValue for a number flag', async () => {
			const binding = createMockBinding();
			(binding.getNumberDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: 99,
				variant: 'high',
				reason: 'DISABLED',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveNumberEvaluation('my-flag', 0, {}, noopLogger);

			expect(result.value).toBe(0);
			expect(result.reason).toBe('DISABLED');
			expect(result.errorCode).toBeUndefined();
		});

		it('returns SDK defaultValue for an object flag', async () => {
			const binding = createMockBinding();
			(binding.getObjectDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: { stored: true },
				variant: 'stored-variant',
				reason: 'DISABLED',
			});

			const provider = new FlagshipServerProvider({ binding });
			const result = await provider.resolveObjectEvaluation('my-flag', { sdk: true }, {}, noopLogger);

			expect(result.value).toEqual({ sdk: true });
			expect(result.reason).toBe('DISABLED');
			expect(result.errorCode).toBeUndefined();
		});
	});

	// -----------------------------------------------------------------------
	// Context conversion
	// -----------------------------------------------------------------------

	describe('context conversion', () => {
		it('should pass primitive context values through', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });

			await provider.resolveBooleanEvaluation('my-flag', false, { targetingKey: 'user-1', age: 25, premium: true }, noopLogger);

			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {
				targetingKey: 'user-1',
				age: 25,
				premium: true,
			});
		});

		it('should convert Date values to ISO strings', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			const date = new Date('2025-06-15T10:30:00.000Z');

			await provider.resolveBooleanEvaluation('my-flag', false, { targetingKey: 'user-1', createdAt: date as any }, noopLogger);

			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {
				targetingKey: 'user-1',
				createdAt: '2025-06-15T10:30:00.000Z',
			});
		});

		it('should skip null and undefined values', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });

			await provider.resolveBooleanEvaluation(
				'my-flag',
				false,
				{ targetingKey: 'user-1', nullVal: null as any, undefVal: undefined as any },
				noopLogger,
			);

			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {
				targetingKey: 'user-1',
			});
		});

		it('should drop complex objects and warn when logging is enabled', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, { targetingKey: 'user-1', nested: { foo: 'bar' } as any }, spyLogger);

			// The nested key should be excluded from the binding call
			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {
				targetingKey: 'user-1',
			});
			// A warning should have been emitted
			expect(spyLogger.warn).toHaveBeenCalledWith(expect.stringContaining('nested'));
		});

		it('should drop arrays and warn when logging is enabled', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, { targetingKey: 'user-1', tags: ['a', 'b'] as any }, spyLogger);

			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {
				targetingKey: 'user-1',
			});
			expect(spyLogger.warn).toHaveBeenCalledWith(expect.stringContaining('tags'));
		});

		it('should NOT warn about dropped keys when logging is disabled', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding, logging: false });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, { targetingKey: 'user-1', nested: { foo: 'bar' } as any }, spyLogger);

			expect(spyLogger.warn).not.toHaveBeenCalled();
		});

		it('should handle empty context', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });

			await provider.resolveBooleanEvaluation('my-flag', false, {}, noopLogger);

			expect(binding.getBooleanDetails).toHaveBeenCalledWith('my-flag', false, {});
		});
	});

	// -----------------------------------------------------------------------
	// Logger integration
	// -----------------------------------------------------------------------

	describe('logger integration', () => {
		it('does not call logger when logging is false (default)', async () => {
			const binding = createMockBinding();
			const provider = new FlagshipServerProvider({ binding });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.debug).not.toHaveBeenCalled();
			expect(spyLogger.warn).not.toHaveBeenCalled();
			expect(spyLogger.error).not.toHaveBeenCalled();
		});

		it('calls logger.debug on successful resolution when logging is true', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: true,
				reason: 'DEFAULT',
			});

			const provider = new FlagshipServerProvider({ binding, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.debug).toHaveBeenCalled();
		});

		it('calls logger.error when binding returns errorCode and logging is true', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValueOnce({
				flagKey: 'my-flag',
				value: false,
				errorCode: 'FLAG_NOT_FOUND',
				errorMessage: 'Not found',
				reason: 'ERROR',
			});

			const provider = new FlagshipServerProvider({ binding, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.error).toHaveBeenCalledWith(expect.stringContaining('my-flag'));
		});

		it('calls logger.error when binding throws and logging is true', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockRejectedValueOnce(new Error('boom'));

			const provider = new FlagshipServerProvider({ binding, logging: true });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.error).toHaveBeenCalledWith(expect.stringContaining('boom'));
		});

		it('does not call logger.error when binding throws and logging is false', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockRejectedValueOnce(new Error('boom'));

			const provider = new FlagshipServerProvider({ binding, logging: false });
			const spyLogger: Logger = { debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() };

			await provider.resolveBooleanEvaluation('my-flag', false, {}, spyLogger);

			expect(spyLogger.error).not.toHaveBeenCalled();
		});
	});

	// -----------------------------------------------------------------------
	// Integration with OpenFeature
	// -----------------------------------------------------------------------

	describe('integration with OpenFeature', () => {
		beforeEach(() => {
			OpenFeature.clearProviders();
		});

		it('should work end-to-end with OpenFeature SDK', async () => {
			const binding = createMockBinding();
			(binding.getBooleanDetails as any).mockResolvedValue({
				flagKey: 'dark-mode',
				value: true,
				variant: 'enabled',
				reason: 'TARGETING_MATCH',
			});

			const provider = new FlagshipServerProvider({ binding });
			await OpenFeature.setProviderAndWait(provider);

			const client = OpenFeature.getClient();
			const value = await client.getBooleanValue('dark-mode', false);

			expect(value).toBe(true);
		});

		it('should emit READY event via OpenFeature', async () => {
			const readyHandler = vi.fn();
			OpenFeature.addHandler(ProviderEvents.Ready, readyHandler);

			const binding = createMockBinding();
			await OpenFeature.setProviderAndWait(new FlagshipServerProvider({ binding }));

			expect(readyHandler).toHaveBeenCalled();
		});
	});
});
