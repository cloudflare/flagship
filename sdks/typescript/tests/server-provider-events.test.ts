import { describe, it, expect, beforeEach, vi } from 'vitest';
import { OpenFeature, ProviderEvents } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';

// Mock fetch globally
global.fetch = vi.fn();

describe('Provider Events', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		OpenFeature.clearHandlers();
		OpenFeature.clearProviders();
	});

	describe('Initialization', () => {
		it('should emit READY event on successful initialization', async () => {
			const readyHandler = vi.fn();

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			OpenFeature.addHandler(ProviderEvents.Ready, readyHandler);

			await OpenFeature.setProviderAndWait(provider);

			expect(readyHandler).toHaveBeenCalled();
			expect(global.fetch).not.toHaveBeenCalled();
		});

		it('should handle initialization without explicit initialize call', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({ flagKey: 'test-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await OpenFeature.setProviderAndWait(provider);

			const client = OpenFeature.getClient();
			const value = await client.getBooleanValue('test-flag', false);

			expect(value).toBe(true);
		});
	});

	describe('Shutdown', () => {
		it('should handle shutdown gracefully', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({ flagKey: 'test-flag', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await OpenFeature.setProviderAndWait(provider);

			await expect(OpenFeature.clearProviders()).resolves.not.toThrow();
		});
	});

	describe('Event Handlers', () => {
		it('should allow adding multiple event handlers', async () => {
			const handler1 = vi.fn();
			const handler2 = vi.fn();
			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			OpenFeature.addHandler(ProviderEvents.Ready, handler1);
			OpenFeature.addHandler(ProviderEvents.Ready, handler2);

			await OpenFeature.setProviderAndWait(provider);

			expect(handler1).toHaveBeenCalled();
			expect(handler2).toHaveBeenCalled();
		});

		it('should allow removing event handlers', async () => {
			const handler = vi.fn();
			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			OpenFeature.addHandler(ProviderEvents.Ready, handler);
			OpenFeature.removeHandler(ProviderEvents.Ready, handler);

			await OpenFeature.setProviderAndWait(provider);

			expect(handler).not.toHaveBeenCalled();
		});
	});

	describe('Integration with OpenFeature', () => {
		it('should work with OpenFeature event system', async () => {
			const providerReadyHandler = vi.fn();

			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({ flagKey: 'test-flag', value: true }),
			});

			OpenFeature.addHandler(ProviderEvents.Ready, providerReadyHandler);

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			expect(providerReadyHandler).toHaveBeenCalled();
		});

		it('should evaluate flags after initialization', async () => {
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
		});
	});
});
