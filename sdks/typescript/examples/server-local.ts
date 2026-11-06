/**
 * Example: Local in-process evaluation with FlagshipServerProvider
 *
 * Downloads flag definitions once, evaluates flags without a network call per
 * evaluation, and refreshes definitions lazily in the background.
 *
 * Requires a token with app **read** permission (not evaluate). Local evaluations
 * do not appear in server-side analytics.
 */

import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider, LoggingHook } from '@cloudflare/flagship/server';

const FLAGSHIP_APP_ID = 'your-app-id';
const FLAGSHIP_ACCOUNT_ID = 'your-account-id';
const FLAGSHIP_READ_TOKEN = 'your-read-token';

async function main() {
	await OpenFeature.setProviderAndWait(
		new FlagshipServerProvider({
			appId: FLAGSHIP_APP_ID,
			accountId: FLAGSHIP_ACCOUNT_ID,
			authToken: FLAGSHIP_READ_TOKEN,
			localEvaluation: true,
			// Minimum interval between background definition refreshes (default 30s).
			// Refresh is lazy — triggered on evaluate when the snapshot is stale.
			refreshInterval: 30_000,
			logging: true,
		}),
	);

	OpenFeature.addHooks(
		new LoggingHook((message, ...args) => {
			console.log(`[FLAGSHIP] ${message}`, ...args);
		}),
	);

	const client = OpenFeature.getClient();
	const context = {
		targetingKey: 'user-123',
		email: 'user@example.com',
		plan: 'premium',
	};

	const darkMode = await client.getBooleanValue('dark-mode', false, context);
	console.log('Dark mode:', darkMode);

	const details = await client.getBooleanDetails('premium-features', false, context);
	console.log('Premium features:', {
		value: details.value,
		reason: details.reason, // STATIC | TARGETING_MATCH | SPLIT | DEFAULT | DISABLED | …
		variant: details.variant,
	});
}

main().catch(console.error);
