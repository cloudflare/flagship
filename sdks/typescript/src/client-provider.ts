import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/web-sdk';
import { ErrorCode, OpenFeatureEventEmitter, ProviderEvents, ProviderFatalError, ProviderStatus } from '@openfeature/web-sdk';
import { FlagshipClient, isAuthFailure } from './client.js';
import { type FlagshipClientProviderOptions, type CachedFlag } from './types.js';

const ERROR_CODES = new Set<string>(Object.values(ErrorCode));

/**
 * OpenFeature provider for Flagship (client-side / browser).
 *
 * Evaluates every flag for the current context in a single OFREP bulk request
 * during initialization and on every context change, storing results in an
 * in-memory cache. All `resolve*` methods are synchronous, as required by the
 * OpenFeature web SDK.
 *
 * A flag that is missing from the response returns `ErrorCode.FLAG_NOT_FOUND`
 * with the default value.
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
	private readonly logging: boolean;
	private currentStatus: ProviderStatus = ProviderStatus.NOT_READY;
	private epoch = 0;

	constructor(options: FlagshipClientProviderOptions) {
		this.metadata = { name: 'Flagship Client Provider' };
		this.client = new FlagshipClient(resolveRelativeEndpoint(options));
		this.logging = options.logging ?? false;
	}

	get status(): ProviderStatus {
		return this.currentStatus;
	}

	async initialize(context: EvaluationContext = {}): Promise<void> {
		if (await this.load(context, 'initialization')) this.events.emit(ProviderEvents.Ready);
	}

	async onClose(): Promise<void> {
		this.epoch += 1;
		this.cache.clear();
		this.currentStatus = ProviderStatus.NOT_READY;
	}

	/**
	 * Invalidates the entire cache and re-evaluates every flag for the new
	 * context. Returning a Promise causes the SDK to automatically emit
	 * `ProviderEvents.Reconciling` while this method executes.
	 */
	async onContextChange(_oldContext: EvaluationContext, newContext: EvaluationContext = {}): Promise<void> {
		this.cache.clear();
		await this.load(newContext, 'context change');
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

	/** Resolves to false when `onClose()` or a newer load superseded this one. */
	private async load(context: EvaluationContext, phase: string): Promise<boolean> {
		const epoch = ++this.epoch;
		try {
			const flags = await this.fetchAll(context, phase);
			if (epoch !== this.epoch) return false;
			this.cache = flags;
			this.currentStatus = ProviderStatus.READY;
			return true;
		} catch (error) {
			if (epoch === this.epoch) {
				this.currentStatus = error instanceof ProviderFatalError ? ProviderStatus.FATAL : ProviderStatus.ERROR;
			}
			throw error;
		}
	}

	/**
	 * Evaluates every flag for `context`. Throws when the request fails, with
	 * `PROVIDER_FATAL` on a 401 or 403.
	 */
	private async fetchAll(context: EvaluationContext, phase: string): Promise<Map<string, CachedFlag>> {
		try {
			const flags = await this.client.evaluateAll(context);
			return new Map(flags.map(({ key, ...flag }) => [key, flag]));
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			if (this.logging) console.warn(`[Flagship] Failed to evaluate flags during ${phase}: ${message}`);
			throw isAuthFailure(error) ? new ProviderFatalError(message) : error;
		}
	}

	private resolveFromCache<T>(flagKey: string, defaultValue: T, expectedType: string, logger: Logger): ResolutionDetails<T> {
		const cached = this.cache.get(flagKey);

		if (!cached) {
			const msg = `Flag "${flagKey}" was not returned by the bulk evaluation`;
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

		const flagMetadata = cached.metadata ?? {};

		if (cached.errorCode) {
			const msg = cached.errorDetails ?? `Flag "${flagKey}" evaluation failed`;
			if (this.logging) {
				logger.warn(`[Flagship] ${msg}`);
			}
			return {
				value: defaultValue,
				reason: 'ERROR',
				errorCode: ERROR_CODES.has(cached.errorCode) ? (cached.errorCode as ErrorCode) : ErrorCode.GENERAL,
				errorMessage: msg,
			};
		}

		if (cached.reason === 'DISABLED') {
			return { value: defaultValue, reason: 'DISABLED', flagMetadata };
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
			reason: cached.reason,
			variant: cached.variant,
			flagMetadata,
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
