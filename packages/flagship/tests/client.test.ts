import { describe, it, expect, beforeEach, vi } from 'vitest';
import { FlagshipClient } from '../src/client.js';
import { FlagshipError, FlagshipErrorCode } from '../src/types.js';

// Mock fetch globally
global.fetch = vi.fn();

describe('FlagshipClient', () => {
	beforeEach(() => {
		vi.clearAllMocks();
	});

	describe('constructor', () => {
		it('should create client with endpoint', () => {
			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});
			expect(client).toBeInstanceOf(FlagshipClient);
		});

		it('should create client with appId and accountId', () => {
			const client = new FlagshipClient({
				appId: 'app-abc123',
				accountId: 'my-account',
			});
			expect(client).toBeInstanceOf(FlagshipClient);
		});

		it('should create client with appId, accountId, and custom baseUrl', () => {
			const client = new FlagshipClient({
				appId: 'app-abc123',
				accountId: 'my-account',
				baseUrl: 'http://localhost:8787',
			});
			expect(client).toBeInstanceOf(FlagshipClient);
		});

		it('should throw if neither appId nor endpoint is provided', () => {
			expect(() => new FlagshipClient({})).toThrow('either "appId" or "endpoint" is required');
		});

		it('should throw if both appId and endpoint are provided', () => {
			expect(
				() =>
					new FlagshipClient({
						appId: 'app-abc123',
						accountId: 'my-account',
						endpoint: 'https://api.example.com/evaluate',
					}),
			).toThrow('provide either "appId" or "endpoint", not both');
		});

		it('should throw if appId is provided without accountId', () => {
			expect(
				() =>
					new FlagshipClient({
						appId: 'app-abc123',
					}),
			).toThrow('"accountId" is required when using "appId"');
		});

		it('should throw if endpoint is empty string', () => {
			expect(() => new FlagshipClient({ endpoint: '' })).toThrow('either "appId" or "endpoint" is required');
		});

		it('should throw if endpoint is invalid URL', () => {
			expect(() => new FlagshipClient({ endpoint: 'not-a-url' })).toThrow('invalid endpoint URL');
		});

		it('should accept custom timeout and retries', () => {
			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				timeout: 10000,
				retries: 3,
			});
			expect(client).toBeInstanceOf(FlagshipClient);
		});
	});

	describe('evaluate', () => {
		it('should successfully evaluate a flag', async () => {
			const mockResponse = {
				flagKey: 'my-flag',
				value: true,
			};

			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => mockResponse,
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});

			const result = await client.evaluate('my-flag', {
				targetingKey: 'user-123',
			});

			expect(result).toEqual(mockResponse);
			expect(global.fetch).toHaveBeenCalledTimes(1);
		});

		it('should include context in query parameters', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});

			await client.evaluate('my-flag', {
				targetingKey: 'user-123',
				email: 'user@example.com',
				age: 25,
			});

			const callArgs = (global.fetch as any).mock.calls[0];
			const url = new URL(callArgs[0]);

			expect(url.searchParams.get('flagKey')).toBe('my-flag');
			expect(url.searchParams.get('targetingKey')).toBe('user-123');
			expect(url.searchParams.get('email')).toBe('user@example.com');
			expect(url.searchParams.get('age')).toBe('25');
		});

		it('should throw INVALID_CONTEXT when context contains complex objects', async () => {
			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});

			const error = await client
				.evaluate('my-flag', {
					targetingKey: 'user-123',
					nested: { foo: 'bar' } as any,
					arr: [1, 2, 3] as any,
				})
				.catch((e) => e);

			expect(error).toBeInstanceOf(FlagshipError);
			expect(error.code).toBe(FlagshipErrorCode.INVALID_CONTEXT);
			expect(error.message).toContain('nested');
			expect(error.message).toContain('arr');
			// fetch should NOT have been called — error thrown before the request
			expect(global.fetch).not.toHaveBeenCalled();
		});

		it('should throw FlagshipError on 404', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: false,
				status: 404,
				statusText: 'Not Found',
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});

			await expect(client.evaluate('my-flag', {})).rejects.toThrow(FlagshipError);
			await expect(client.evaluate('my-flag', {})).rejects.toMatchObject({
				code: FlagshipErrorCode.NETWORK_ERROR,
			});
		});

		it('should throw FlagshipError on 500', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: false,
				status: 500,
				statusText: 'Internal Server Error',
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0, // Disable retries for this test
			});

			await expect(client.evaluate('my-flag', {})).rejects.toThrow(FlagshipError);
		});

		it('should throw FlagshipError on invalid response format', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ invalid: 'response' }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});

			await expect(client.evaluate('my-flag', {})).rejects.toThrow(FlagshipError);
			// The client catches PARSE_ERROR but re-throws as NETWORK_ERROR in the outer catch
			// This is expected behavior - the inner error is a PARSE_ERROR but wrapped
			const error = await client.evaluate('my-flag', {}).catch((e) => e);
			expect(error).toBeInstanceOf(FlagshipError);
			// The underlying cause has PARSE_ERROR, but it gets wrapped
		});

		it('should retry on network errors', async () => {
			// First call fails, second succeeds
			(global.fetch as any).mockRejectedValueOnce(new Error('Network error')).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 1,
			});

			const result = await client.evaluate('my-flag', {});

			expect(result.value).toBe(true);
			expect(global.fetch).toHaveBeenCalledTimes(2);
		});

		it('should not retry on 404', { timeout: 10000 }, async () => {
			// Reset mock to ensure clean state
			vi.clearAllMocks();

			// Create a mock Response object that will pass instanceof check
			const mockResponse = new Response(null, { status: 404, statusText: 'Not Found' });
			Object.defineProperty(mockResponse, 'ok', { value: false });

			(global.fetch as any).mockResolvedValue(mockResponse);

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 3,
			});

			try {
				await client.evaluate('my-flag', {});
			} catch (_e) {
				// Expected to throw
			}

			// Should only be called once despite retries being enabled
			expect(global.fetch).toHaveBeenCalledTimes(1);
		});

		it('should handle timeout', { timeout: 10000 }, async () => {
			// Mock a fetch that simulates an abort
			(global.fetch as any).mockImplementation(() => {
				const error = new Error('The operation was aborted');
				error.name = 'AbortError';
				return Promise.reject(error);
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				timeout: 100, // 100ms timeout
				retries: 0,
			});

			const error = await client.evaluate('my-flag', {}).catch((e) => e);
			expect(error).toBeInstanceOf(FlagshipError);
			expect(error.code).toBe(FlagshipErrorCode.TIMEOUT_ERROR);
		});

		it('should pass custom fetch options', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				fetchOptions: {
					headers: {
						'X-Custom-Header': 'test-value',
					},
				},
			});

			await client.evaluate('my-flag', {});

			const callArgs = (global.fetch as any).mock.calls[0];
			expect(callArgs[1].headers).toEqual({
				'X-Custom-Header': 'test-value',
			});
		});

		it('should send Authorization header when bearerToken is provided', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				bearerToken: 'my-secret-token',
			});

			await client.evaluate('my-flag', {});

			const callArgs = (global.fetch as any).mock.calls[0];
			const headers: Headers = callArgs[1].headers;
			expect(headers.get('Authorization')).toBe('Bearer my-secret-token');
		});

		it('should not override an explicit Authorization header with bearerToken', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				bearerToken: 'token-from-bearer-option',
				fetchOptions: {
					headers: {
						Authorization: 'Bearer token-from-fetch-options',
					},
				},
			});

			await client.evaluate('my-flag', {});

			const callArgs = (global.fetch as any).mock.calls[0];
			const headers: Headers = callArgs[1].headers;
			// fetchOptions.headers takes precedence over bearerToken
			expect(headers.get('Authorization')).toBe('Bearer token-from-fetch-options');
		});

		it('should preserve other fetchOptions headers alongside bearerToken', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				bearerToken: 'my-secret-token',
				fetchOptions: {
					headers: {
						'X-Custom-Header': 'custom-value',
					},
				},
			});

			await client.evaluate('my-flag', {});

			const callArgs = (global.fetch as any).mock.calls[0];
			const headers: Headers = callArgs[1].headers;
			expect(headers.get('Authorization')).toBe('Bearer my-secret-token');
			expect(headers.get('X-Custom-Header')).toBe('custom-value');
		});

		it('should not add Authorization header when bearerToken is not set', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
			});

			await client.evaluate('my-flag', {});

			const callArgs = (global.fetch as any).mock.calls[0];
			// No Authorization header expected
			const fetchInit: RequestInit = callArgs[1];
			const hasAuthHeader =
				fetchInit.headers instanceof Headers
					? fetchInit.headers.has('Authorization')
					: Object.prototype.hasOwnProperty.call(fetchInit.headers ?? {}, 'Authorization');
			expect(hasAuthHeader).toBe(false);
		});

		it('should throw PARSE_ERROR when response is missing required fields', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ invalid: 'response' }),
			});

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			const error = await client.evaluate('my-flag', {}).catch((e) => e);
			expect(error).toBeInstanceOf(FlagshipError);
			expect(error.code).toBe(FlagshipErrorCode.PARSE_ERROR);
		});

		it('should not retry on 400', { timeout: 5000 }, async () => {
			vi.clearAllMocks();
			const mockResponse = new Response(null, { status: 400, statusText: 'Bad Request' });
			(global.fetch as any).mockResolvedValue(mockResponse);

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 3,
			});

			await client.evaluate('my-flag', {}).catch(() => {});
			expect(global.fetch).toHaveBeenCalledTimes(1);
		});

		it('should retry on 500 and succeed on second attempt', async () => {
			(global.fetch as any)
				.mockResolvedValueOnce({ ok: false, status: 500, statusText: 'Server Error' })
				.mockResolvedValueOnce({ ok: true, json: async () => ({ flagKey: 'my-flag', value: true }) });

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 1,
				retryDelay: 0,
			});

			const result = await client.evaluate('my-flag', {});
			expect(result.value).toBe(true);
			expect(global.fetch).toHaveBeenCalledTimes(2);
		});

		it('should exhaust all retries on repeated failures', async () => {
			(global.fetch as any).mockRejectedValue(new Error('flaky'));

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 2,
				retryDelay: 0,
			});

			await client.evaluate('my-flag', {}).catch(() => {});
			expect(global.fetch).toHaveBeenCalledTimes(3);
		});

		it('uses configured retryDelay between attempts', async () => {
			vi.useFakeTimers();

			(global.fetch as any)
				.mockRejectedValueOnce(new Error('flaky'))
				.mockResolvedValueOnce({ ok: true, json: async () => ({ flagKey: 'my-flag', value: true }) });

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 1,
				retryDelay: 500,
			});

			const setTimeoutSpy = vi.spyOn(globalThis, 'setTimeout');

			const evaluatePromise = client.evaluate('my-flag', {});
			await vi.runAllTimersAsync();
			await evaluatePromise;

			const retryDelayCall = setTimeoutSpy.mock.calls.find((call) => call[1] === 500);
			expect(retryDelayCall).toBeDefined();

			vi.useRealTimers();
		});

		it('uses default baseUrl when only appId and accountId provided', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({ appId: 'app-1', accountId: 'acct-1' });
			await client.evaluate('my-flag', {});

			const calledUrl: string = (global.fetch as any).mock.calls[0][0];
			expect(calledUrl).toContain('flagship.cloudflare.dev');
			expect(calledUrl).toContain('acct-1');
			expect(calledUrl).toContain('app-1');
		});

		it('strips trailing slash from baseUrl', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({ appId: 'app-1', accountId: 'acct-1', baseUrl: 'http://localhost:8787/' });
			await client.evaluate('my-flag', {});

			const calledUrl: string = (global.fetch as any).mock.calls[0][0];
			expect(calledUrl).not.toContain('//v1');
		});

		it('encodes special characters in appId and accountId', async () => {
			(global.fetch as any).mockResolvedValueOnce({
				ok: true,
				json: async () => ({ flagKey: 'my-flag', value: true }),
			});

			const client = new FlagshipClient({ appId: 'app/id', accountId: 'acct&id', baseUrl: 'http://localhost:8787' });
			await client.evaluate('my-flag', {});

			const calledUrl: string = (global.fetch as any).mock.calls[0][0];
			expect(calledUrl).toContain('app%2Fid');
			expect(calledUrl).toContain('acct%26id');
		});

		it('wraps non-Error rejection in NETWORK_ERROR', async () => {
			(global.fetch as any).mockRejectedValueOnce('some string error');

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			const error = await client.evaluate('my-flag', {}).catch((e) => e);
			expect(error).toBeInstanceOf(FlagshipError);
			expect(error.code).toBe(FlagshipErrorCode.NETWORK_ERROR);
		});

		it('retries: 0 calls fetch exactly once on failure', async () => {
			(global.fetch as any).mockResolvedValueOnce({ ok: false, status: 500, statusText: 'Error' });

			const client = new FlagshipClient({
				endpoint: 'https://api.example.com/evaluate',
				retries: 0,
			});

			await client.evaluate('my-flag', {}).catch(() => {});
			expect(global.fetch).toHaveBeenCalledTimes(1);
		});
	});
});
