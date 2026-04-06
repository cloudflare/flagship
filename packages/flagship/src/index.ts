/**
 * @cloudflare/flagship
 *
 * Core utilities for Flagship feature flags.
 * For OpenFeature providers, import from the sub-paths:
 *   - `@cloudflare/flagship/server` for server-side (Node.js, Workers)
 *   - `@cloudflare/flagship/web` for client-side (browsers)
 *
 * @packageDocumentation
 */

// Export types
export type { FlagshipProviderOptions, FlagshipClientProviderOptions, FlagshipEvaluationResponse, CachedFlag } from './types.js';

export { FlagshipError, FlagshipErrorCode, FLAGSHIP_DEFAULT_BASE_URL } from './types.js';

// Export utilities (for advanced use cases)
export { ContextTransformer } from './context.js';
export { FlagshipClient } from './client.js';
