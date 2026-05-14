/**
 * Example: Using Flagship in a Cloudflare Worker
 *
 * This example shows how to integrate Flagship feature flags
 * into a Cloudflare Worker to personalize responses.
 */

import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

const FLAGSHIP_APP_ID = 'your-app-id';
const FLAGSHIP_ACCOUNT_ID = 'your-account-id';

let isInitialized = false;

async function initializeOpenFeature() {
	if (!isInitialized) {
		await OpenFeature.setProviderAndWait(
			new FlagshipServerProvider({
				appId: FLAGSHIP_APP_ID,
				accountId: FLAGSHIP_ACCOUNT_ID,
			}),
		);
		isInitialized = true;
	}
}

export default {
	async fetch(request: Request): Promise<Response> {
		// Initialize OpenFeature (only happens once)
		await initializeOpenFeature();

		// Extract user information from request
		const url = new URL(request.url);
		const userId = url.searchParams.get('userId') || 'anonymous';
		const country = request.headers.get('cf-ipcountry') || 'unknown';

		// Build evaluation context from request
		const context = {
			targetingKey: userId,
			country: country,
			path: url.pathname,
			userAgent: request.headers.get('user-agent') || '',
		};

		// Get OpenFeature client
		const client = OpenFeature.getClient();

		// Evaluate feature flags
		const darkModeEnabled = await client.getBooleanValue('dark-mode', false, context);

		const welcomeMessage = await client.getStringValue('welcome-message', 'Welcome!', context);

		const maxUploads = await client.getNumberValue('max-uploads', 5, context);

		// Use flags to customize response
		const response = {
			message: welcomeMessage,
			features: {
				darkMode: darkModeEnabled,
				maxUploads: maxUploads,
			},
			user: {
				id: userId,
				country: country,
			},
		};

		return new Response(JSON.stringify(response, null, 2), {
			headers: {
				'content-type': 'application/json',
			},
		});
	},
};
