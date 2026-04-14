/**
 * @cloudflare/flagship
 *
 * Core utilities and types for Flagship feature flags.
 * For OpenFeature providers, import from the sub-paths:
 *   - `@cloudflare/flagship/server` for server-side (Node.js, Workers — HTTP or binding mode)
 *   - `@cloudflare/flagship/web` for client-side (browsers)
 *
 * This core entry also exports the `FlagshipBinding` type for typing the
 * wrangler binding in Cloudflare Workers environments.
 *
 * @packageDocumentation
 */

// Export types
export type {
	FlagshipProviderOptions,
	FlagshipClientProviderOptions,
	FlagshipEvaluationResponse,
	CachedFlag,
	FlagshipBinding,
	FlagshipBindingEvaluationDetails,
	FlagshipBindingProviderOptions,
	FlagshipServerProviderOptions,
} from './types.js';

export { FlagshipError, FlagshipErrorCode, FLAGSHIP_DEFAULT_BASE_URL, isBindingOptions } from './types.js';

// Export utilities (for advanced use cases)
export { ContextTransformer } from './context.js';
export { FlagshipClient } from './client.js';
