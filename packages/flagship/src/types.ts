/// <reference types="@cloudflare/workers-types" />

/** Default base URL for the Flagship API. */
export const FLAGSHIP_DEFAULT_BASE_URL = 'https://api.cloudflare.com';

/**
 * Configuration options for Flagship providers.
 *
 * Provide either `appId` + `accountId` (recommended) or `endpoint` (for full control).
 *
 * @example Using appId + accountId (recommended)
 * { appId: 'app-abc123', accountId: 'your-account-id', authToken: 'your-token' }
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
	 * @default 'https://api.cloudflare.com'
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
	 * Enable SDK-level logging. When `false` (the default), the SDK produces no
	 * console output of its own — all internal `debug`, `warn`, and `error` calls
	 * are suppressed. Set to `true` to surface evaluation debug info and errors in
	 * the console, which can be helpful during development or debugging.
	 *
	 * Note: this only controls logs emitted directly by the Flagship SDK.
	 * OpenFeature's own framework-level logs are controlled separately via
	 * `OpenFeature.setLogger(...)`.
	 *
	 * @default false
	 */
	logging?: boolean;

	/**
	 * Bearer token for authenticating requests to the Flagship API.
	 * When set, an `Authorization: Bearer <token>` header is automatically
	 * added to every request.
	 *
	 * If you also supply an `Authorization` header via `fetchOptions.headers`,
	 * the explicit header takes precedence and `authToken` is ignored for
	 * that header slot.
	 *
	 * @example
	 * { appId: 'app-abc123', accountId: 'my-account', authToken: 'my-secret-token' }
	 */
	authToken?: string;

	/**
	 * Custom fetch options applied to every request (e.g. custom headers).
	 * Headers provided here are merged with any headers derived from other
	 * options (e.g. `authToken`), with values in `fetchOptions.headers`
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
	 * Flag keys to fetch during `initialize()` and on every `onContextChange()`.
	 * Fetched flags are stored in an in-memory cache and resolved synchronously
	 * by the OpenFeature web SDK. Any flag key not listed here will return
	 * `ErrorCode.FLAG_NOT_FOUND` at resolution time.
	 *
	 * @example
	 * prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads']
	 */
	prefetchFlags?: string[];
}

/**
 * A single entry in the client provider's in-memory flag cache.
 * Fields mirror the data-plane's `EvaluateResult`.
 */
export interface CachedFlag {
	value: unknown;
	reason: string;
	variant: string;
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

// ---------------------------------------------------------------------------
// Flagship Wrangler Binding types
//
// Re-exported from @cloudflare/workers-types so consumers don't need to
// install the full workers-types package just for typing `env.FLAGS`.
// ---------------------------------------------------------------------------

/**
 * Shape of the Flagship wrangler binding exposed on `env` in Cloudflare Workers.
 *
 * This is an alias for the `Flags` class from `@cloudflare/workers-types`.
 * The binding communicates directly with the Flagship service via workerd RPC —
 * no HTTP overhead, no auth tokens required. Configure it in `wrangler.json`:
 *
 * ```jsonc
 * {
 *   "flagship": [
 *     { "binding": "FLAGS", "app_id": "<your-app-id>" }
 *   ]
 * }
 * ```
 */
export type FlagshipBinding = Flags;

/**
 * Evaluation details returned by the Flagship binding's `*Details` methods.
 * Contains the resolved value along with metadata about why that value was
 * chosen.
 *
 * This is an alias for the `EvaluationDetails` interface from
 * `@cloudflare/workers-types`.
 */
export type FlagshipBindingEvaluationDetails<T> = EvaluationDetails<T>;

/**
 * Configuration for `FlagshipServerProvider` when using a wrangler binding.
 *
 * In this mode the provider delegates all evaluations to the Flagship binding
 * on `env`, bypassing HTTP entirely. No `appId`, `accountId`, or `authToken`
 * is required — the binding handles authentication and routing.
 *
 * @example
 * ```typescript
 * new FlagshipServerProvider({ binding: env.FLAGS })
 * ```
 */
export interface FlagshipBindingProviderOptions {
	/** The Flagship binding from the Worker's `env` object. */
	binding: FlagshipBinding;

	/**
	 * Enable SDK-level logging.
	 * @default false
	 */
	logging?: boolean;
}

/**
 * Options accepted by `FlagshipServerProvider`.
 *
 * Provide **either** HTTP configuration (`appId`/`endpoint` + credentials) **or**
 * a wrangler `binding` — never both. The provider detects which mode to use
 * based on the presence of the `binding` field.
 */
export type FlagshipServerProviderOptions = FlagshipProviderOptions | FlagshipBindingProviderOptions;

/**
 * Type guard: returns `true` when the options use binding mode.
 */
export function isBindingOptions(options: FlagshipServerProviderOptions): options is FlagshipBindingProviderOptions {
	return 'binding' in options && options.binding !== null && options.binding !== undefined;
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

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
