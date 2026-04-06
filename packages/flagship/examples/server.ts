/**
 * Example: Using Flagship with OpenFeature Server SDK
 *
 * Demonstrates server-side flag evaluation for Node.js, Cloudflare Workers,
 * or other server-side JavaScript environments.
 */

import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider, LoggingHook } from '@cloudflare/flagship/server';

const FLAGSHIP_APP_ID = 'your-app-id';
const FLAGSHIP_ACCOUNT_ID = 'your-account-id';

async function main() {
	// 1. Set up the Flagship provider
	await OpenFeature.setProviderAndWait(
		new FlagshipServerProvider({
			appId: FLAGSHIP_APP_ID,
			accountId: FLAGSHIP_ACCOUNT_ID,
			timeout: 5000,
			retries: 1,
		}),
	);

	// 2. Add hooks for logging
	OpenFeature.addHooks(
		new LoggingHook((message, ...args) => {
			console.log(`[FLAGSHIP] ${message}`, ...args);
		}),
	);

	// 3. Get a client and evaluate flags with context
	const client = OpenFeature.getClient();

	const context = {
		targetingKey: 'user-123', // user identifier for targeting rules
		email: 'user@example.com',
		plan: 'premium',
		country: 'US',
		age: 25,
	};

	// Boolean flag
	const darkModeEnabled = await client.getBooleanValue('dark-mode', false, context);
	console.log('Dark mode enabled:', darkModeEnabled);

	// String flag — e.g. for A/B testing copy
	const welcomeMessage = await client.getStringValue('welcome-message', 'Welcome!', context);
	console.log('Welcome message:', welcomeMessage);

	// Number flag — e.g. for feature limits
	const maxUploads = await client.getNumberValue('max-uploads', 5, context);
	console.log('Max uploads:', maxUploads);

	// Object flag — e.g. for complex configuration
	const themeConfig = await client.getObjectValue('theme-config', { primaryColor: '#000000', fontSize: 14 }, context);
	console.log('Theme config:', themeConfig);

	// 4. Detailed evaluation — reason reflects how the flag resolved
	// ('TARGETING_MATCH', 'DEFAULT', 'DISABLED', 'SPLIT')
	const details = await client.getBooleanDetails('premium-features', false, context);
	console.log('Premium features details:', {
		value: details.value,
		reason: details.reason, // why this value was served
		variant: details.variant, // which variation key was matched
		flagMetadata: details.flagMetadata,
	});

	// 5. Non-existent flags return the default value — no exceptions thrown
	const unknownFlag = await client.getBooleanValue('non-existent-flag', false, context);
	console.log('Unknown flag (returns default):', unknownFlag);

	// 6. Evaluate without context — rules that don't require targeting still apply
	const betaAccess = await client.getBooleanValue('beta-access', false);
	console.log('Beta access:', betaAccess);
}

main().catch(console.error);
