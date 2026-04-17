import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/web-sdk';
import { ErrorCode, OpenFeatureEventEmitter, ProviderEvents, ProviderStatus } from '@openfeature/web-sdk';
import { FlagshipClient } from './client.js';
import { type FlagshipClientProviderOptions, type CachedFlag } from './types.js';

/**
 * OpenFeature provider for Flagship (client-side / browser).
 *
 * Fetches all flags listed in `prefetchFlags` during initialization and on
 * every context change, storing results in an in-memory cache. All
 * `resolve*` methods are synchronous, as required by the OpenFeature web SDK.
 *
 * A cache miss (flag key not in `prefetchFlags`, or fetch failed) returns
 * `ErrorCode.FLAG_NOT_FOUND` with the default value.
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
 *     authToken: 'your-token',
 *     prefetchFlags: ['dark-mode', 'welcome-message'],
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
	private readonly logging: boolean;
	private currentStatus: ProviderStatus = ProviderStatus.NOT_READY;

	constructor(options: FlagshipClientProviderOptions) {
		this.metadata = { name: 'Flagship Client Provider' };
		this.client = new FlagshipClient(resolveRelativeEndpoint(options));
		this.prefetchFlags = options.prefetchFlags || [];
		this.logging = options.logging ?? false;
	}

	get status(): ProviderStatus {
		return this.currentStatus;
	}

	/**
	 * Fetches all `prefetchFlags` in parallel and populates the cache.
	 * Individual flag fetch failures are logged when `logging` is enabled but
	 * do not prevent the provider from reaching READY.
	 */
	async initialize(context: EvaluationContext = {}): Promise<void> {
		await this.fetchAll(context, 'initialization');
		this.currentStatus = ProviderStatus.READY;
		this.events.emit(ProviderEvents.Ready);
	}

	async onClose(): Promise<void> {
		this.cache.clear();
		this.currentStatus = ProviderStatus.NOT_READY;
	}

	/**
	 * Invalidates the entire cache and re-fetches all `prefetchFlags` for the
	 * new context. Returning a Promise causes the SDK to automatically emit
	 * `ProviderEvents.Reconciling` while this method executes.
	 */
	async onContextChange(_oldContext: EvaluationContext, newContext: EvaluationContext = {}): Promise<void> {
		this.cache.clear();
		await this.fetchAll(newContext, 'context change');
	}

	resolveBooleanEvaluation(
		flagKey: string,
		defaultValue: boolean,
		_context: EvaluationContext,
		logger: Logger,
	): ResolutionDetails<boolean> {
		return this.resolveFromCache(flagKey, defaultValue, 'boolean', logger);
	}

	resolveStringEvaluation(flagKey: string, defaultValue: string, _context: EvaluationContext, logger: Logger): ResolutionDetails<string> {
		return this.resolveFromCache(flagKey, defaultValue, 'string', logger);
	}

	resolveNumberEvaluation(flagKey: string, defaultValue: number, _context: EvaluationContext, logger: Logger): ResolutionDetails<number> {
		return this.resolveFromCache(flagKey, defaultValue, 'number', logger);
	}

	resolveObjectEvaluation<T extends JsonValue>(
		flagKey: string,
		defaultValue: T,
		_context: EvaluationContext,
		logger: Logger,
	): ResolutionDetails<T> {
		return this.resolveFromCache(flagKey, defaultValue, 'object', logger);
	}

	/**
	 * Fetches all `prefetchFlags` in parallel using `Promise.allSettled`.
	 * Failures are logged individually when `logging` is enabled.
	 */
	private async fetchAll(context: EvaluationContext, phase: string): Promise<void> {
		if (this.prefetchFlags.length === 0) return;

		const results = await Promise.allSettled(
			this.prefetchFlags.map(async (flagKey) => {
				const result = await this.client.evaluate(flagKey, context);
				this.cache.set(flagKey, {
					value: result.value,
					reason: result.reason,
					variant: result.variant,
				});
			}),
		);

		if (this.logging) {
			results.forEach((result, i) => {
				if (result.status === 'rejected') {
					const reason = result.reason instanceof Error ? result.reason.message : String(result.reason);
					console.warn(`[Flagship] Failed to fetch flag "${this.prefetchFlags[i]}" during ${phase}: ${reason}`);
				}
			});
		}
	}

	private resolveFromCache<T>(flagKey: string, defaultValue: T, expectedType: string, logger: Logger): ResolutionDetails<T> {
		const cached = this.cache.get(flagKey);

		if (!cached) {
			const msg = `Flag "${flagKey}" not found in cache. Add it to prefetchFlags to ensure it is fetched on initialization.`;
			if (this.logging) {
				logger.warn(`[Flagship] ${msg}`);
			}
			return {
				value: defaultValue,
				reason: 'ERROR',
				errorCode: ErrorCode.FLAG_NOT_FOUND,
				errorMessage: msg,
			};
		}

		const actualType = this.getValueType(cached.value);
		if (actualType !== expectedType) {
			const msg = `Flag "${flagKey}" type mismatch: expected ${expectedType}, got ${actualType}`;
			if (this.logging) {
				logger.warn(`[Flagship] ${msg}`);
			}
			return {
				value: defaultValue,
				errorCode: ErrorCode.TYPE_MISMATCH,
				errorMessage: msg,
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

function resolveRelativeEndpoint(options: FlagshipClientProviderOptions): FlagshipClientProviderOptions {
	const { endpoint } = options;
	if (!endpoint || !endpoint.startsWith('/')) return options;

	if (typeof window === 'undefined' || !window.location?.origin) {
		throw new Error(`Flagship: relative endpoint "${endpoint}" requires a browser context with window.location.origin`);
	}

	return { ...options, endpoint: `${window.location.origin}${endpoint}` };
}
