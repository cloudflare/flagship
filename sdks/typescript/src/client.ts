import type { EvaluationContext } from '@openfeature/core';
import { ContextTransformer } from './context.js';
import {
	FlagshipError,
	FlagshipErrorCode,
	FLAGSHIP_DEFAULT_BASE_URL,
	type FlagshipEvaluationResponse,
	type FlagshipProviderOptions,
	type FlagshipRequestOptions,
} from './types.js';

interface ResolvedOptions {
	endpoint: string;
	fetchOptions: RequestInit;
	fetch: typeof globalThis.fetch | undefined;
	timeout: number;
	retries: number;
	retryDelay: number;
}

/**
 * Non-2xx statuses that represent a transient condition. Everything else in
 * the 4xx range is treated as a definitive answer and never retried.
 */
const RETRYABLE_STATUSES = new Set([408, 425, 429]);

export class FlagshipClient {
	private readonly options: ResolvedOptions;

	constructor(options: FlagshipProviderOptions) {
		this.options = {
			endpoint: resolveEndpoint(options),
			fetchOptions: buildFetchOptions(options),
			fetch: options.fetch,
			timeout: options.timeout || 5000,
			retries: Math.min(options.retries !== undefined ? options.retries : 1, 10),
			retryDelay: Math.min(options.retryDelay !== undefined ? options.retryDelay : 1000, 30_000),
		};
	}

	/**
	 * Evaluate a flag with the given context.
	 *
	 * Throws a `FlagshipError` with `FlagshipErrorCode.INVALID_CONTEXT` if the
	 * evaluation context contains complex values (objects or arrays) that cannot
	 * be serialized to query parameters.
	 *
	 * `options.fetch` overrides the transport for this call, and
	 * `options.signal` cancels the in-flight HTTP request — aborting rejects
	 * with `FlagshipErrorCode.ABORTED` and is never retried.
	 */
	async evaluate(flagKey: string, context: EvaluationContext, options?: FlagshipRequestOptions): Promise<FlagshipEvaluationResponse> {
		const droppedKeys: string[] = [];
		const url = ContextTransformer.buildUrl(this.options.endpoint, flagKey, context, droppedKeys);

		if (droppedKeys.length > 0) {
			throw new FlagshipError(
				`Evaluation context contains complex values that cannot be serialized for flag "${flagKey}". ` +
					`Unsupported keys: ${droppedKeys.join(', ')}. Use primitive values (string, number, boolean) or Date objects.`,
				FlagshipErrorCode.INVALID_CONTEXT,
			);
		}

		return this.fetchWithRetry(url, this.options.retries, options);
	}

	/**
	 * Fetch with retry logic. Only retries failures marked as retryable —
	 * terminal responses (400, 401, 403, 404, …) and caller aborts are
	 * propagated immediately.
	 */
	private async fetchWithRetry(url: string, retriesLeft: number, options?: FlagshipRequestOptions): Promise<FlagshipEvaluationResponse> {
		try {
			return await this.fetchWithTimeout(url, this.options.timeout, options);
		} catch (error) {
			if (error instanceof FlagshipError && !error.retryable) {
				throw error;
			}

			if (retriesLeft > 0) {
				await new Promise((resolve) => setTimeout(resolve, this.options.retryDelay));
				return this.fetchWithRetry(url, retriesLeft - 1, options);
			}

			throw error;
		}
	}

	/**
	 * Issues a single request against the resolved transport, aborting it when
	 * the timeout elapses or when any caller-supplied signal fires.
	 */
	private async fetchWithTimeout(url: string, timeout: number, options?: FlagshipRequestOptions): Promise<FlagshipEvaluationResponse> {
		const callerSignals = [options?.signal, this.options.fetchOptions.signal].filter((signal): signal is AbortSignal => Boolean(signal));

		const alreadyAborted = callerSignals.find((signal) => signal.aborted);
		if (alreadyAborted) {
			throw abortedError(alreadyAborted.reason);
		}

		const transport = options?.fetch ?? this.options.fetch ?? globalThis.fetch.bind(globalThis);

		const timeoutController = new AbortController();
		let timedOut = false;
		const timeoutId = setTimeout(() => {
			timedOut = true;
			timeoutController.abort();
		}, timeout);
		const merged = mergeSignals([timeoutController.signal, ...callerSignals]);

		try {
			const response = await transport(url, {
				...this.options.fetchOptions,
				signal: merged.signal,
			});

			if (!response.ok) {
				throw new FlagshipError(
					`HTTP ${response.status}: ${response.statusText}`,
					FlagshipErrorCode.NETWORK_ERROR,
					response,
					isRetryableStatus(response.status),
				);
			}

			const data = await response.json();

			if (!data || typeof data !== 'object' || !('flagKey' in data) || !('value' in data)) {
				throw new FlagshipError('Invalid response format from Flagship API', FlagshipErrorCode.PARSE_ERROR, undefined, true);
			}

			return data as FlagshipEvaluationResponse;
		} catch (error) {
			if (error instanceof FlagshipError) {
				throw error;
			}

			const abortedBy = callerSignals.find((signal) => signal.aborted);
			if (abortedBy) {
				throw abortedError(abortedBy.reason ?? error);
			}

			if (timedOut || (error instanceof Error && error.name === 'AbortError')) {
				throw new FlagshipError(`Request timeout after ${timeout}ms`, FlagshipErrorCode.TIMEOUT_ERROR, error, true);
			}

			throw new FlagshipError(`Network error: ${error}`, FlagshipErrorCode.NETWORK_ERROR, error, true);
		} finally {
			clearTimeout(timeoutId);
			merged.dispose();
		}
	}
}

function abortedError(cause: unknown): FlagshipError {
	return new FlagshipError('Request aborted by caller', FlagshipErrorCode.ABORTED, cause, false);
}

/** 408, 425, 429 and any 5xx are transient; every other non-2xx is definitive. */
function isRetryableStatus(status: number): boolean {
	return status >= 500 || RETRYABLE_STATUSES.has(status);
}

/**
 * Combines signals into one. Prefers `AbortSignal.any` where available and
 * falls back to a manually linked `AbortController` on older runtimes.
 */
function mergeSignals(signals: AbortSignal[]): { signal: AbortSignal; dispose: () => void } {
	const noop = (): void => {};

	if (signals.length === 1) {
		return { signal: signals[0]!, dispose: noop };
	}

	if (typeof AbortSignal.any === 'function') {
		return { signal: AbortSignal.any(signals), dispose: noop };
	}

	const controller = new AbortController();
	const onAbort = (event: Event): void => controller.abort((event.target as AbortSignal).reason);

	for (const signal of signals) {
		signal.addEventListener('abort', onAbort);
	}

	return {
		signal: controller.signal,
		dispose: () => {
			for (const signal of signals) {
				signal.removeEventListener('abort', onAbort);
			}
		},
	};
}

/**
 * Merge `authToken` and `fetchOptions` into a single `RequestInit`.
 *
 * Precedence for the `Authorization` header (highest → lowest):
 * 1. An explicit `Authorization` value inside `fetchOptions.headers`
 * 2. A value derived from `authToken`
 *
 * All other `fetchOptions` fields are spread as-is.
 */
function buildFetchOptions(options: FlagshipProviderOptions): RequestInit {
	const { authToken, fetchOptions = {} } = options;

	if (!authToken) {
		return fetchOptions;
	}

	const existingHeaders = new Headers(fetchOptions.headers as HeadersInit | undefined);

	// Only inject the Authorization header when the caller hasn't already
	// provided one explicitly — their value takes precedence.
	if (!existingHeaders.has('Authorization')) {
		existingHeaders.set('Authorization', `Bearer ${authToken}`);
	}

	return {
		...fetchOptions,
		headers: existingHeaders,
	};
}

function resolveEndpoint(options: FlagshipProviderOptions): string {
	const { appId, endpoint, baseUrl, accountId } = options;

	if (appId && endpoint) {
		throw new Error('Flagship: provide either "appId" or "endpoint", not both');
	}

	if (!appId && !endpoint) {
		throw new Error('Flagship: either "appId" or "endpoint" is required');
	}

	if (endpoint) {
		try {
			new URL(endpoint);
		} catch {
			throw new Error(`Flagship: invalid endpoint URL: ${endpoint}`);
		}
		return endpoint;
	}

	if (!accountId) {
		throw new Error('Flagship: "accountId" is required when using "appId"');
	}

	const base = (baseUrl || FLAGSHIP_DEFAULT_BASE_URL).replace(/\/+$/, '');
	const resolved = `${base}/client/v4/accounts/${encodeURIComponent(accountId)}/flagship/apps/${encodeURIComponent(appId!)}/evaluate`;

	try {
		new URL(resolved);
	} catch {
		throw new Error(`Flagship: resolved endpoint is not a valid URL: ${resolved}`);
	}

	return resolved;
}
