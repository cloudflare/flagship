import { describe, it, expect, vi } from 'vitest';
import { ContextTransformer } from '../src/context.js';

describe('ContextTransformer', () => {
	describe('toQueryParams', () => {
		it('should convert string values', () => {
			const context = {
				targetingKey: 'user-123',
				email: 'user@example.com',
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				targetingKey: 'user-123',
				email: 'user@example.com',
			});
		});

		it('should convert number values to strings', () => {
			const context = {
				age: 25,
				score: 100.5,
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				age: '25',
				score: '100.5',
			});
		});

		it('should convert boolean values to strings', () => {
			const context = {
				isPremium: true,
				hasAccess: false,
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				isPremium: 'true',
				hasAccess: 'false',
			});
		});

		it('should convert Date objects to ISO strings', () => {
			const date = new Date('2024-01-15T10:30:00Z');
			const context = {
				signupDate: date,
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				signupDate: '2024-01-15T10:30:00.000Z',
			});
		});

		it('should skip undefined values', () => {
			const context: Record<string, string | undefined> = {
				key1: 'value1',
				key2: undefined,
			};

			const result = ContextTransformer.toQueryParams(context as any);

			expect(result).toEqual({
				key1: 'value1',
			});
		});

		it('should skip null values', () => {
			const context = {
				key1: 'value1',
				key2: null,
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				key1: 'value1',
			});
		});

		it('should skip complex objects with warning', () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			const context = {
				key1: 'value1',
				nested: { foo: 'bar' },
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				key1: 'value1',
			});
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('Context key "nested" is a complex object/array'));

			consoleSpy.mockRestore();
		});

		it('should handle empty context', () => {
			const result = ContextTransformer.toQueryParams({});
			expect(result).toEqual({});
		});

		it('should handle mixed types', () => {
			const context = {
				targetingKey: 'user-123',
				age: 25,
				isPremium: true,
				signupDate: new Date('2024-01-15T10:30:00Z'),
			};

			const result = ContextTransformer.toQueryParams(context);

			expect(result).toEqual({
				targetingKey: 'user-123',
				age: '25',
				isPremium: 'true',
				signupDate: '2024-01-15T10:30:00.000Z',
			});
		});
	});

	describe('buildUrl', () => {
		it('should build URL with flagKey parameter', () => {
			const baseUrl = 'https://api.example.com/evaluate';
			const flagKey = 'my-flag';
			const context = {};

			const result = ContextTransformer.buildUrl(baseUrl, flagKey, context);

			expect(result).toBe('https://api.example.com/evaluate?flagKey=my-flag');
		});

		it('should build URL with context parameters', () => {
			const baseUrl = 'https://api.example.com/evaluate';
			const flagKey = 'my-flag';
			const context = {
				targetingKey: 'user-123',
				email: 'user@example.com',
			};

			const result = ContextTransformer.buildUrl(baseUrl, flagKey, context);

			const url = new URL(result);
			expect(url.searchParams.get('flagKey')).toBe('my-flag');
			expect(url.searchParams.get('targetingKey')).toBe('user-123');
			expect(url.searchParams.get('email')).toBe('user@example.com');
		});

		it('should handle special characters in values', () => {
			const baseUrl = 'https://api.example.com/evaluate';
			const flagKey = 'my-flag';
			const context = {
				email: 'user+test@example.com',
				message: 'Hello World!',
			};

			const result = ContextTransformer.buildUrl(baseUrl, flagKey, context);

			const url = new URL(result);
			expect(url.searchParams.get('email')).toBe('user+test@example.com');
			expect(url.searchParams.get('message')).toBe('Hello World!');
		});

		it('should preserve base URL path', () => {
			const baseUrl = 'https://api.example.com/api/v1/apps/my-app/evaluate';
			const flagKey = 'my-flag';
			const context = {};

			const result = ContextTransformer.buildUrl(baseUrl, flagKey, context);

			expect(result).toBe('https://api.example.com/api/v1/apps/my-app/evaluate?flagKey=my-flag');
		});

		it('should encode special characters in flagKey', () => {
			const result = ContextTransformer.buildUrl('https://api.example.com/evaluate', 'my flag/name', {});
			const url = new URL(result);
			expect(url.searchParams.get('flagKey')).toBe('my flag/name');
		});

		it('should populate droppedKeys when complex context values are present', () => {
			const dropped: string[] = [];
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			ContextTransformer.buildUrl('https://api.example.com/evaluate', 'my-flag', { nested: { a: 1 } as any }, dropped);

			expect(dropped).toContain('nested');
			expect(consoleSpy).not.toHaveBeenCalled();

			consoleSpy.mockRestore();
		});
	});

	describe('toQueryParams — additional edge cases', () => {
		it('should serialize zero as "0"', () => {
			const result = ContextTransformer.toQueryParams({ count: 0 });
			expect(result.count).toBe('0');
		});

		it('should serialize false as "false"', () => {
			const result = ContextTransformer.toQueryParams({ active: false });
			expect(result.active).toBe('false');
		});

		it('should serialize empty string', () => {
			const result = ContextTransformer.toQueryParams({ label: '' });
			expect(result.label).toBe('');
		});

		it('collects multiple dropped keys when droppedKeys array provided', () => {
			const dropped: string[] = [];
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			const result = ContextTransformer.toQueryParams(
				{
					name: 'Alice',
					meta: { role: 'admin' } as any,
					tags: ['a', 'b'] as any,
				},
				dropped,
			);

			expect(result).toEqual({ name: 'Alice' });
			expect(dropped).toEqual(['meta', 'tags']);
			expect(consoleSpy).not.toHaveBeenCalled();

			consoleSpy.mockRestore();
		});

		it('emits console.warn for each complex key when no droppedKeys collector', () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			ContextTransformer.toQueryParams({ a: { x: 1 } as any, b: [1, 2] as any });

			expect(consoleSpy).toHaveBeenCalledTimes(2);
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('"a"'));
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('"b"'));

			consoleSpy.mockRestore();
		});

		it('includes targetingKey in output like any other string', () => {
			const result = ContextTransformer.toQueryParams({ targetingKey: 'user-42' });
			expect(result.targetingKey).toBe('user-42');
		});
	});
});
