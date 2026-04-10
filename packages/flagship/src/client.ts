import type { EvaluationContext } from '@openfeature/core';
import { ContextTransformer } from './context.js';
import {
	FlagshipError,
	FlagshipErrorCode,
	FLAGSHIP_DEFAULT_BASE_URL,
	type FlagshipEvaluationResponse,
	type FlagshipProviderOptions,
} from './types.js';

interface ResolvedOptions {
	endpoint: string;
	fetchOptions: RequestInit;
	timeout: number;
	retries: number;
	retryDelay: number;
}

export class FlagshipClient {
	private readonly options: ResolvedOptions;

	constructor(options: FlagshipProviderOptions) {
		this.options = {
			endpoint: resolveEndpoint(options),
			fetchOptions: buildFetchOptions(options),
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
	 */
	async evaluate(flagKey: string, context: EvaluationContext): Promise<FlagshipEvaluationResponse> {
		const droppedKeys: string[] = [];
		const url = ContextTransformer.buildUrl(this.options.endpoint, flagKey, context, droppedKeys);

		if (droppedKeys.length > 0) {
			throw new FlagshipError(
				`Evaluation context contains complex values that cannot be serialized for flag "${flagKey}". ` +
					`Unsupported keys: ${droppedKeys.join(', ')}. Use primitive values (string, number, boolean) or Date objects.`,
				FlagshipErrorCode.INVALID_CONTEXT,
			);
		}

		return this.fetchWithRetry(url, this.options.retries);
	}

	/**
	 * Fetch with retry logic. Only retries on transient network/server errors —
	 * 404 and 400 responses are terminal and propagated immediately.
	 */
	private async fetchWithRetry(url: string, retriesLeft: number): Promise<FlagshipEvaluationResponse> {
		try {
			return await this.fetchWithTimeout(url, this.options.timeout);
		} catch (error) {
			// Do not retry on client errors — 404 (flag not found) and 400 (bad request)
			// are deterministic and retrying will not change the outcome.
			if (error instanceof FlagshipError && error.cause instanceof Response) {
				const status = error.cause.status;
				if (status === 404 || status === 400) {
					throw error;
				}
			}

			if (retriesLeft > 0) {
				await new Promise((resolve) => setTimeout(resolve, this.options.retryDelay));
				return this.fetchWithRetry(url, retriesLeft - 1);
			}

			throw error;
		}
	}

	/**
	 * Fetch with timeout using AbortController
	 */
	private async fetchWithTimeout(url: string, timeout: number): Promise<FlagshipEvaluationResponse> {
		const controller = new AbortController();
		const timeoutId = setTimeout(() => controller.abort(), timeout);

		try {
			const response = await fetch(url, {
				...this.options.fetchOptions,
				signal: controller.signal,
			});

			clearTimeout(timeoutId);

			if (!response.ok) {
				throw new FlagshipError(`HTTP ${response.status}: ${response.statusText}`, FlagshipErrorCode.NETWORK_ERROR, response);
			}

			const data = await response.json();

			if (!data || typeof data !== 'object' || !('flagKey' in data) || !('value' in data)) {
				throw new FlagshipError('Invalid response format from Flagship API', FlagshipErrorCode.PARSE_ERROR);
			}

			return data as FlagshipEvaluationResponse;
		} catch (error) {
			clearTimeout(timeoutId);

			if (error instanceof Error && error.name === 'AbortError') {
				throw new FlagshipError(`Request timeout after ${timeout}ms`, FlagshipErrorCode.TIMEOUT_ERROR, error);
			}

			if (error instanceof FlagshipError) {
				throw error;
			}

			throw new FlagshipError(`Network error: ${error}`, FlagshipErrorCode.NETWORK_ERROR, error);
		}
	}
}

/**
 * Merge `token` and `fetchOptions` into a single `RequestInit`.
 *
 * Precedence for the `Authorization` header (highest → lowest):
 * 1. An explicit `Authorization` value inside `fetchOptions.headers`
 * 2. A value derived from `token`
 *
 * All other `fetchOptions` fields are spread as-is.
 */
function buildFetchOptions(options: FlagshipProviderOptions): RequestInit {
	const { token, fetchOptions = {} } = options;

	if (!token) {
		return fetchOptions;
	}

	const existingHeaders = new Headers(fetchOptions.headers as HeadersInit | undefined);

	// Only inject the Authorization header when the caller hasn't already
	// provided one explicitly — their value takes precedence.
	if (!existingHeaders.has('Authorization')) {
		existingHeaders.set('Authorization', `Bearer ${token}`);
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
	const resolved = `${base}/v1/${encodeURIComponent(accountId)}/apps/${encodeURIComponent(appId!)}/evaluate`;

	try {
		new URL(resolved);
	} catch {
		throw new Error(`Flagship: resolved endpoint is not a valid URL: ${resolved}`);
	}

	return resolved;
}
