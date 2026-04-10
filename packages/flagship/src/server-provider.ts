import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/server-sdk';
import { ErrorCode, ProviderStatus, OpenFeatureEventEmitter, ProviderEvents } from '@openfeature/server-sdk';
import { FlagshipClient } from './client.js';
import { FlagshipError, FlagshipErrorCode, type FlagshipProviderOptions } from './types.js';

// Shared no-op used to build a silent logger when logging is false.
const _noop = (): void => {};

/**
 * OpenFeature provider for Flagship (server-side / dynamic context).
 *
 * Use this provider with `@openfeature/server-sdk` for Node.js,
 * Cloudflare Workers, and other server-side JavaScript environments.
 *
 * @example
 * ```typescript
 * import { OpenFeature } from '@openfeature/server-sdk';
 * import { FlagshipServerProvider } from '@cloudflare/flagship/server';
 *
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipServerProvider({
 *     appId: 'app-abc123',
 *     accountId: 'your-account-id',
 *   })
 * );
 *
 * const client = OpenFeature.getClient();
 * const value = await client.getBooleanValue('my-flag', false, {
 *   targetingKey: 'user-123',
 *   email: 'user@example.com',
 * });
 * ```
 */
export class FlagshipServerProvider implements Provider {
	readonly metadata: ProviderMetadata;
	readonly runsOn = 'server' as const;
	readonly events = new OpenFeatureEventEmitter();

	private readonly client: FlagshipClient;
	private readonly logging: boolean;
	private currentStatus: ProviderStatus = ProviderStatus.NOT_READY;

	constructor(options: FlagshipProviderOptions) {
		this.metadata = { name: 'Flagship Server Provider' };
		this.client = new FlagshipClient(options);
		this.logging = options.logging ?? false;
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
	 * Probes the evaluation endpoint with a health-check request. A 404 response
	 * is treated as success — it means the endpoint is reachable but the
	 * health-check flag simply doesn't exist, which is expected. Any network
	 * failure or timeout sets the status to ERROR.
	 */
	async initialize(_context?: EvaluationContext): Promise<void> {
		try {
			await this.client.evaluate('_flagship_health_check', {});
			this.currentStatus = ProviderStatus.READY;
			this.events.emit(ProviderEvents.Ready);
		} catch (error) {
			if (error instanceof FlagshipError && error.cause instanceof Response && error.cause.status === 404) {
				this.currentStatus = ProviderStatus.READY;
				this.events.emit(ProviderEvents.Ready);
				return;
			}
			this.currentStatus = ProviderStatus.ERROR;
			this.events.emit(ProviderEvents.Error, { message: error instanceof Error ? error.message : String(error) });
		}
	}

	async onClose(): Promise<void> {
		this.currentStatus = ProviderStatus.NOT_READY;
	}

	get status(): ProviderStatus {
		return this.currentStatus;
	}

	async resolveBooleanEvaluation(
		flagKey: string,
		defaultValue: boolean,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<boolean>> {
		return this.resolve(flagKey, defaultValue, context, 'boolean', logger);
	}

	async resolveStringEvaluation(
		flagKey: string,
		defaultValue: string,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<string>> {
		return this.resolve(flagKey, defaultValue, context, 'string', logger);
	}

	async resolveNumberEvaluation(
		flagKey: string,
		defaultValue: number,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<number>> {
		return this.resolve(flagKey, defaultValue, context, 'number', logger);
	}

	async resolveObjectEvaluation<T extends JsonValue>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		return this.resolve(flagKey, defaultValue, context, 'object', logger);
	}

	private async resolve<T>(
		flagKey: string,
		defaultValue: T,
		context: EvaluationContext,
		expectedType: 'boolean' | 'string' | 'number' | 'object',
		logger: Logger,
	): Promise<ResolutionDetails<T>> {
		const log = this.logger(logger);
		try {
			log.debug(`[Flagship] Evaluating flag "${flagKey}" (expected: ${expectedType})`);

			const result = await this.client.evaluate(flagKey, context);

			const actualType = this.getValueType(result.value);
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
			return this.handleError(flagKey, defaultValue, error, log);
		}
	}

	/**
	 * Maps a runtime value to one of the four OpenFeature flag types.
	 * `null` maps to `'object'` (typeof null === 'object'), treating it as a
	 * JSON null which belongs to the object/structure category.
	 */
	private getValueType(value: unknown): 'boolean' | 'string' | 'number' | 'object' {
		if (typeof value === 'boolean') return 'boolean';
		if (typeof value === 'string') return 'string';
		if (typeof value === 'number') return 'number';
		return 'object';
	}

	private handleError<T>(flagKey: string, defaultValue: T, error: unknown, logger: Logger): ResolutionDetails<T> {
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
}
