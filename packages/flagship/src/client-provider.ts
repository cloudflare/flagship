import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/web-sdk';
import { ErrorCode, OpenFeatureEventEmitter, ProviderEvents, ProviderStatus } from '@openfeature/web-sdk';
import { FlagshipClient } from './client.js';
import { type FlagshipClientProviderOptions, type CachedFlag } from './types.js';

/**
 * OpenFeature provider for Flagship (client-side / browser).
 *
 * Pre-fetches a configured set of flags when the evaluation context changes
 * and serves them synchronously from an in-memory cache, as required by the
 * OpenFeature web SDK.
 *
 * @example
 * ```typescript
 * import { OpenFeature } from '@openfeature/web-sdk';
 * import { FlagshipClientProvider } from '@cloudflare/flagship/web';
 *
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipClientProvider({
 *     appId: 'app-abc123',
 *     accountId: 'your-account-id',
 *     prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads'],
 *     cacheTTL: 60_000,
 *   })
 * );
 *
 * await OpenFeature.setContext({ targetingKey: 'user-123', plan: 'premium' });
 *
 * const client = OpenFeature.getClient();
 * const darkMode = client.getBooleanValue('dark-mode', false);
 * ```
 */
export class FlagshipClientProvider implements Provider {
	readonly metadata: ProviderMetadata;
	readonly runsOn = 'client' as const;
	readonly events = new OpenFeatureEventEmitter();

	private cache: Map<string, CachedFlag> = new Map();
	private client: FlagshipClient;
	private readonly prefetchFlags: string[];
	private readonly cacheTTL: number;
	private readonly logging: boolean;
	private currentStatus: ProviderStatus = ProviderStatus.NOT_READY;

	constructor(options: FlagshipClientProviderOptions) {
		this.metadata = { name: 'Flagship Client Provider' };
		this.client = new FlagshipClient(options);
		this.prefetchFlags = options.prefetchFlags || [];
		this.cacheTTL = options.cacheTTL ?? 0;
		this.logging = options.logging ?? false;
	}

	get status(): ProviderStatus {
		return this.currentStatus;
	}

	async initialize(context: EvaluationContext = {}): Promise<void> {
		if (this.prefetchFlags.length > 0) {
			const results = await Promise.allSettled(this.prefetchFlags.map((flagKey) => this.fetchAndCache(flagKey, context)));
			const failures = results.filter((r) => r.status === 'rejected');
			if (failures.length > 0 && this.logging) {
				console.warn(`[Flagship] ${failures.length} of ${this.prefetchFlags.length} flag(s) failed to pre-fetch during initialization.`);
			}
		}
		this.currentStatus = ProviderStatus.READY;
		this.events.emit(ProviderEvents.Ready);
	}

	async onClose(): Promise<void> {
		this.cache.clear();
		this.currentStatus = ProviderStatus.NOT_READY;
	}

	/**
	 * Returning a Promise causes the SDK to automatically emit
	 * `ProviderEvents.Reconciling` while this method is executing.
	 *
	 * Cache entries for all prefetchFlags are invalidated before fetching
	 * so a failed re-fetch yields DEFAULT rather than stale values from
	 * the previous context.
	 */
	async onContextChange(_oldContext: EvaluationContext, newContext: EvaluationContext = {}): Promise<void> {
		if (this.prefetchFlags.length > 0) {
			for (const flagKey of this.prefetchFlags) {
				this.cache.delete(flagKey);
			}
			const results = await Promise.allSettled(this.prefetchFlags.map((flagKey) => this.fetchAndCache(flagKey, newContext)));
			const failures = results.filter((r) => r.status === 'rejected');
			if (failures.length > 0 && this.logging) {
				console.warn(`[Flagship] ${failures.length} of ${this.prefetchFlags.length} flag(s) failed to re-fetch during context change.`);
			}
		}
	}

	resolveBooleanEvaluation(
		flagKey: string,
		defaultValue: boolean,
		_context: EvaluationContext,
		_logger: Logger,
	): ResolutionDetails<boolean> {
		return this.resolveFromCache(flagKey, defaultValue, 'boolean');
	}

	resolveStringEvaluation(flagKey: string, defaultValue: string, _context: EvaluationContext, _logger: Logger): ResolutionDetails<string> {
		return this.resolveFromCache(flagKey, defaultValue, 'string');
	}

	resolveNumberEvaluation(flagKey: string, defaultValue: number, _context: EvaluationContext, _logger: Logger): ResolutionDetails<number> {
		return this.resolveFromCache(flagKey, defaultValue, 'number');
	}

	resolveObjectEvaluation<T extends JsonValue>(
		flagKey: string,
		defaultValue: T,
		_context: EvaluationContext,
		_logger: Logger,
	): ResolutionDetails<T> {
		return this.resolveFromCache(flagKey, defaultValue, 'object');
	}

	private async fetchAndCache(flagKey: string, context: EvaluationContext): Promise<void> {
		const result = await this.client.evaluate(flagKey, context);
		this.cache.set(flagKey, {
			value: result.value,
			reason: result.reason,
			variant: result.variant,
			timestamp: Date.now(),
		});
	}

	private resolveFromCache<T>(flagKey: string, defaultValue: T, expectedType: string): ResolutionDetails<T> {
		const cached = this.cache.get(flagKey);

		if (!cached) {
			return { value: defaultValue, reason: 'DEFAULT' };
		}

		if (this.cacheTTL > 0 && Date.now() - cached.timestamp > this.cacheTTL) {
			this.cache.delete(flagKey);
			return { value: defaultValue, reason: 'DEFAULT' };
		}

		const actualType = this.getValueType(cached.value);
		if (actualType !== expectedType) {
			return {
				value: defaultValue,
				errorCode: ErrorCode.TYPE_MISMATCH,
				errorMessage: `Flag "${flagKey}" type mismatch: expected ${expectedType}, got ${actualType}`,
				reason: 'ERROR',
			};
		}

		return {
			value: cached.value as T,
			reason: 'CACHED',
			variant: cached.variant,
			flagMetadata: {},
		};
	}

	private getValueType(value: unknown): string {
		if (typeof value === 'boolean') return 'boolean';
		if (typeof value === 'string') return 'string';
		if (typeof value === 'number') return 'number';
		return 'object';
	}
}
