/**
 * Example: Client-side usage (Browser)
 *
 * The FlagshipClientProvider supports synchronous flag resolution in the
 * browser. All flags are evaluated in one bulk request when the evaluation
 * context changes and cached in memory for instant access.
 *
 * NOTE: This is an example file. Type checking may show errors in editors
 * due to dynamic imports, but the code works at runtime.
 */

// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-nocheck

const FLAGSHIP_APP_ID = 'your-app-id';
const FLAGSHIP_ACCOUNT_ID = 'your-account-id';

/**
 * Basic client provider setup with caching
 */
async function basicClientSetup() {
	const { OpenFeature } = await import('@openfeature/web-sdk');
	const { FlagshipClientProvider } = await import('@cloudflare/flagship/web');

	await OpenFeature.setProviderAndWait(
		new FlagshipClientProvider({
			appId: FLAGSHIP_APP_ID,
			accountId: FLAGSHIP_ACCOUNT_ID,
			authToken: 'your-token',
		}),
	);

	// Setting context evaluates all flags
	await OpenFeature.setContext({
		targetingKey: 'user-123',
		email: 'user@example.com',
		plan: 'premium',
		country: 'US',
	});

	const client = OpenFeature.getClient();

	// Flags are resolved synchronously from cache after evaluation
	const darkMode = client.getBooleanValue('dark-mode', false);
	const welcomeMsg = client.getStringValue('welcome-message', 'Welcome!');
	const maxUploads = client.getNumberValue('max-uploads', 5);
	const themeConfig = client.getObjectValue('theme-config', { primaryColor: '#000000', fontSize: 14 });

	console.log('Flags loaded from cache:');
	console.log('- Dark mode:', darkMode);
	console.log('- Welcome message:', welcomeMsg);
	console.log('- Max uploads:', maxUploads);
	console.log('- Theme config:', themeConfig);

	if (darkMode) {
		document.body.classList.add('dark-mode');
	}
	document.getElementById('welcome')!.textContent = welcomeMsg;
}

/**
 * Handling user login — context change triggers re-fetch
 */
async function handleUserLogin() {
	const { OpenFeature } = await import('@openfeature/web-sdk');

	// Updating context automatically re-fetches all pre-configured flags for the new user
	await OpenFeature.setContext({
		targetingKey: 'user-456',
		email: 'newuser@example.com',
		plan: 'free',
		country: 'CA',
	});

	const client = OpenFeature.getClient();
	const darkMode = client.getBooleanValue('dark-mode', false);
	console.log('Dark mode for logged-in user:', darkMode);
}

/**
 * Handling evaluation details — reason reflects how the flag resolved
 */
async function checkEvaluationDetails() {
	const { OpenFeature } = await import('@openfeature/web-sdk');
	const client = OpenFeature.getClient();

	const details = client.getBooleanDetails('dark-mode', false);

	console.log('Evaluation details:');
	console.log('- Value:', details.value);
	console.log('- Reason:', details.reason); // The evaluation reason on success, 'ERROR' on a missing flag or type mismatch
	console.log('- Variant:', details.variant);
	console.log('- Metadata:', details.flagMetadata);

	if (details.errorCode) {
		console.error('Error:', details.errorCode, details.errorMessage);
	}

	switch (details.reason) {
		case 'TARGETING_MATCH':
		case 'SPLIT':
		case 'STATIC':
		case 'DEFAULT':
			console.log(`✓ Flag resolved: ${details.reason}`);
			break;
		case 'DISABLED':
			console.warn('⚠ Flag is disabled, using default value');
			break;
		case 'ERROR':
			console.error('✗ Error resolving flag:', details.errorMessage);
			break;
	}
}

/**
 * Progressive enhancement pattern
 */
async function progressiveEnhancement() {
	const { OpenFeature } = await import('@openfeature/web-sdk');
	const { FlagshipClientProvider } = await import('@cloudflare/flagship/web');

	await OpenFeature.setProviderAndWait(
		new FlagshipClientProvider({
			appId: FLAGSHIP_APP_ID,
			accountId: FLAGSHIP_ACCOUNT_ID,
			authToken: 'your-token',
		}),
	);

	// Anonymous user on initial load
	await OpenFeature.setContext({
		targetingKey: 'anonymous',
		plan: 'free',
	});

	const client = OpenFeature.getClient();

	function updateUI() {
		const premiumFeatures = client.getBooleanValue('premium-features', false);
		const betaAccess = client.getBooleanValue('beta-access', false);

		const premiumElement = document.getElementById('premium-features');
		if (premiumElement) premiumElement.style.display = premiumFeatures ? 'block' : 'none';

		const betaElement = document.getElementById('beta-features');
		if (betaElement) betaElement.style.display = betaAccess ? 'block' : 'none';
	}

	updateUI();

	document.getElementById('login-button')?.addEventListener('click', async () => {
		// On login, update context with real user data — flags are re-fetched automatically
		await OpenFeature.setContext({
			targetingKey: 'user-789',
			email: 'premium@example.com',
			plan: 'premium',
		});
		updateUI();
	});
}

/**
 * Cache hit/miss monitoring
 */
async function cacheMonitoring() {
	const { OpenFeature } = await import('@openfeature/web-sdk');
	const { FlagshipClientProvider } = await import('@cloudflare/flagship/web');

	await OpenFeature.setProviderAndWait(
		new FlagshipClientProvider({
			appId: FLAGSHIP_APP_ID,
			accountId: FLAGSHIP_ACCOUNT_ID,
		}),
	);

	await OpenFeature.setContext({ targetingKey: 'user-123' });

	const client = OpenFeature.getClient();
	const flags = ['flag1', 'flag2', 'flag3', 'missing-flag']; // missing-flag is not in the app — returns FLAG_NOT_FOUND
	const stats = { hits: 0, errors: 0 };

	flags.forEach((flagKey) => {
		const details = client.getBooleanDetails(flagKey, false);
		if (details.errorCode) {
			stats.errors++;
			console.error(`✗ ${details.errorCode}: ${flagKey} - ${details.errorMessage}`);
		} else {
			stats.hits++;
			console.log(`✓ Cache HIT: ${flagKey}`);
		}
	});

	console.log('Cache statistics:', stats);
	console.log(`Cache hit rate: ${((stats.hits / flags.length) * 100).toFixed(1)}%`);
}

/**
 * Production setup with proper error handling
 */
async function productionClientApp() {
	const { OpenFeature } = await import('@openfeature/web-sdk');
	const { FlagshipClientProvider } = await import('@cloudflare/flagship/web');

	try {
		// Initialize provider
		await OpenFeature.setProviderAndWait(
			new FlagshipClientProvider({
				appId: FLAGSHIP_APP_ID,
				accountId: FLAGSHIP_ACCOUNT_ID,
				authToken: 'your-token',
				timeout: 5000,
				retries: 1,
			}),
		);

		// Set user context — evaluates all flags
		await OpenFeature.setContext({
			targetingKey: getCurrentUserId(),
			email: getUserEmail(),
			plan: getUserPlan(),
			country: getUserCountry(),
		});

		const client = OpenFeature.getClient();
		applyDarkMode(client);
		applyWelcomeMessage(client);
		applyUploadLimits(client);
		applyPremiumFeatures(client);
		applyTheme(client);
	} catch (error) {
		console.error('Failed to initialize Flagship:', error);
	}
}

function applyDarkMode(client: any) {
	document.body.classList.toggle('dark-mode', client.getBooleanValue('dark-mode', false));
}

function applyWelcomeMessage(client: any) {
	const element = document.getElementById('welcome');
	if (element) element.textContent = client.getStringValue('welcome-message', 'Welcome!');
}

function applyUploadLimits(client: any) {
	const element = document.getElementById('upload-limit');
	if (element) element.textContent = `Max uploads: ${client.getNumberValue('max-uploads', 5)}`;
}

function applyPremiumFeatures(client: any) {
	const element = document.getElementById('premium-section');
	if (element) element.style.display = client.getBooleanValue('premium-features', false) ? 'block' : 'none';
}

function applyTheme(client: any) {
	const theme = client.getObjectValue('theme-config', { primaryColor: '#007bff', fontSize: 14 });
	document.documentElement.style.setProperty('--primary-color', theme.primaryColor);
	document.documentElement.style.setProperty('--font-size', `${theme.fontSize}px`);
}

function getCurrentUserId(): string {
	return localStorage.getItem('userId') || 'anonymous';
}

function getUserEmail(): string {
	return localStorage.getItem('userEmail') || '';
}

function getUserPlan(): string {
	return localStorage.getItem('userPlan') || 'free';
}

function getUserCountry(): string {
	return 'US';
}

export { basicClientSetup, handleUserLogin, checkEvaluationDetails, progressiveEnhancement, cacheMonitoring, productionClientApp };
