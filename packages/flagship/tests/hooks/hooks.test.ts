import { describe, it, expect, beforeEach, vi } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../../src/server-provider.js';
import { LoggingHook } from '../../src/hooks/logging-hook.js';
import { TelemetryHook, type TelemetryEvent } from '../../src/hooks/telemetry-hook.js';

// Mock fetch globally
global.fetch = vi.fn();

describe('Hooks', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		OpenFeature.clearProviders();
		OpenFeature.clearHooks();
	});

	describe('LoggingHook', () => {
		it('should log flag evaluations', async () => {
			const logMessages: any[] = [];
			const logger = vi.fn((...args: any[]) => logMessages.push(args));

			// First mock for initialization health check
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: '_flagship_health_check',
					value: true,
				}),
			});

			// Second mock for actual flag evaluation
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'test-flag',
					value: true,
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			OpenFeature.addHooks(new LoggingHook(logger));

			const client = OpenFeature.getClient();
			await client.getBooleanValue('test-flag', false, {
				targetingKey: 'user-123',
			});

			// Should have logged before and after
			expect(logger).toHaveBeenCalledTimes(2);

			// Check before log
			expect(logMessages[0][0]).toContain('Evaluating flag: test-flag');

			// Check after log
			expect(logMessages[1][0]).toContain('Flag test-flag evaluated');
		});

		it('should log errors during evaluation', async () => {
			const logMessages: any[] = [];
			const logger = vi.fn((...args: any[]) => logMessages.push(args));

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Second mock for actual flag evaluation (error)
			(global.fetch as any).mockRejectedValueOnce(new Error('Network error'));

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
					retries: 0,
				}),
			);

			OpenFeature.addHooks(new LoggingHook(logger));

			const client = OpenFeature.getClient();
			await client.getBooleanValue('test-flag', false, {
				targetingKey: 'user-123',
			});

			// Should have logged before, error, and after
			expect(logger).toHaveBeenCalled();

			// Check that error was logged
			const errorLog = logMessages.find((msg) => msg[0].includes('Error evaluating flag'));
			expect(errorLog).toBeDefined();
		});

		it('should use default console.log if no logger provided', async () => {
			const consoleSpy = vi.spyOn(console, 'log').mockImplementation(() => undefined);

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Second mock for actual flag evaluation
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'test-flag',
					value: true,
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			OpenFeature.addHooks(new LoggingHook());

			const client = OpenFeature.getClient();
			await client.getBooleanValue('test-flag', false);

			expect(consoleSpy).toHaveBeenCalled();

			consoleSpy.mockRestore();
		});
	});

	describe('TelemetryHook', () => {
		it('should track flag evaluations', async () => {
			const events: TelemetryEvent[] = [];
			const onEvent = vi.fn((event: TelemetryEvent) => events.push(event));

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Second mock for actual flag evaluation
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({
					flagKey: 'test-flag',
					value: true,
					reason: 'TARGETING_MATCH',
					variant: 'enabled',
				}),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			OpenFeature.addHooks(new TelemetryHook(onEvent));

			const client = OpenFeature.getClient();
			await client.getBooleanValue('test-flag', false, {
				targetingKey: 'user-123',
			});

			expect(onEvent).toHaveBeenCalledTimes(1);

			const event = events[0];
			expect(event.type).toBe('evaluation');
			expect(event.flagKey).toBe('test-flag');
			expect(event.value).toBe(true);
			expect(event.reason).toBe('TARGETING_MATCH');
			expect(event.variant).toBe('enabled');
			expect(event.timestamp).toBeDefined();
			expect(event.duration).toBeDefined();
			expect(event.duration).toBeGreaterThanOrEqual(0);
		});

		it('should track errors during evaluation', async () => {
			const events: TelemetryEvent[] = [];
			const onEvent = vi.fn((event: TelemetryEvent) => events.push(event));

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Second mock for actual flag evaluation (error)
			(global.fetch as any).mockRejectedValueOnce(new Error('Network error'));

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
					retries: 0,
				}),
			);

			OpenFeature.addHooks(new TelemetryHook(onEvent));

			const client = OpenFeature.getClient();
			await client.getBooleanValue('test-flag', false, {
				targetingKey: 'user-123',
			});

			// Should have tracked the error
			const errorEvent = events.find((e) => e.type === 'error');
			expect(errorEvent).toBeDefined();
			expect(errorEvent!.flagKey).toBe('test-flag');
			expect(errorEvent!.errorMessage).toContain('Network error');
			expect(errorEvent!.duration).toBeDefined();
		});

		it('should track multiple flag evaluations', async () => {
			const events: TelemetryEvent[] = [];
			const onEvent = vi.fn((event: TelemetryEvent) => events.push(event));

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Mock two successful evaluations
			(global.fetch as any)
				.mockResolvedValueOnce({
					ok: true,
					json: async () => ({ flagKey: 'flag1', value: true }),
				})
				.mockResolvedValueOnce({
					ok: true,
					json: async () => ({ flagKey: 'flag2', value: 'variant-a' }),
				});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			OpenFeature.addHooks(new TelemetryHook(onEvent));

			const client = OpenFeature.getClient();
			await client.getBooleanValue('flag1', false);
			await client.getStringValue('flag2', 'default');

			expect(onEvent).toHaveBeenCalledTimes(2);
			expect(events[0].flagKey).toBe('flag1');
			expect(events[1].flagKey).toBe('flag2');
		});

		it('should include context in telemetry events', async () => {
			const events: TelemetryEvent[] = [];
			const onEvent = vi.fn((event: TelemetryEvent) => events.push(event));

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Second mock for actual evaluation
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'test-flag', value: true }),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			OpenFeature.addHooks(new TelemetryHook(onEvent));

			const client = OpenFeature.getClient();
			const context = {
				targetingKey: 'user-123',
				email: 'user@example.com',
				plan: 'premium',
			};

			await client.getBooleanValue('test-flag', false, context);

			expect(events[0].context).toEqual(context);
		});
	});

	describe('Multiple Hooks', () => {
		it('should execute both logging and telemetry hooks', async () => {
			const logMessages: any[] = [];
			const logger = vi.fn((...args: any[]) => logMessages.push(args));

			const events: TelemetryEvent[] = [];
			const onEvent = vi.fn((event: TelemetryEvent) => events.push(event));

			// First mock for initialization
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: '_flagship_health_check', value: true }),
			});

			// Second mock for actual evaluation
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'test-flag', value: true }),
			});

			await OpenFeature.setProviderAndWait(
				new FlagshipServerProvider({
					endpoint: 'https://api.example.com/evaluate',
				}),
			);

			OpenFeature.addHooks(new LoggingHook(logger), new TelemetryHook(onEvent));

			const client = OpenFeature.getClient();
			await client.getBooleanValue('test-flag', false);

			// Both hooks should have been called
			expect(logger).toHaveBeenCalled();
			expect(onEvent).toHaveBeenCalled();
		});
	});
});
