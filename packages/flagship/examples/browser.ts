/**
 * Example: Client-side usage (Browser)
 *
 * The FlagshipClientProvider implements caching to support synchronous flag
 * resolution in the browser. Flags are pre-fetched when the evaluation context
 * changes and cached in memory for instant access.
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
			prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads', 'theme-config'],
			cacheTTL: 60000,
		}),
	);

	// Setting context triggers pre-fetching of the configured flags
	await OpenFeature.setContext({
		targetingKey: 'user-123',
		email: 'user@example.com',
		plan: 'premium',
		country: 'US',
	});

	const client = OpenFeature.getClient();

	// Flags are resolved synchronously from cache after pre-fetch
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
	console.log('- Reason:', details.reason); // 'CACHED', 'DEFAULT', or 'ERROR'
	console.log('- Variant:', details.variant);
	console.log('- Metadata:', details.flagMetadata);

	if (details.errorCode) {
		console.error('Error:', details.errorCode, details.errorMessage);
	}

	switch (details.reason) {
		case 'CACHED':
			console.log('✓ Flag resolved from cache');
			break;
		case 'DEFAULT':
			console.warn('⚠ Flag not in cache, using default value');
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
			prefetchFlags: ['premium-features', 'beta-access'],
			cacheTTL: 300000,
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
			prefetchFlags: ['flag1', 'flag2', 'flag3'],
		}),
	);

	await OpenFeature.setContext({ targetingKey: 'user-123' });

	const client = OpenFeature.getClient();
	const flags = ['flag1', 'flag2', 'flag3', 'flag4']; // flag4 not pre-fetched
	const stats = { hits: 0, misses: 0, errors: 0 };

	flags.forEach((flagKey) => {
		const details = client.getBooleanDetails(flagKey, false);
		switch (details.reason) {
			case 'CACHED':
				stats.hits++;
				console.log(`✓ Cache HIT: ${flagKey}`);
				break;
			case 'DEFAULT':
				stats.misses++;
				console.warn(`⚠ Cache MISS: ${flagKey}`);
				break;
			case 'ERROR':
				stats.errors++;
				console.error(`✗ Cache ERROR: ${flagKey} - ${details.errorMessage}`);
				break;
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
		// Initialize provider and pre-fetch flags
		await OpenFeature.setProviderAndWait(
			new FlagshipClientProvider({
				appId: FLAGSHIP_APP_ID,
				accountId: FLAGSHIP_ACCOUNT_ID,
				prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads', 'premium-features', 'beta-access', 'theme-config'],
				cacheTTL: 60000,
				timeout: 5000,
				retries: 1,
			}),
		);

		// Set user context — triggers flag pre-fetch
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
