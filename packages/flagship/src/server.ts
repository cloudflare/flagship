/**
 * @cloudflare/flagship/server
 *
 * OpenFeature server provider and hooks for Flagship feature flags.
 * Requires `@openfeature/server-sdk` as a peer dependency.
 *
 * The provider supports two modes:
 * - **Binding mode** (recommended for Workers) — uses a wrangler binding, no HTTP.
 * - **HTTP mode** — makes HTTP requests to the Flagship API.
 *
 * @example Binding mode (Cloudflare Workers)
 * ```typescript
 * import { OpenFeature } from '@openfeature/server-sdk';
 * import { FlagshipServerProvider } from '@cloudflare/flagship/server';
 *
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipServerProvider({ binding: env.FLAGS })
 * );
 * ```
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
 *   })
 * );
 * ```
 *
 * @packageDocumentation
 */

// Re-export server-relevant core utilities and types
export {
	FlagshipClient,
	ContextTransformer,
	FlagshipError,
	FlagshipErrorCode,
	FLAGSHIP_DEFAULT_BASE_URL,
	isBindingOptions,
} from './index.js';
export type {
	FlagshipProviderOptions,
	FlagshipEvaluationResponse,
	FlagshipBinding,
	FlagshipBindingEvaluationDetails,
	FlagshipBindingProviderOptions,
	FlagshipServerProviderOptions,
} from './index.js';

// Export server provider
export { FlagshipServerProvider } from './server-provider.js';

// Export hooks (server-sdk only)
export { LoggingHook } from './hooks/logging-hook.js';
export { TelemetryHook } from './hooks/telemetry-hook.js';
export type { TelemetryEvent } from './hooks/telemetry-hook.js';
