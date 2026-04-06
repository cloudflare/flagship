/**
 * @cloudflare/flagship/web
 *
 * OpenFeature client provider for Flagship feature flags in the browser.
 * Requires `@openfeature/web-sdk` as a peer dependency.
 *
 * @example
 * ```typescript
 * import { OpenFeature } from '@openfeature/web-sdk';
 * import { FlagshipClientProvider } from '@cloudflare/flagship/web';
 *
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipClientProvider({
 *     appId: 'app-abc123',
 *     accountId: 'your-account-id',
 *     prefetchFlags: ['dark-mode', 'welcome-message'],
 *     cacheTTL: 60000,
 *   })
 * );
 * ```
 *
 * @packageDocumentation
 */

// Re-export core utilities
export { FlagshipClient, ContextTransformer, FlagshipError, FlagshipErrorCode, FLAGSHIP_DEFAULT_BASE_URL } from './index.js';
export type { FlagshipProviderOptions, FlagshipClientProviderOptions, FlagshipEvaluationResponse, CachedFlag } from './index.js';

// Export client provider
export { FlagshipClientProvider } from './client-provider.js';
