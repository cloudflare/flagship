import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/server-sdk';
import { ErrorCode, OpenFeatureEventEmitter, ProviderEvents, ProviderFatalError } from '@openfeature/server-sdk';
import { LRUCache } from 'lru-cache';
import { FlagshipClient, isAuthFailure, resolveDefinitionsEndpoint } from './client.js';
import { normalizeEvaluationContext, type NormalizedContextValue } from './context.js';
import { evaluateFlag, FlagConfigError, type EvalFlag } from './local/evaluate.js';
import {
	FlagshipError,
	FlagshipErrorCode,
	isBindingOptions,
	type FlagshipBinding,
	type FlagshipBindingEvaluationDetails,
	type FlagshipServerProviderOptions,
} from './types.js';

const DEFAULT_CACHE_MAX_SIZE = 1000;
const DEFAULT_REFRESH_INTERVAL_MS = 30_000;
type ExpectedType = 'boolean' | 'string' | 'number' | 'object';

// Shared no-op used to build a silent logger when logging is false.
const _noop = (): void => {};

/** HTTP-specific fields that must NOT be present alongside `binding`. */
const HTTP_ONLY_FIELDS = [
	'appId',
	'endpoint',
	'accountId',
	'authToken',
	'baseUrl',
	'fetchOptions',
	'fetch',
	'timeout',
	'retries',
	'retryDelay',
	'localEvaluation',
	'refreshInterval',
] as const;

/**
 * OpenFeature provider for Flagship (server-side / dynamic context).
 *
 * Supports three modes of operation:
 *
 * **HTTP mode** — evaluates flags via HTTP requests to the Flagship API.
 * Requires `appId`/`endpoint`, `accountId`, and optionally `authToken`.
 *
 * **Local evaluation mode** — downloads flag definitions once, evaluates
 * in-process, and refreshes definitions lazily. Requires `localEvaluation: true`
 * plus HTTP config and a token with app **read** permission.
 *
 * **Binding mode** — evaluates flags via a Cloudflare Workers wrangler binding.
 * Only requires the `binding` field (the `Flagship` object from `env`). No HTTP
 * overhead, no auth tokens. This is the recommended approach for Workers.
 *
 * @example HTTP mode
 * ```typescript
 * import { OpenFeature } from '@openfeature/server-sdk';
 * import { FlagshipServerProvider } from '@cloudflare/flagship/server';
 *
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipServerProvider({
 *     appId: 'app-abc123',
 *     accountId: 'your-account-id',
 *     authToken: 'your-token',
 *   })
 * );
 * ```
 *
 * @example Local evaluation mode
 * ```typescript
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipServerProvider({
 *     appId: 'app-abc123',
 *     accountId: 'your-account-id',
 *     authToken: 'your-read-token',
 *     localEvaluation: true,
 *   })
 * );
 * ```
 *
 * @example Binding mode (Cloudflare Workers)
 * ```typescript
 * import { OpenFeature } from '@openfeature/server-sdk';
 * import { FlagshipServerProvider } from '@cloudflare/flagship/server';
 *
 * export default {
 *   async fetch(request: Request, env: { FLAGS: FlagshipBinding }) {
 *     await OpenFeature.setProviderAndWait(
 *       new FlagshipServerProvider({ binding: env.FLAGS })
 *     );
 *     const client = OpenFeature.getClient();
 *     const value = await client.getBooleanValue('my-flag', false);
 *     return new Response(JSON.stringify({ value }));
 *   },
 * };
 * ```
 */
export class FlagshipServerProvider implements Provider {
	readonly metadata: ProviderMetadata;
	readonly runsOn = 'server' as const;
	readonly events = new OpenFeatureEventEmitter();

	/** Set when operating in HTTP or local mode; `undefined` in binding mode. */
	private readonly client: FlagshipClient | undefined;
	/** Set when operating in binding mode; `undefined` in HTTP/local mode. */
	private readonly binding: FlagshipBinding | undefined;
	private readonly logging: boolean;
	private readonly localEvaluation: boolean;
	private readonly refreshInterval: number;
	private readonly accountId: string | undefined;

	/** TTL + LRU response cache; `undefined` when caching is disabled. */
	private readonly cache: LRUCache<string, ResolutionDetails<unknown>> | undefined;

	/** Immutable definitions snapshot swapped atomically on refresh. */
	private snapshot: Record<string, EvalFlag> | undefined;
	private etag: string | undefined;
	private lastFetch = 0;
	private refreshInFlight: Promise<void> | undefined;
	/**
	 * Generation counter for the local-evaluation lifecycle. Bumped on every
	 * `initialize()` and `onClose()` so in-flight fetches from a previous
	 * session cannot apply their result after close or re-init. Paired with
	 * `localReady` to distinguish an active session from a closed one without
	 * relying on a boolean that re-init would flip back too early for stale writes.
	 */
	private epoch = 0;
	/** True only between a successful local `initialize()` and `onClose()`. */
	private localReady = false;
	private stale = false;

	private readonly resolve: <T>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		expectedType: ExpectedType,
		logger: Logger,
	) => Promise<ResolutionDetails<T>>;

	constructor(options: FlagshipServerProviderOptions) {
		this.metadata = { name: 'Flagship Server Provider' };
		this.logging = options.logging ?? false;
		this.localEvaluation = options.localEvaluation === true;
		this.refreshInterval = options.refreshInterval ?? DEFAULT_REFRESH_INTERVAL_MS;

		if (this.localEvaluation && (!Number.isFinite(this.refreshInterval) || this.refreshInterval <= 0)) {
			throw new Error('Flagship: "refreshInterval" must be a finite number of milliseconds greater than 0');
		}

		if (options.cacheTtl !== undefined && options.cacheTtl > 0) {
			if (this.localEvaluation) {
				throw new Error('Flagship: localEvaluation is incompatible with cacheTtl — local evaluation has nothing to cache');
			}
			this.cache = new LRUCache({ max: options.cacheMaxSize ?? DEFAULT_CACHE_MAX_SIZE, ttl: options.cacheTtl });
		}

		if (isBindingOptions(options)) {
			// Validate that no HTTP-specific fields are present alongside `binding`.
			if (this.localEvaluation) {
				throw new Error('Flagship: localEvaluation is incompatible with binding mode');
			}
			const conflicts = HTTP_ONLY_FIELDS.filter((key) => key in options);
			if (conflicts.length > 0) {
				throw new Error(
					`Flagship: when using a binding, the following HTTP-specific options must not be provided: ${conflicts.join(', ')}. ` +
						'Provide either a binding or HTTP configuration, not both.',
				);
			}
			this.binding = options.binding;
			this.client = undefined;
			this.accountId = undefined;
			this.resolve = this.resolveViaBinding.bind(this);
		} else {
			if (this.localEvaluation) {
				if (!options.accountId) {
					throw new Error('Flagship: "accountId" is required when localEvaluation is enabled');
				}
				// Fail fast if the definitions URL cannot be derived.
				resolveDefinitionsEndpoint(options);
			}
			this.client = new FlagshipClient(options);
			this.binding = undefined;
			this.accountId = options.accountId;
			this.resolve = this.localEvaluation ? this.resolveViaLocal.bind(this) : this.resolveViaHttp.bind(this);
		}
	}

	/**
	 * Returns the provided logger when logging is enabled, or a no-op logger
	 * when `logging` is `false`. Using this in every resolve method ensures
	 * the SDK produces no console output unless the caller opts in.
	 */
	private logger(logger: Logger): Logger {
		if (this.logging) return logger;
		return { debug: _noop, info: _noop, warn: _noop, error: _noop };
	}

	/**
	 * In local mode, blocks until the first definitions fetch succeeds.
	 * Failure puts the OpenFeature provider in ERROR.
	 */
	async initialize(): Promise<void> {
		if (!this.localEvaluation || !this.client) return;
		// Bump the epoch so any refresh still in flight from a prior session is
		// ignored when it resolves. Drop residual state before the blocking fetch.
		this.epoch += 1;
		this.localReady = false;
		this.stale = false;
		this.snapshot = undefined;
		this.etag = undefined;
		this.lastFetch = 0;
		this.refreshInFlight = undefined;
		const epoch = this.epoch;
		await this.fetchAndApplySnapshot(/* initial */ true, undefined, epoch);
		// Only mark ready if this initialize was not superseded by onClose/re-init.
		if (epoch === this.epoch) this.localReady = true;
	}

	async onClose(): Promise<void> {
		// Bump the epoch so in-flight fetches observe a stale generation and
		// refuse to write. Clearing refreshInFlight is safe: the old promise
		// finally handler only clears when it still owns the slot (identity check).
		this.epoch += 1;
		this.localReady = false;
		this.stale = false;
		this.snapshot = undefined;
		this.etag = undefined;
		this.lastFetch = 0;
		this.refreshInFlight = undefined;
		this.cache?.clear();
	}

	async resolveBooleanEvaluation(
		flagKey: string,
		defaultValue: boolean,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<boolean>> {
		return this.resolveCached(flagKey, defaultValue, context, 'boolean', logger);
	}

	async resolveStringEvaluation(
		flagKey: string,
		defaultValue: string,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<string>> {
		return this.resolveCached(flagKey, defaultValue, context, 'string', logger);
	}

	async resolveNumberEvaluation(
		flagKey: string,
		defaultValue: number,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<number>> {
		return this.resolveCached(flagKey, defaultValue, context, 'number', logger);
	}

	async resolveObjectEvaluation<T extends JsonValue>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		return this.resolveCached(flagKey, defaultValue, context, 'object', logger);
	}

	// ---------------------------------------------------------------------------
	// Caching
	// ---------------------------------------------------------------------------

	private async resolveCached<T>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		expectedType: ExpectedType,
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		if (!this.cache) {
			return this.resolve(flagKey, defaultValue, context, expectedType, logger);
		}

		let key: string;
		try {
			key = buildCacheKey(flagKey, expectedType, context);
		} catch (error) {
			return this.handleHttpError(flagKey, defaultValue, error, this.logger(logger));
		}
		const cached = this.cache.get(key) as ResolutionDetails<T> | undefined;
		if (cached) {
			return { ...cached, reason: 'CACHED' };
		}

		const result = await this.resolve(flagKey, defaultValue, context, expectedType, logger);
		if (isCacheable(result)) {
			this.cache.set(key, result as ResolutionDetails<unknown>);
		}
		return result;
	}

	// ---------------------------------------------------------------------------
	// HTTP mode resolution
	// ---------------------------------------------------------------------------

	private async resolveViaHttp<T>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		expectedType: ExpectedType,
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		const log = this.logger(logger);
		try {
			log.debug(`[Flagship] Evaluating flag "${flagKey}" (expected: ${expectedType})`);

			const result = await this.client!.evaluate(flagKey, context);
			return finalizeEvaluation(flagKey, defaultValue, expectedType, result, log);
		} catch (error) {
			return this.handleHttpError(flagKey, defaultValue, error, log);
		}
	}

	private handleHttpError<T>(flagKey: string, defaultValue: T, error: unknown, logger: Logger): ResolutionDetails<T> {
		if (error instanceof FlagshipError) {
			let errorCode: ErrorCode;

			switch (error.code) {
				case FlagshipErrorCode.NETWORK_ERROR:
					errorCode = error.errorCode
						? mapErrorCode(error.errorCode)
						: error.cause instanceof Response && error.cause.status === 404
							? ErrorCode.FLAG_NOT_FOUND
							: ErrorCode.GENERAL;
					break;
				case FlagshipErrorCode.TIMEOUT_ERROR:
				case FlagshipErrorCode.ABORTED:
					errorCode = ErrorCode.GENERAL;
					break;
				case FlagshipErrorCode.PARSE_ERROR:
					errorCode = ErrorCode.PARSE_ERROR;
					break;
				case FlagshipErrorCode.INVALID_CONTEXT:
					errorCode = ErrorCode.INVALID_CONTEXT;
					break;
				default:
					errorCode = ErrorCode.GENERAL;
			}

			logger.error(`[Flagship] Flag "${flagKey}" evaluation failed (${errorCode}): ${error.message}`);
			return { value: defaultValue, errorCode, errorMessage: error.message, reason: 'ERROR' };
		}

		const errorMessage = String(error);
		logger.error(`[Flagship] Flag "${flagKey}" evaluation failed (GENERAL): ${errorMessage}`);
		return { value: defaultValue, errorCode: ErrorCode.GENERAL, errorMessage, reason: 'ERROR' };
	}

	// ---------------------------------------------------------------------------
	// Local evaluation mode
	// ---------------------------------------------------------------------------

	private async resolveViaLocal<T>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		expectedType: ExpectedType,
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		const log = this.logger(logger);
		this.maybeRefresh(log);

		const snapshot = this.snapshot;
		if (!snapshot) {
			const msg = 'Flagship local evaluation is not ready — definitions have not been loaded';
			log.error(`[Flagship] Flag "${flagKey}" evaluation failed (PROVIDER_NOT_READY): ${msg}`);
			return { value: defaultValue, errorCode: ErrorCode.PROVIDER_NOT_READY, errorMessage: msg, reason: 'ERROR' };
		}

		const flagDef = Object.prototype.hasOwnProperty.call(snapshot, flagKey) ? snapshot[flagKey] : undefined;
		if (!flagDef || typeof flagDef !== 'object') {
			const msg = `Flag "${flagKey}" was not found in local definitions`;
			log.error(`[Flagship] ${msg}`);
			return { value: defaultValue, errorCode: ErrorCode.FLAG_NOT_FOUND, errorMessage: msg, reason: 'ERROR' };
		}

		try {
			log.debug(`[Flagship] Evaluating flag "${flagKey}" locally (expected: ${expectedType})`);
			const normalized = normalizeEvaluationContext(context);
			const result = evaluateFlag(flagDef, normalized.context, this.accountId!);
			return finalizeEvaluation(flagKey, defaultValue, expectedType, result, log);
		} catch (error) {
			if (error instanceof FlagConfigError) {
				const msg = error.message;
				log.error(`[Flagship] Flag "${flagKey}" evaluation failed (PARSE_ERROR): ${msg}`);
				return { value: defaultValue, errorCode: ErrorCode.PARSE_ERROR, errorMessage: msg, reason: 'ERROR' };
			}
			if (error instanceof FlagshipError) {
				return this.handleHttpError(flagKey, defaultValue, error, log);
			}
			// The engine is pure, so any other throw means the flag entry is malformed.
			const errorMessage = error instanceof Error ? error.message : String(error);
			const msg = `Flag "${flagKey}" definition is malformed: ${errorMessage}`;
			log.error(`[Flagship] ${msg} (PARSE_ERROR)`);
			return { value: defaultValue, errorCode: ErrorCode.PARSE_ERROR, errorMessage: msg, reason: 'ERROR' };
		}
	}

	/**
	 * Lazy stale-while-revalidate: if the snapshot is older than the refresh
	 * interval and no refresh is already in flight, kick one off without
	 * awaiting it. Failures are swallowed (log only) and `lastFetch` is bumped
	 * so a down API isn't hammered on every evaluate.
	 */
	private maybeRefresh(log: Logger): void {
		if (!this.localReady || this.refreshInFlight) return;
		if (Date.now() - this.lastFetch < this.refreshInterval) return;

		const epoch = this.epoch;
		const pending = this.fetchAndApplySnapshot(false, log, epoch);
		this.refreshInFlight = pending;
		void pending.finally(() => {
			// Only clear the slot if we still own it — a later initialize/onClose
			// may have started a different fetch or cleared the pointer.
			if (this.refreshInFlight === pending) this.refreshInFlight = undefined;
		});
	}

	private async fetchAndApplySnapshot(initial: boolean, log: Logger | undefined, epoch: number): Promise<void> {
		try {
			const result = await this.client!.fetchDefinitions(this.etag);
			// Stale session (closed or superseded by a newer initialize).
			if (epoch !== this.epoch) return;

			if (result === 'not-modified') {
				this.lastFetch = Date.now();
				this.recover();
				log?.debug('[Flagship] Definitions not modified (304)');
				return;
			}

			// Only validate the top-level shape; individual malformed flags are
			// left in the snapshot and surface PARSE_ERROR at evaluation time.
			this.snapshot = result.flags as Record<string, EvalFlag>;
			this.etag = result.etag || undefined;
			if (!result.etag) {
				log?.debug('[Flagship] Definitions response had no ETag; conditional refresh (304) is unavailable');
			}
			this.lastFetch = Date.now();
			this.recover();
			log?.debug(`[Flagship] Definitions snapshot updated (${Object.keys(this.snapshot).length} flags)`);
		} catch (error) {
			if (epoch !== this.epoch) return;
			this.lastFetch = Date.now();
			const message = error instanceof Error ? error.message : String(error);
			if (initial) throw isAuthFailure(error) ? new ProviderFatalError(message) : error;
			this.markStale(message);
			log?.warn(`[Flagship] Definitions refresh failed; keeping last good snapshot: ${message}`);
		}
	}

	private markStale(message: string): void {
		if (this.stale) return;
		this.stale = true;
		this.events.emit(ProviderEvents.Stale, { message });
	}

	private recover(): void {
		if (!this.stale) return;
		this.stale = false;
		this.events.emit(ProviderEvents.Ready);
	}

	// ---------------------------------------------------------------------------
	// Binding mode resolution
	// ---------------------------------------------------------------------------

	private async resolveViaBinding<T>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		expectedType: ExpectedType,
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		const log = this.logger(logger);
		try {
			log.debug(`[Flagship] Evaluating flag "${flagKey}" via binding (expected: ${expectedType})`);

			const bindingContext = normalizeEvaluationContext(context).context;
			const details = await this.evaluateBinding(flagKey, defaultValue, expectedType, bindingContext);

			// If the binding signals an error, map it to an OpenFeature error response.
			if (details.errorCode) {
				const errorCode = mapErrorCode(details.errorCode);
				const errorMessage = details.errorMessage ?? `Binding error: ${details.errorCode}`;
				log.error(`[Flagship] Flag "${flagKey}" evaluation failed (${errorCode}): ${errorMessage}`);
				return { value: defaultValue, errorCode, errorMessage, reason: details.reason ?? 'ERROR' };
			}

			return finalizeEvaluation(flagKey, defaultValue, expectedType, details, log);
		} catch (error) {
			if (error instanceof FlagshipError) return this.handleHttpError(flagKey, defaultValue, error, log);
			const errorMessage = error instanceof Error ? error.message : String(error);
			log.error(`[Flagship] Flag "${flagKey}" binding evaluation failed (GENERAL): ${errorMessage}`);
			return { value: defaultValue, errorCode: ErrorCode.GENERAL, errorMessage, reason: 'ERROR' };
		}
	}

	/**
	 * Calls the appropriate `*Details` method on the binding based on the
	 * expected type. Falls back to `get` + synthetic details for unknown types.
	 */
	private async evaluateBinding<T>(
		flagKey: string,
		defaultValue: T,
		expectedType: ExpectedType,
		context: Record<string, NormalizedContextValue>,
	): Promise<FlagshipBindingEvaluationDetails<T>> {
		const binding = this.binding!;
		// The published binding types are primitive-only, but the runtime accepts structured context.
		const compatibleContext = context as Record<string, string | number | boolean>;

		switch (expectedType) {
			case 'boolean':
				return binding.getBooleanDetails(flagKey, defaultValue as unknown as boolean, compatibleContext) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
			case 'string':
				return binding.getStringDetails(flagKey, defaultValue as unknown as string, compatibleContext) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
			case 'number':
				return binding.getNumberDetails(flagKey, defaultValue as unknown as number, compatibleContext) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
			case 'object':
				return binding.getObjectDetails(flagKey, defaultValue as unknown as object, compatibleContext) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
		}
	}
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/**
 * Shared post-processing for HTTP, local, and binding evaluation results:
 * DISABLED → default value; wrong type → TYPE_MISMATCH; otherwise pass through.
 */
function finalizeEvaluation<T>(
	flagKey: string,
	defaultValue: T,
	expectedType: ExpectedType,
	result: { value: unknown; variant?: string; reason?: string },
	log: Logger,
): ResolutionDetails<T> {
	if (result.reason === 'DISABLED') {
		return { value: defaultValue, reason: 'DISABLED', flagMetadata: {} };
	}

	const actualType = getValueType(result.value);
	if (actualType !== expectedType) {
		const msg = `Flag "${flagKey}" type mismatch: expected ${expectedType}, got ${actualType}`;
		log.warn(`[Flagship] ${msg}`);
		return { value: defaultValue, errorCode: ErrorCode.TYPE_MISMATCH, errorMessage: msg, reason: 'ERROR' };
	}

	log.debug(`[Flagship] Flag "${flagKey}" resolved: value=${String(result.value)} reason=${result.reason} variant=${result.variant}`);

	return {
		value: result.value as T,
		variant: result.variant,
		reason: result.reason,
		flagMetadata: {},
	};
}

/**
 * Maps a runtime value to one of the four OpenFeature flag types.
 * `null` maps to `'object'` (typeof null === 'object'), treating it as a
 * JSON null which belongs to the object/structure category.
 */
function getValueType(value: unknown): ExpectedType {
	if (typeof value === 'boolean') return 'boolean';
	if (typeof value === 'string') return 'string';
	if (typeof value === 'number') return 'number';
	return 'object';
}

/** A resolution is cacheable only when it succeeded and isn't a disabled flag. */
function isCacheable(details: ResolutionDetails<unknown>): boolean {
	return details.errorCode === undefined && details.reason !== 'DISABLED';
}

/** Stable cache key over flag key, expected type, and the evaluation context. */
function buildCacheKey(flagKey: string, expectedType: ExpectedType, context: EvaluationContext): string {
	const entries = Object.entries(normalizeEvaluationContext(context).context)
		.map(([key, value]): [string, string] => [key, serializeContextValue(value)])
		.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
	return JSON.stringify([flagKey, expectedType, entries]);
}

function serializeContextValue(value: NormalizedContextValue): string {
	// Sort object keys at every depth so semantically equal objects share a cache key.
	return JSON.stringify(value, (_key, val) =>
		val !== null && typeof val === 'object' && !Array.isArray(val)
			? Object.fromEntries(Object.entries(val).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
			: val,
	);
}

/** Maps an error code string from the binding or an HTTP error body to an OpenFeature `ErrorCode`. */
function mapErrorCode(code: string): ErrorCode {
	switch (code) {
		case 'PROVIDER_NOT_READY':
			return ErrorCode.PROVIDER_NOT_READY;
		case 'PROVIDER_FATAL':
			return ErrorCode.PROVIDER_FATAL;
		case 'FLAG_NOT_FOUND':
			return ErrorCode.FLAG_NOT_FOUND;
		case 'PARSE_ERROR':
			return ErrorCode.PARSE_ERROR;
		case 'TYPE_MISMATCH':
			return ErrorCode.TYPE_MISMATCH;
		case 'TARGETING_KEY_MISSING':
			return ErrorCode.TARGETING_KEY_MISSING;
		case 'INVALID_CONTEXT':
			return ErrorCode.INVALID_CONTEXT;
		default:
			return ErrorCode.GENERAL;
	}
}
