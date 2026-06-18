import { describe, it, expect, vi } from 'vitest';
import type { HookContext, EvaluationDetails, FlagValue } from '@openfeature/server-sdk';
import { ErrorCode } from '@openfeature/server-sdk';
import { LoggingHook } from '../../src/hooks/logging-hook.js';

function makeHookContext(flagKey = 'test-flag'): HookContext {
	return {
		flagKey,
		defaultValue: false,
		flagValueType: 'boolean',
		context: { targetingKey: 'user-1' },
		clientMetadata: { name: 'test-client', providerMetadata: { name: 'test-provider' } },
		providerMetadata: { name: 'test-provider' },
		logger: { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} },
	} as unknown as HookContext;
}

function makeDetails(overrides: Partial<EvaluationDetails<FlagValue>> = {}): EvaluationDetails<FlagValue> {
	return {
		flagKey: 'test-flag',
		flagMetadata: {},
		value: true,
		reason: 'STATIC',
		variant: 'on',
		...overrides,
	};
}

describe('LoggingHook (unit)', () => {
	describe('before', () => {
		it('calls the logger with the flag key', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.before(makeHookContext('my-flag') as any);
			expect(logger).toHaveBeenCalledTimes(1);
			expect(logger.mock.calls[0][0]).toContain('my-flag');
		});

		it('passes defaultValue and context as second argument', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			const ctx = makeHookContext();
			hook.before(ctx as any);
			const secondArg = logger.mock.calls[0][1];
			expect(secondArg).toHaveProperty('defaultValue');
			expect(secondArg).toHaveProperty('context');
		});

		it('accepts optional hookHints without error', () => {
			const hook = new LoggingHook(() => {});
			expect(() => hook.before(makeHookContext() as any, { someHint: true })).not.toThrow();
		});
	});

	describe('after', () => {
		it('calls the logger with the flag key', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.after(makeHookContext('after-flag') as any, makeDetails());
			expect(logger.mock.calls[0][0]).toContain('after-flag');
		});

		it('passes value, reason, variant, errorCode in second argument', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.after(
				makeHookContext() as any,
				makeDetails({ value: 42, reason: 'TARGETING_MATCH', variant: 'v1', errorCode: ErrorCode.TYPE_MISMATCH }),
			);
			const secondArg = logger.mock.calls[0][1];
			expect(secondArg.value).toBe(42);
			expect(secondArg.reason).toBe('TARGETING_MATCH');
			expect(secondArg.variant).toBe('v1');
			expect(secondArg.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});

		it('accepts optional hookHints without error', () => {
			const hook = new LoggingHook(() => {});
			expect(() => hook.after(makeHookContext() as any, makeDetails(), { hint: 1 })).not.toThrow();
		});
	});

	describe('error', () => {
		it('extracts message from Error instance', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.error(makeHookContext('err-flag') as any, new Error('boom'));
			expect(logger.mock.calls[0][0]).toContain('err-flag');
			expect(logger.mock.calls[0][1]).toBe('boom');
		});

		it('stringifies non-Error thrown values', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.error(makeHookContext() as any, 'plain string error');
			expect(logger.mock.calls[0][1]).toBe('plain string error');
		});

		it('stringifies number thrown values', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.error(makeHookContext() as any, 42);
			expect(logger.mock.calls[0][1]).toBe('42');
		});

		it('accepts optional hookHints without error', () => {
			const hook = new LoggingHook(() => {});
			expect(() => hook.error(makeHookContext() as any, new Error('x'), { h: 1 })).not.toThrow();
		});
	});

	describe('finally', () => {
		it('is a no-op and does not call logger', () => {
			const logger = vi.fn();
			const hook = new LoggingHook(logger);
			hook.finally(makeHookContext() as any, makeDetails());
			expect(logger).not.toHaveBeenCalled();
		});

		it('does not throw', () => {
			const hook = new LoggingHook(() => {});
			expect(() => hook.finally(makeHookContext() as any, makeDetails(), { hint: 1 })).not.toThrow();
		});
	});

	describe('default logger', () => {
		it('uses console.log when no logger is provided', () => {
			const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => undefined);
			const hook = new LoggingHook();
			hook.before(makeHookContext() as any);
			expect(consoleSpy).toHaveBeenCalled();
			consoleSpy.mockRestore();
		});
	});
});
