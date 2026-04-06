/**
 * @cloudflare/flagship/server
 *
 * OpenFeature server provider and hooks for Flagship feature flags.
 * Requires `@openfeature/server-sdk` as a peer dependency.
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
 * ```
 *
 * @packageDocumentation
 */

// Re-export server-relevant core utilities and types
export { FlagshipClient, ContextTransformer, FlagshipError, FlagshipErrorCode, FLAGSHIP_DEFAULT_BASE_URL } from './index.js';
export type { FlagshipProviderOptions, FlagshipEvaluationResponse } from './index.js';

// Export server provider
export { FlagshipServerProvider } from './server-provider.js';

// Export hooks (server-sdk only)
export { LoggingHook } from './hooks/logging-hook.js';
export { TelemetryHook } from './hooks/telemetry-hook.js';
export type { TelemetryEvent } from './hooks/telemetry-hook.js';
