import { describe, it, expect, beforeEach, vi } from 'vitest';
import { OpenFeature, ProviderEvents } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';

global.fetch = vi.fn();

describe('Provider Events', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		OpenFeature.clearProviders();
	});

	it('emits READY through OpenFeature without an initialization request', async () => {
		const providerReadyHandler = vi.fn();
		OpenFeature.addHandler(ProviderEvents.Ready, providerReadyHandler);

		await OpenFeature.setProviderAndWait(
			new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			}),
		);

		expect(providerReadyHandler).toHaveBeenCalled();
		expect(global.fetch).not.toHaveBeenCalled();
	});

	it('evaluates flags after initialization', async () => {
		(global.fetch as any).mockResolvedValue({
			ok: true,
			json: async () => ({ flagKey: 'test-flag', value: true }),
		});

		await OpenFeature.setProviderAndWait(
			new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			}),
		);

		const client = OpenFeature.getClient();
		const value = await client.getBooleanValue('test-flag', false);

		expect(value).toBe(true);
		expect(global.fetch).toHaveBeenCalledTimes(1);
	});

	it('handles shutdown gracefully', async () => {
		const provider = new FlagshipServerProvider({
			endpoint: 'https://api.example.com/evaluate',
		});

		await OpenFeature.setProviderAndWait(provider);

		await expect(provider.onClose()).resolves.toBeUndefined();
	});
});
