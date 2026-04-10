/**
 * Example: FlagshipClientProvider (Node.js / browser)
 *
 * Run this to test the client provider end-to-end:
 *   pnpm --filter @cloudflare/flagship run examples:browser
 *
 * The FlagshipClientProvider works like other production OpenFeature client
 * providers (ofrep-web, flagd-web): all flags are fetched upfront during
 * initialization and on every context change, then served synchronously from
 * an in-memory cache. Any flag not listed in `prefetchFlags` returns
 * FLAG_NOT_FOUND at resolution time.
 */

// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-nocheck

import { OpenFeature } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '@cloudflare/flagship/web';

const ACCOUNT_ID = 'a89743af3e108be922f3440b9feeb9da';
const APP_ID = '80c8011b-2778-4d34-bdd1-8e50e228ad98';

// ---------------------------------------------------------------------------
// 1. Initialize — fetches all prefetchFlags upfront
// ---------------------------------------------------------------------------

await OpenFeature.setProviderAndWait(
	new FlagshipClientProvider({
		accountId: ACCOUNT_ID,
		appId: APP_ID,
		// List every flag key your app uses. These are fetched on init and on
		// every context change. Any key omitted here returns FLAG_NOT_FOUND.
		prefetchFlags: ['boolean-test'],
		logging: true,
	}),
);

console.log('Provider ready.\n');

// ---------------------------------------------------------------------------
// 2. Set context — triggers a re-fetch of all prefetchFlags for this user
// ---------------------------------------------------------------------------

await OpenFeature.setContext({
	email: 'asinha@cloudflare.com',
});

console.log('Context set.\n');

// ---------------------------------------------------------------------------
// 3. Resolve flags synchronously from cache
// ---------------------------------------------------------------------------

const client = OpenFeature.getClient();

const details = client.getBooleanDetails('boolean-test', false);

console.log('boolean-test details:');
console.log('  value:    ', details.value);
console.log('  reason:   ', details.reason); // 'CACHED' on success
console.log('  variant:  ', details.variant);
console.log('  errorCode:', details.errorCode ?? 'none');

// ---------------------------------------------------------------------------
// 4. Demonstrate FLAG_NOT_FOUND for a flag not in prefetchFlags
// ---------------------------------------------------------------------------

const missing = client.getBooleanDetails('not-in-prefetch', false);
console.log('\nnot-in-prefetch details:');
console.log('  value:    ', missing.value); // false (default)
console.log('  reason:   ', missing.reason); // 'ERROR'
console.log('  errorCode:', missing.errorCode); // FLAG_NOT_FOUND

// ---------------------------------------------------------------------------
// 5. Context change — cache is cleared and all flags re-fetched for new user
// ---------------------------------------------------------------------------

console.log('\nChanging context to a different user…');

await OpenFeature.setContext({
	email: 'other@cloudflare.com',
});

const afterChange = client.getBooleanDetails('boolean-test', false);
console.log('\nboolean-test after context change:');
console.log('  value:    ', afterChange.value);
console.log('  reason:   ', afterChange.reason);
