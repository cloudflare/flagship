import type { Hook, HookContext, EvaluationDetails, FlagValue, HookHints, BeforeHookContext } from '@openfeature/server-sdk';

/**
 * Logging hook for debugging flag evaluations
 *
 * @example
 * ```typescript
 * import { OpenFeature } from '@openfeature/server-sdk';
 * import { FlagshipServerProvider, LoggingHook } from '@cloudflare/flagship/server';
 *
 * const provider = new FlagshipServerProvider({ appId: 'your-app-id', accountId: 'your-account-id' });
 * await OpenFeature.setProviderAndWait(provider);
 *
 * // Add logging hook
 * OpenFeature.addHooks(new LoggingHook());
 * ```
 */
export class LoggingHook implements Hook {
	private readonly logger: (message: string, ...args: unknown[]) => void;

	constructor(logger: (message: string, ...args: unknown[]) => void = console.log) {
		this.logger = logger;
	}

	before(hookContext: BeforeHookContext, _hookHints?: HookHints): void {
		this.logger(`[Flagship] Evaluating flag: ${hookContext.flagKey}`, {
			defaultValue: hookContext.defaultValue,
			context: hookContext.context,
		});
	}

	after(hookContext: Readonly<HookContext>, evaluationDetails: EvaluationDetails<FlagValue>, _hookHints?: HookHints): void {
		this.logger(`[Flagship] Flag ${hookContext.flagKey} evaluated`, {
			value: evaluationDetails.value,
			reason: evaluationDetails.reason,
			variant: evaluationDetails.variant,
			errorCode: evaluationDetails.errorCode,
		});
	}

	error(hookContext: Readonly<HookContext>, error: unknown, _hookHints?: HookHints): void {
		const message = error instanceof Error ? error.message : String(error);
		this.logger(`[Flagship] Error evaluating flag ${hookContext.flagKey}:`, message);
	}

	finally(_hookContext: Readonly<HookContext>, _evaluationDetails: EvaluationDetails<FlagValue>, _hookHints?: HookHints): void {
		// No-op
	}
}
