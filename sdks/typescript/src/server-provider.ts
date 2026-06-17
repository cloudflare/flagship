import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/server-sdk';
import { ErrorCode, OpenFeatureEventEmitter } from '@openfeature/server-sdk';
import { LRUCache } from 'lru-cache';
import { FlagshipClient } from './client.js';
import {
	FlagshipError,
	FlagshipErrorCode,
	isBindingOptions,
	type FlagshipBinding,
	type FlagshipBindingEvaluationDetails,
	type FlagshipServerProviderOptions,
} from './types.js';

const DEFAULT_CACHE_MAX_SIZE = 1000;
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
	'timeout',
	'retries',
	'retryDelay',
] as const;

/**
 * OpenFeature provider for Flagship (server-side / dynamic context).
 *
 * Supports two modes of operation:
 *
 * **HTTP mode** — evaluates flags via HTTP requests to the Flagship API.
 * Requires `appId`/`endpoint`, `accountId`, and optionally `authToken`.
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

	/** Set when operating in HTTP mode; `undefined` in binding mode. */
	private readonly client: FlagshipClient | undefined;
	/** Set when operating in binding mode; `undefined` in HTTP mode. */
	private readonly binding: FlagshipBinding | undefined;
	private readonly logging: boolean;

	/** TTL + LRU response cache; `undefined` when caching is disabled. */
	private readonly cache: LRUCache<string, ResolutionDetails<unknown>> | undefined;

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

		if (options.cacheTtl !== undefined && options.cacheTtl > 0) {
			this.cache = new LRUCache({ max: options.cacheMaxSize ?? DEFAULT_CACHE_MAX_SIZE, ttl: options.cacheTtl });
		}

		if (isBindingOptions(options)) {
			// Validate that no HTTP-specific fields are present alongside `binding`.
			const conflicts = HTTP_ONLY_FIELDS.filter((key) => key in options);
			if (conflicts.length > 0) {
				throw new Error(
					`Flagship: when using a binding, the following HTTP-specific options must not be provided: ${conflicts.join(', ')}. ` +
						'Provide either a binding or HTTP configuration, not both.',
				);
			}
			this.binding = options.binding;
			this.client = undefined;
			this.resolve = this.resolveViaBinding.bind(this);
		} else {
			this.client = new FlagshipClient(options);
			this.binding = undefined;
			this.resolve = this.resolveViaHttp.bind(this);
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

	async onClose(): Promise<void> {
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

		const key = buildCacheKey(flagKey, expectedType, context);
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
		} catch (error) {
			return this.handleHttpError(flagKey, defaultValue, error, log);
		}
	}

	private handleHttpError<T>(flagKey: string, defaultValue: T, error: unknown, logger: Logger): ResolutionDetails<T> {
		if (error instanceof FlagshipError) {
			let errorCode: ErrorCode;

			switch (error.code) {
				case FlagshipErrorCode.NETWORK_ERROR:
					errorCode = error.cause instanceof Response && error.cause.status === 404 ? ErrorCode.FLAG_NOT_FOUND : ErrorCode.GENERAL;
					break;
				case FlagshipErrorCode.TIMEOUT_ERROR:
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

			const bindingContext = toBindingContext(context, log);
			const details = await this.evaluateBinding(flagKey, defaultValue, expectedType, bindingContext);

			// If the binding signals an error, map it to an OpenFeature error response.
			if (details.errorCode) {
				const errorCode = mapBindingErrorCode(details.errorCode);
				const errorMessage = details.errorMessage ?? `Binding error: ${details.errorCode}`;
				log.error(`[Flagship] Flag "${flagKey}" evaluation failed (${errorCode}): ${errorMessage}`);
				return { value: defaultValue, errorCode, errorMessage, reason: details.reason ?? 'ERROR' };
			}

			if (details.reason === 'DISABLED') {
				return { value: defaultValue, reason: 'DISABLED', flagMetadata: {} };
			}

			// Type-check the resolved value.
			const actualType = getValueType(details.value);
			if (actualType !== expectedType) {
				const msg = `Flag "${flagKey}" type mismatch: expected ${expectedType}, got ${actualType}`;
				log.warn(`[Flagship] ${msg}`);
				return { value: defaultValue, errorCode: ErrorCode.TYPE_MISMATCH, errorMessage: msg, reason: 'ERROR' };
			}

			log.debug(
				`[Flagship] Flag "${flagKey}" resolved via binding: value=${String(details.value)} reason=${details.reason} variant=${details.variant}`,
			);

			return {
				value: details.value as T,
				variant: details.variant,
				reason: details.reason,
				flagMetadata: {},
			};
		} catch (error) {
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
		context: Record<string, string | number | boolean>,
	): Promise<FlagshipBindingEvaluationDetails<T>> {
		const binding = this.binding!;

		switch (expectedType) {
			case 'boolean':
				return binding.getBooleanDetails(flagKey, defaultValue as unknown as boolean, context) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
			case 'string':
				return binding.getStringDetails(flagKey, defaultValue as unknown as string, context) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
			case 'number':
				return binding.getNumberDetails(flagKey, defaultValue as unknown as number, context) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
			case 'object':
				return binding.getObjectDetails(flagKey, defaultValue as unknown as object, context) as Promise<
					FlagshipBindingEvaluationDetails<T>
				>;
		}
	}
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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
	const entries = Object.entries(context)
		.filter(([, value]) => value !== undefined && value !== null)
		.map(([key, value]): [string, string] => [key, serializeContextValue(value)])
		.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
	return JSON.stringify([flagKey, expectedType, entries]);
}

function serializeContextValue(value: unknown): string {
	if (value instanceof Date) return value.toISOString();
	if (typeof value === 'object') return JSON.stringify(value);
	return String(value);
}

/**
 * Converts an OpenFeature `EvaluationContext` to the flat primitive map that
 * the Flagship binding expects.
 *
 * - `string`, `number`, `boolean` → pass through
 * - `Date` → ISO-8601 string
 * - `null` / `undefined` → skipped
 * - objects / arrays → skipped with a warning (when logging is enabled)
 */
function toBindingContext(context: EvaluationContext, logger: Logger): Record<string, string | number | boolean> {
	const result: Record<string, string | number | boolean> = {};

	for (const [key, value] of Object.entries(context)) {
		if (value === undefined || value === null) {
			continue;
		}

		if (value instanceof Date) {
			result[key] = value.toISOString();
			continue;
		}

		if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
			result[key] = value;
			continue;
		}

		if (typeof value === 'object') {
			logger.warn(
				`[Flagship] Context key "${key}" is a complex object/array and cannot be passed to the binding. This value will be ignored.`,
			);
			continue;
		}
	}

	return result;
}

/**
 * Maps an error code string from the binding's `EvaluationDetails` to an
 * OpenFeature `ErrorCode`.
 */
function mapBindingErrorCode(code: string): ErrorCode {
	switch (code) {
		case 'FLAG_NOT_FOUND':
			return ErrorCode.FLAG_NOT_FOUND;
		case 'PARSE_ERROR':
			return ErrorCode.PARSE_ERROR;
		case 'TYPE_MISMATCH':
			return ErrorCode.TYPE_MISMATCH;
		case 'INVALID_CONTEXT':
			return ErrorCode.INVALID_CONTEXT;
		default:
			return ErrorCode.GENERAL;
	}
}
