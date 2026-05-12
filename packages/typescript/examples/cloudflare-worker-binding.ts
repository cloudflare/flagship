/**
 * Example: Using Flagship in a Cloudflare Worker via wrangler binding
 *
 * This is the recommended approach for Cloudflare Workers. The Flagship
 * binding communicates directly with the Flagship service via workerd RPC —
 * no HTTP overhead, no auth tokens, no account IDs needed.
 *
 * Configure the binding in wrangler.json:
 *
 * ```jsonc
 * {
 *   "flagship": [
 *     { "binding": "FLAGS", "app_id": "<your-app-id>" }
 *   ]
 * }
 * ```
 */

import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';
import type { FlagshipBinding } from '@cloudflare/flagship/server';

interface Env {
	FLAGS: FlagshipBinding;
}

export default {
	async fetch(request: Request, env: Env): Promise<Response> {
		// Initialize OpenFeature with the Flagship binding — that's it!
		// No appId, accountId, or authToken required.
		await OpenFeature.setProviderAndWait(new FlagshipServerProvider({ binding: env.FLAGS }));

		// Extract user information from request
		const url = new URL(request.url);
		const userId = url.searchParams.get('userId') || 'anonymous';
		const country = request.headers.get('cf-ipcountry') || 'unknown';

		// Build evaluation context from request
		const context = {
			targetingKey: userId,
			country: country,
			path: url.pathname,
		};

		// Get OpenFeature client
		const client = OpenFeature.getClient();

		// Evaluate feature flags — these go directly through the binding,
		// no HTTP requests are made.
		const darkModeEnabled = await client.getBooleanValue('dark-mode', false, context);
		const welcomeMessage = await client.getStringValue('welcome-message', 'Welcome!', context);
		const maxUploads = await client.getNumberValue('max-uploads', 5, context);

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
			headers: { 'content-type': 'application/json' },
		});
	},
};
