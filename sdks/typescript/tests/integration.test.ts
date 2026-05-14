import { describe, it, expect, beforeEach, vi } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';

/**
 * Integration tests for Flagship OpenFeature Provider
 *
 * These tests verify the full integration with the OpenFeature SDK
 * and mock the backend API responses.
 */

// Mock fetch globally
global.fetch = vi.fn();

describe('Flagship Integration Tests', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		// Clear any previous providers
		OpenFeature.clearProviders();
	});

	describe('Server Provider Integration', () => {
		it('should integrate with OpenFeature server SDK', async () => {
			// Mock API response
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'feature-flag',
					value: true,
				}),
			});

			// Set up provider
			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();

			// Evaluate flag
			const value = await client.getBooleanValue('feature-flag', false, {
				targetingKey: 'user-123',
				email: 'user@example.com',
			});

			expect(value).toBe(true);
			expect(global.fetch).toHaveBeenCalledTimes(2); // Init + evaluation

			// Verify the URL contains context (second call is the actual evaluation)
			const callArgs = (global.fetch as any).mock.calls[1];
			const url = new URL(callArgs[0]);
			expect(url.searchParams.get('flagKey')).toBe('feature-flag');
			expect(url.searchParams.get('targetingKey')).toBe('user-123');
			expect(url.searchParams.get('email')).toBe('user@example.com');
		});

		it('should handle string flags with OpenFeature', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'welcome-message',
					value: 'Hello Premium User!',
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();
			const value = await client.getStringValue('welcome-message', 'Hello User!', {
				targetingKey: 'user-123',
				plan: 'premium',
			});

			expect(value).toBe('Hello Premium User!');
		});

		it('should handle number flags with OpenFeature', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'max-items',
					value: 100,
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();
			const value = await client.getNumberValue('max-items', 10, {
				targetingKey: 'user-123',
			});

			expect(value).toBe(100);
		});

		it('should handle object flags with OpenFeature', async () => {
			const configValue = {
				theme: 'dark',
				language: 'en',
				features: ['feature1', 'feature2'],
			};

			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'user-config',
					value: configValue,
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();
			const value = await client.getObjectValue(
				'user-config',
				{},
				{
					targetingKey: 'user-123',
				},
			);

			expect(value).toEqual(configValue);
		});

		it('should return default value on flag not found', async () => {
			const mockResponse = new Response(null, { status: 404, statusText: 'Not Found' });
			Object.defineProperty(mockResponse, 'ok', { value: false });

			(global.fetch as any).mockResolvedValueOnce(mockResponse);

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
					retries: 0,
				}),
			);

			const client = OpenFeature.getClient();
			const value = await client.getBooleanValue('non-existent-flag', false, {
				targetingKey: 'user-123',
			});

			expect(value).toBe(false); // Default value
		});

		it('should return default value on type mismatch', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'boolean-flag',
					value: 'not-a-boolean', // Wrong type
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();
			const value = await client.getBooleanValue('boolean-flag', false, {
				targetingKey: 'user-123',
			});

			expect(value).toBe(false); // Default value
		});

		it('flagMetadata is always empty (API does not return metadata)', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'premium-feature',
					value: true,
					reason: 'TARGETING_MATCH',
					variant: 'premium-enabled',
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();
			const details = await client.getBooleanDetails('premium-feature', false, {
				targetingKey: 'user-123',
			});

			expect(details.value).toBe(true);
			expect(details.reason).toBe('TARGETING_MATCH');
			expect(details.variant).toBe('premium-enabled');
			expect(details.flagMetadata).toEqual({});
		});

		it('should handle multiple sequential evaluations', async () => {
			// Mock all responses
			(global.fetch as any)
				.mockResolvedValueOnce({
					ok: true,
					json: async () => ({ flagKey: '_flagship_health_check', value: true }),
				})
				.mockResolvedValueOnce({
					ok: true,
					json: async () => ({ flagKey: 'flag1', value: true }),
				})
				.mockResolvedValueOnce({
					ok: true,
					json: async () => ({ flagKey: 'flag2', value: 'variant-b' }),
				});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();

			const value1 = await client.getBooleanValue('flag1', false, {
				targetingKey: 'user-123',
			});

			const value2 = await client.getStringValue('flag2', 'variant-a', {
				targetingKey: 'user-123',
			});

			expect(value1).toBe(true);
			expect(value2).toBe('variant-b');
			expect(global.fetch).toHaveBeenCalledTimes(3); // Init + 2 evaluations
		});

		it('should work with empty context', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'default-feature',
					value: true,
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			const client = OpenFeature.getClient();
			const value = await client.getBooleanValue('default-feature', false);

			expect(value).toBe(true);

			// Verify URL only has flagKey (second call is the actual evaluation)
			const callArgs = (global.fetch as any).mock.calls[1];
			const url = new URL(callArgs[0]);
			expect(url.searchParams.get('flagKey')).toBe('default-feature');
			expect(url.searchParams.get('targetingKey')).toBeNull();
		});

		it('should pass custom headers from fetchOptions', async () => {
			(global.fetch as any).mockResolvedValue({
				ok: true,
				json: async () => ({
					flagKey: 'feature',
					value: true,
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
					fetchOptions: {
						headers: {
							'X-API-Key': 'secret-key',
							'X-Custom-Header': 'custom-value',
						},
					},
				}),
			);

			const client = OpenFeature.getClient();
			await client.getBooleanValue('feature', false, {
				targetingKey: 'user-123',
			});

			const callArgs = (global.fetch as any).mock.calls[0];
			expect(callArgs[1].headers).toEqual({
				'X-API-Key': 'secret-key',
				'X-Custom-Header': 'custom-value',
			});
		});
	});
});
