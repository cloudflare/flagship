import type { Hook, HookContext, EvaluationDetails, FlagValue, HookHints, BeforeHookContext, ErrorCode } from '@openfeature/server-sdk';

/**
 * Telemetry event data
 */
export interface TelemetryEvent {
	type: 'evaluation' | 'error';
	flagKey: string;
	timestamp: number;
	duration?: number;
	value?: unknown;
	reason?: string;
	variant?: string;
	/** The OpenFeature ErrorCode, if evaluation produced an error. */
	errorCode?: ErrorCode;
	errorMessage?: string;
	context?: Record<string, unknown>;
	/**
	 * Caller-supplied hints forwarded from `EvaluationOptions.hookHints`.
	 * Use this to attach trace IDs, request IDs, or any other metadata
	 * that the application author wants to associate with the evaluation.
	 */
	hints?: Readonly<Record<string, unknown>>;
}

/**
 * Telemetry hook for tracking flag evaluations
 *
 * @example
 * ```typescript
 * import { OpenFeature } from '@openfeature/server-sdk';
 * import { FlagshipServerProvider, TelemetryHook } from '@cloudflare/flagship/server';
 *
 * const telemetryHook = new TelemetryHook((event) => {
 *   // Send to your analytics service
 *   analytics.track('flag_evaluated', event);
 * });
 *
 * OpenFeature.addHooks(telemetryHook);
 * ```
 */
export class TelemetryHook implements Hook {
	// Maps each evaluation's unique key to its start timestamp.
	// Keyed by a string stored in `contextKeys` to link the hookContext to its entry.
	private startTimes: Map<string, number> = new Map();

	// WeakMap<object> avoids the `as HookContext` cast: OpenFeature passes the same
	// object reference to all hook stages, so the lookup is always correct at runtime.
	private contextKeys: WeakMap<object, string> = new WeakMap();
	private hints: WeakMap<object, HookHints> = new WeakMap();

	private readonly onEvent: (event: TelemetryEvent) => void;

	constructor(onEvent: (event: TelemetryEvent) => void) {
		this.onEvent = onEvent;
	}

	before(hookContext: BeforeHookContext, hookHints?: HookHints): void {
		const now = Date.now();
		const key = `${hookContext.flagKey}-${now}-${Math.random()}`;
		this.startTimes.set(key, now);
		this.contextKeys.set(hookContext, key);
		// Store hints alongside the key so they can be forwarded in after/error.
		if (hookHints !== undefined) {
			this.hints.set(hookContext, hookHints);
		}
	}

	after(hookContext: Readonly<HookContext>, evaluationDetails: EvaluationDetails<FlagValue>, _hookHints?: HookHints): void {
		const telemetryKey = this.contextKeys.get(hookContext);
		const startTime = telemetryKey ? this.startTimes.get(telemetryKey) : undefined;
		const duration = startTime !== undefined ? Date.now() - startTime : undefined;

		if (telemetryKey !== undefined) {
			this.startTimes.delete(telemetryKey);
			this.contextKeys.delete(hookContext);
		}

		this.onEvent({
			type: 'evaluation',
			flagKey: hookContext.flagKey,
			timestamp: Date.now(),
			duration,
			value: evaluationDetails.value,
			reason: evaluationDetails.reason,
			variant: evaluationDetails.variant,
			errorCode: evaluationDetails.errorCode,
			context: hookContext.context,
			hints: this.hints.get(hookContext),
		});
	}

	error(hookContext: Readonly<HookContext>, error: unknown, _hookHints?: HookHints): void {
		const telemetryKey = this.contextKeys.get(hookContext);
		const startTime = telemetryKey ? this.startTimes.get(telemetryKey) : undefined;
		const duration = startTime !== undefined ? Date.now() - startTime : undefined;

		if (telemetryKey !== undefined) {
			this.startTimes.delete(telemetryKey);
			this.contextKeys.delete(hookContext);
		}

		const errorMessage = error instanceof Error ? error.message : String(error);
		this.onEvent({
			type: 'error',
			flagKey: hookContext.flagKey,
			timestamp: Date.now(),
			duration,
			errorMessage,
			context: hookContext.context,
			hints: this.hints.get(hookContext),
		});
	}

	finally(hookContext: Readonly<HookContext>, _evaluationDetails: EvaluationDetails<FlagValue>, _hookHints?: HookHints): void {
		const telemetryKey = this.contextKeys.get(hookContext);
		if (telemetryKey !== undefined) {
			this.startTimes.delete(telemetryKey);
			this.contextKeys.delete(hookContext);
		}
		this.hints.delete(hookContext);
	}
}
