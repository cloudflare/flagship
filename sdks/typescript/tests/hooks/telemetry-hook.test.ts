import { describe, it, expect, vi } from 'vitest';
import type { HookContext, EvaluationDetails, FlagValue } from '@openfeature/server-sdk';
import { ErrorCode } from '@openfeature/server-sdk';
import { TelemetryHook, type TelemetryEvent } from '../../src/hooks/telemetry-hook.js';

function makeHookContext(flagKey = 'test-flag'): HookContext {
	return {
		flagKey,
		defaultValue: false,
		flagValueType: 'boolean',
		context: { targetingKey: 'user-1', plan: 'free' },
		clientMetadata: { name: 'client', providerMetadata: { name: 'provider' } },
		providerMetadata: { name: 'provider' },
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

describe('TelemetryHook (unit)', () => {
	describe('before + after (evaluation event)', () => {
		it('emits an evaluation event with correct fields', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext('my-flag');
			const details = makeDetails({ value: true, reason: 'TARGETING_MATCH', variant: 'on', errorCode: undefined });

			hook.before(ctx as any);
			hook.after(ctx as any, details);
			hook.finally(ctx as any, details);

			expect(events).toHaveLength(1);
			expect(events[0].type).toBe('evaluation');
			expect(events[0].flagKey).toBe('my-flag');
			expect(events[0].value).toBe(true);
			expect(events[0].reason).toBe('TARGETING_MATCH');
			expect(events[0].variant).toBe('on');
			expect(events[0].timestamp).toBeGreaterThan(0);
		});

		it('forwards hookHints to the evaluation event', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();
			const hints = { requestId: 'req-123', traceId: 'trace-abc' };

			hook.before(ctx as any, hints);
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect(events[0].hints).toEqual(hints);
		});

		it('hints is undefined when no hookHints were passed to before', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect(events[0].hints).toBeUndefined();
		});

		it('includes context in the evaluation event', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect(events[0].context).toEqual(ctx.context);
		});

		it('includes errorCode from evaluationDetails when present', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();
			const details = makeDetails({ errorCode: ErrorCode.TYPE_MISMATCH });

			hook.before(ctx as any);
			hook.after(ctx as any, details);
			hook.finally(ctx as any, details);

			expect(events[0].errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});

		it('duration is non-negative', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect(events[0].duration).toBeGreaterThanOrEqual(0);
		});

		it('duration increases with elapsed time', async () => {
			vi.useFakeTimers();
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			vi.advanceTimersByTime(100);
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect(events[0].duration).toBeGreaterThanOrEqual(100);
			vi.useRealTimers();
		});
	});

	describe('before + error (error event)', () => {
		it('emits an error event with errorMessage from Error instance', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext('err-flag');

			hook.before(ctx as any);
			hook.error(ctx as any, new Error('network down'));
			hook.finally(ctx as any, makeDetails());

			expect(events).toHaveLength(1);
			expect(events[0].type).toBe('error');
			expect(events[0].flagKey).toBe('err-flag');
			expect(events[0].errorMessage).toBe('network down');
		});

		it('stringifies non-Error thrown values', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.error(ctx as any, 'some string error');
			hook.finally(ctx as any, makeDetails());

			expect(events[0].errorMessage).toBe('some string error');
		});

		it('forwards hookHints to the error event', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();
			const hints = { requestId: 'req-456' };

			hook.before(ctx as any, hints);
			hook.error(ctx as any, new Error('boom'));
			hook.finally(ctx as any, makeDetails());

			expect(events[0].hints).toEqual(hints);
		});

		it('includes context in the error event', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.error(ctx as any, new Error('x'));
			hook.finally(ctx as any, makeDetails());

			expect(events[0].context).toEqual(ctx.context);
		});

		it('error event has no errorCode field', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.error(ctx as any, new Error('x'));
			hook.finally(ctx as any, makeDetails());

			expect(events[0].errorCode).toBeUndefined();
		});
	});

	describe('finally — cleanup', () => {
		it('cleans up startTimes and contextKeys after after', () => {
			const hook = new TelemetryHook(() => {});
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			const startTimesSize = (hook as any).startTimes.size;
			expect(startTimesSize).toBe(0);
		});

		it('cleans up startTimes and contextKeys after error', () => {
			const hook = new TelemetryHook(() => {});
			const ctx = makeHookContext();

			hook.before(ctx as any);
			hook.error(ctx as any, new Error('x'));
			hook.finally(ctx as any, makeDetails());

			expect((hook as any).startTimes.size).toBe(0);
		});

		it('cleans up hints WeakMap after finally', () => {
			const hook = new TelemetryHook(() => {});
			const ctx = makeHookContext();

			hook.before(ctx as any, { traceId: 'abc' });
			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect((hook as any).hints.has(ctx)).toBe(false);
		});

		it('does not throw when finally called without preceding before', () => {
			const hook = new TelemetryHook(() => {});
			expect(() => hook.finally(makeHookContext() as any, makeDetails())).not.toThrow();
		});
	});

	describe('after without preceding before', () => {
		it('still emits event with undefined duration', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.after(ctx as any, makeDetails());
			hook.finally(ctx as any, makeDetails());

			expect(events).toHaveLength(1);
			expect(events[0].duration).toBeUndefined();
		});
	});

	describe('error without preceding before', () => {
		it('still emits event with undefined duration', () => {
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx = makeHookContext();

			hook.error(ctx as any, new Error('x'));
			hook.finally(ctx as any, makeDetails());

			expect(events).toHaveLength(1);
			expect(events[0].duration).toBeUndefined();
		});
	});

	describe('multiple concurrent evaluations', () => {
		it('tracks timing independently per hookContext object', () => {
			vi.useFakeTimers();
			const events: TelemetryEvent[] = [];
			const hook = new TelemetryHook((e) => events.push(e));
			const ctx1 = makeHookContext('flag-1');
			const ctx2 = makeHookContext('flag-2');

			hook.before(ctx1 as any);
			vi.advanceTimersByTime(50);
			hook.before(ctx2 as any);
			vi.advanceTimersByTime(50);

			hook.after(ctx1 as any, makeDetails({ flagKey: 'flag-1' }));
			hook.finally(ctx1 as any, makeDetails());
			hook.after(ctx2 as any, makeDetails({ flagKey: 'flag-2' }));
			hook.finally(ctx2 as any, makeDetails());

			expect(events).toHaveLength(2);
			expect(events[0].flagKey).toBe('flag-1');
			expect(events[1].flagKey).toBe('flag-2');
			expect(events[0].duration).toBeGreaterThanOrEqual(100);
			expect(events[1].duration).toBeGreaterThanOrEqual(50);
			expect(events[1].duration!).toBeLessThan(events[0].duration!);

			vi.useRealTimers();
		});

		it('unique keys per before call prevent collision', () => {
			const hook = new TelemetryHook(() => {});
			const ctx1 = makeHookContext('flag');
			const ctx2 = makeHookContext('flag');

			hook.before(ctx1 as any);
			hook.before(ctx2 as any);

			expect((hook as any).startTimes.size).toBe(2);
		});
	});

	describe('hookHints', () => {
		it('before accepts hints without error', () => {
			const hook = new TelemetryHook(() => {});
			expect(() => hook.before(makeHookContext() as any, { hint: 1 })).not.toThrow();
		});

		it('after accepts hints without error', () => {
			const hook = new TelemetryHook(() => {});
			expect(() => hook.after(makeHookContext() as any, makeDetails(), { hint: 1 })).not.toThrow();
		});

		it('error accepts hints without error', () => {
			const hook = new TelemetryHook(() => {});
			expect(() => hook.error(makeHookContext() as any, new Error('x'), { hint: 1 })).not.toThrow();
		});

		it('finally accepts hints without error', () => {
			const hook = new TelemetryHook(() => {});
			expect(() => hook.finally(makeHookContext() as any, makeDetails(), { hint: 1 })).not.toThrow();
		});
	});
});
