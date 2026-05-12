import { describe, it, expect, beforeEach, vi } from 'vitest';
import { OpenFeature, ProviderEvents, ProviderStatus } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';

// Mock fetch globally
global.fetch = vi.fn();

describe('Provider Events', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		OpenFeature.clearProviders();
	});

	describe('Initialization', () => {
		it('should emit READY event on successful initialization', async () => {
			const readyHandler = vi.fn();

			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			provider.events.addHandler(ProviderEvents.Ready, readyHandler);

			await OpenFeature.setProviderAndWait(provider);

			expect(readyHandler).toHaveBeenCalled();
		});

		it('should emit ERROR event on network failure during initialization', async () => {
			const errorHandler = vi.fn();

			(global.fetch as any).mockRejectedValueOnce(new Error('Network error'));

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			provider.events.addHandler(ProviderEvents.Error, errorHandler);

			await OpenFeature.setProviderAndWait(provider);

			expect(errorHandler).toHaveBeenCalled();
			expect(provider.status).toBe(ProviderStatus.ERROR);
		});

		it('should emit READY event when health check returns 404 (endpoint reachable)', async () => {
			const readyHandler = vi.fn();

			// Must use a real Response instance so `instanceof Response` succeeds
			// in the 404 detection path inside initialize().
			const mockResponse = new Response(null, { status: 404, statusText: 'Not Found' });
			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			provider.events.addHandler(ProviderEvents.Ready, readyHandler);

			await OpenFeature.setProviderAndWait(provider);

			expect(readyHandler).toHaveBeenCalled();
			expect(provider.status).toBe(ProviderStatus.READY);
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

			// Shutdown should not throw
			await expect(provider.onClose()).resolves.not.toThrow();
		});
	});

	describe('Provider Status', () => {
		it('should start with NOT_READY status', () => {
			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			expect(provider.status).toBeDefined();
		});

		it('should be READY after initialization', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			await OpenFeature.setProviderAndWait(provider);

			expect(provider.status).toBeDefined();
		});
	});

	describe('Event Handlers', () => {
		it('should allow adding multiple event handlers', async () => {
			const handler1 = vi.fn();
			const handler2 = vi.fn();

			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			provider.events.addHandler(ProviderEvents.Ready, handler1);
			provider.events.addHandler(ProviderEvents.Ready, handler2);

			await OpenFeature.setProviderAndWait(provider);

			expect(handler1).toHaveBeenCalled();
			expect(handler2).toHaveBeenCalled();
		});

		it('should allow removing event handlers', async () => {
			const handler = vi.fn();

			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			const provider = new FlagshipServerProvider({
				endpoint: 'https://api.example.com/evaluate',
			});

			provider.events.addHandler(ProviderEvents.Ready, handler);
			provider.events.removeHandler(ProviderEvents.Ready, handler);

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
