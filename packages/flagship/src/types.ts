/** Default base URL for the Flagship API. */
export const FLAGSHIP_DEFAULT_BASE_URL = 'https://api.flagship.cloudflare.dev';

/**
 * Configuration options for Flagship providers.
 *
 * Provide either `appId` + `accountId` (recommended) or `endpoint` (for full control).
 *
 * @example Using appId + accountId (recommended)
 * { appId: 'app-abc123', accountId: 'your-account-id' }
 *
 * @example Using appId with custom base URL for local dev
 * { appId: 'app-abc123', accountId: 'your-account-id', baseUrl: 'http://localhost:8787' }
 *
 * @example Using endpoint for full control
 * { endpoint: 'http://localhost:8787/v1/my-account/apps/app-abc123/evaluate' }
 */
export interface FlagshipProviderOptions {
	/**
	 * Your Flagship app ID. The SDK constructs the evaluation URL automatically.
	 * Mutually exclusive with `endpoint`.
	 */
	appId?: string;

	/**
	 * Base URL for the Flagship API. Only used with `appId`.
	 * @default 'https://api.flagship.cloudflare.dev'
	 */
	baseUrl?: string;

	/**
	 * Account ID for multi-tenant routing. Required when using `appId`.
	 */
	accountId?: string;

	/**
	 * Full evaluation endpoint URL. Mutually exclusive with `appId`.
	 * Use this for local development or custom deployments.
	 */
	endpoint?: string;

	/**
	 * Bearer token for authenticating requests to the Flagship API.
	 * When set, an `Authorization: Bearer <token>` header is automatically
	 * added to every request.
	 *
	 * If you also supply an `Authorization` header via `fetchOptions.headers`,
	 * the explicit header takes precedence and `bearerToken` is ignored for
	 * that header slot.
	 *
	 * @example
	 * { appId: 'app-abc123', accountId: 'my-account', bearerToken: 'my-secret-token' }
	 */
	bearerToken?: string;

	/**
	 * Custom fetch options applied to every request (e.g. custom headers).
	 * Headers provided here are merged with any headers derived from other
	 * options (e.g. `bearerToken`), with values in `fetchOptions.headers`
	 * taking precedence.
	 */
	fetchOptions?: RequestInit;

	/**
	 * Request timeout in milliseconds.
	 * @default 5000
	 */
	timeout?: number;

	/**
	 * Number of retry attempts on transient errors. Capped at 10.
	 * 404 and 400 responses are never retried.
	 * @default 1
	 */
	retries?: number;

	/**
	 * Fixed delay in milliseconds between retry attempts. Capped at 30 000.
	 * @default 1000
	 */
	retryDelay?: number;
}

/**
 * Configuration options for `FlagshipClientProvider` (browser / static-context environments).
 */
export interface FlagshipClientProviderOptions extends FlagshipProviderOptions {
	/**
	 * Flag keys to pre-fetch whenever the evaluation context changes.
	 * Pre-fetched flags are stored in an in-memory cache and resolved
	 * synchronously by the OpenFeature web SDK.
	 * @default []
	 */
	prefetchFlags?: string[];

	/**
	 * Cache TTL in milliseconds. When a cached entry is older than this value
	 * it is evicted and the next resolution returns the default value with
	 * reason `'DEFAULT'`. Set to `0` to disable expiry.
	 * @default 0
	 */
	cacheTTL?: number;
}

/**
 * A single entry in the client provider's in-memory flag cache.
 * Fields mirror the data-plane's `EvaluateResult` plus a wall-clock timestamp
 * for TTL enforcement.
 */
export interface CachedFlag {
	value: unknown;
	reason: string;
	variant: string;
	timestamp: number;
}

/**
 * Shape of a successful response from the Flagship evaluation API.
 * Mirrors the data-plane's `EvaluateResult` contract exactly — every field
 * is always present on a 200 response.
 */
export interface FlagshipEvaluationResponse {
	flagKey: string;
	value: unknown;
	/** The variation key that was served (e.g. `'on'`, `'off'`, `'v2'`). */
	variant: string;
	/**
	 * Why this value was returned:
	 * - `TARGETING_MATCH` — a rule matched the evaluation context
	 * - `SPLIT`           — a percentage rollout rule matched
	 * - `DEFAULT`         — no rule matched; the default variation was served
	 * - `DISABLED`        — the flag is disabled
	 */
	reason: 'TARGETING_MATCH' | 'DEFAULT' | 'DISABLED' | 'SPLIT';
}

/**
 * Internal error codes produced by `FlagshipClient`.
 * These are mapped to OpenFeature `ErrorCode` values by the providers.
 */
export enum FlagshipErrorCode {
	/** HTTP or fetch-level failure (non-404/400 status, connection refused, etc.) */
	NETWORK_ERROR = 'NETWORK_ERROR',
	/** The request was aborted because the configured timeout elapsed. */
	TIMEOUT_ERROR = 'TIMEOUT_ERROR',
	/** The response body was not a valid evaluation response. */
	PARSE_ERROR = 'PARSE_ERROR',
	/** The evaluation context contained complex values that cannot be serialized to query parameters. */
	INVALID_CONTEXT = 'INVALID_CONTEXT',
}

/**
 * Error thrown by `FlagshipClient` for all abnormal conditions.
 * Carries a `code` for programmatic handling and an optional `cause` which
 * is the underlying `Response` object for HTTP errors, allowing callers to
 * inspect the status code (e.g. to distinguish 404 → `FLAG_NOT_FOUND`).
 */
export class FlagshipError extends Error {
	constructor(
		message: string,
		public code: FlagshipErrorCode,
		public cause?: unknown,
	) {
		super(message);
		this.name = 'FlagshipError';
		// Restore the prototype chain so `instanceof FlagshipError` works correctly
		// in environments that compile TypeScript to ES5.
		Object.setPrototypeOf(this, new.target.prototype);
	}
}
