import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Logger } from '@openfeature/server-sdk';
import { ErrorCode } from '@openfeature/server-sdk';
import { FlagshipClient, resolveDefinitionsEndpoint } from '../src/client.js';
import { evaluateFlag, FlagConfigError, murmurhash3, type EvalFlag } from '../src/local/evaluate.js';
import { FlagshipServerProvider } from '../src/server-provider.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const casesPath = resolve(__dirname, '../../../spec/local-evaluation/cases.json');
const suite = JSON.parse(readFileSync(casesPath, 'utf8')) as {
	cases: Array<{
		name: string;
		definitions: { flags: Record<string, EvalFlag> };
		accountId: string;
		flagKey: string;
		context: Record<string, unknown>;
		expected: { value: unknown; variant: string; reason: string } | { error: string } | { reasonOneOf: string[] };
	}>;
	murmur: Array<{ name: string; input: string; seed: number; expected: number }>;
};

const noopLogger: Logger = {
	debug: () => {},
	info: () => {},
	warn: () => {},
	error: () => {},
};

describe('local evaluation engine — JS edge cases', () => {
	it('orders Infinity via Number()', () => {
		const flagDef: EvalFlag = {
			key: 'inf',
			enabled: true,
			default_variation: 'none',
			variations: { none: 'none', hit: 'hit' },
			rules: [{ conditions: [{ attribute: 'v', operator: 'greater_than', value: 1 }], serve_variation: 'hit' }],
		};
		expect(evaluateFlag(flagDef, { v: Infinity }, 'acct')).toMatchObject({ reason: 'TARGETING_MATCH', value: 'hit' });
	});
});

describe('local evaluation engine — shared vectors', () => {
	for (const c of suite.cases) {
		it(c.name, () => {
			const flagDef = c.definitions.flags[c.flagKey];
			expect(flagDef).toBeDefined();

			if ('error' in c.expected) {
				expect(() => evaluateFlag(flagDef!, c.context as never, c.accountId)).toThrow(FlagConfigError);
				return;
			}

			if ('reasonOneOf' in c.expected) {
				const result = evaluateFlag(flagDef!, c.context as never, c.accountId);
				expect(c.expected.reasonOneOf).toContain(result.reason);
				return;
			}

			const result = evaluateFlag(flagDef!, c.context as never, c.accountId);
			expect(result).toEqual({
				value: c.expected.value,
				variant: c.expected.variant,
				reason: c.expected.reason,
			});
		});
	}

	for (const m of suite.murmur) {
		it(m.name, () => {
			expect(murmurhash3(m.input, m.seed) >>> 0).toBe(m.expected);
		});
	}
});

describe('resolveDefinitionsEndpoint', () => {
	it('builds definitions URL from appId + accountId', () => {
		expect(
			resolveDefinitionsEndpoint({
				appId: 'app-1',
				accountId: 'acct-1',
				baseUrl: 'https://api.example.com',
			}),
		).toBe('https://api.example.com/client/v4/accounts/acct-1/flagship/apps/app-1/definitions');
	});

	it('replaces trailing /evaluate on endpoint', () => {
		expect(
			resolveDefinitionsEndpoint({
				endpoint: 'https://api.example.com/v1/acct/apps/app/evaluate',
			}),
		).toBe('https://api.example.com/v1/acct/apps/app/definitions');
	});

	it('rejects endpoint that does not end in /evaluate', () => {
		expect(() => resolveDefinitionsEndpoint({ endpoint: 'https://api.example.com/v1/flags' })).toThrow(/must end in "\/evaluate"/);
	});
});

describe('FlagshipClient.fetchDefinitions', () => {
	beforeEach(() => {
		vi.restoreAllMocks();
	});

	it('returns flags and etag on 200', async () => {
		const fetch = vi.fn().mockResolvedValue({
			ok: true,
			status: 200,
			headers: new Headers({ etag: '"abc"' }),
			json: async () => ({ flags: { f1: { key: 'f1' } } }),
		});

		const client = new FlagshipClient({
			appId: 'app-1',
			accountId: 'acct-1',
			fetch,
		});

		const result = await client.fetchDefinitions();
		expect(result).toEqual({ etag: '"abc"', flags: { f1: { key: 'f1' } } });
		expect(fetch).toHaveBeenCalledOnce();
		const [url, init] = fetch.mock.calls[0]!;
		expect(String(url)).toContain('/definitions');
		expect(init.method).toBe('GET');
	});

	it('throws a configuration error when the definitions endpoint cannot be derived', async () => {
		const client = new FlagshipClient({ endpoint: 'https://api.example.com/flags', fetch: vi.fn() });
		await expect(client.fetchDefinitions()).rejects.toThrow(/definitions endpoint is not configured/);
	});

	it('sends If-None-Match and returns not-modified on 304', async () => {
		const fetch = vi.fn().mockResolvedValue({
			ok: false,
			status: 304,
			statusText: 'Not Modified',
			headers: new Headers({ etag: '"abc"' }),
		});

		const client = new FlagshipClient({
			appId: 'app-1',
			accountId: 'acct-1',
			fetch,
		});

		const result = await client.fetchDefinitions('"abc"');
		expect(result).toBe('not-modified');
		const init = fetch.mock.calls[0]![1] as RequestInit;
		expect(new Headers(init.headers).get('If-None-Match')).toBe('"abc"');
	});

	it('rejects malformed definitions payload', async () => {
		const fetch = vi.fn().mockResolvedValue({
			ok: true,
			status: 200,
			headers: new Headers(),
			json: async () => ({ notFlags: true }),
		});

		const client = new FlagshipClient({
			appId: 'app-1',
			accountId: 'acct-1',
			fetch,
		});

		await expect(client.fetchDefinitions()).rejects.toMatchObject({ code: 'PARSE_ERROR' });
	});
});

describe('FlagshipServerProvider local evaluation', () => {
	const definitions = {
		flags: {
			bool_flag: {
				key: 'bool_flag',
				enabled: true,
				default_variation: 'off',
				variations: { on: true, off: false },
				rules: [{ conditions: [{ attribute: 'plan', operator: 'equals', value: 'pro' }], serve_variation: 'on' }],
			},
			string_flag: {
				key: 'string_flag',
				enabled: true,
				default_variation: 'a',
				variations: { a: 'hello', b: 'world' },
				rules: [],
			},
			disabled_flag: {
				key: 'disabled_flag',
				enabled: false,
				default_variation: 'off',
				variations: { on: true, off: false },
				rules: [],
			},
			bad_flag: {
				key: 'bad_flag',
				enabled: true,
				default_variation: 'missing',
				variations: { on: true },
				rules: [],
			},
		} satisfies Record<string, EvalFlag>,
	};

	function mockFetch(handlers: {
		definitions?: () => Promise<Response> | Response;
		onDefinitions?: (request: RequestInit & { url: string }) => void;
	}) {
		return vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
			const href = String(url);
			if (href.includes('/definitions')) {
				handlers.onDefinitions?.({ ...init, url: href });
				if (handlers.definitions) return handlers.definitions();
				return {
					ok: true,
					status: 200,
					headers: new Headers({ etag: '"v1"' }),
					json: async () => definitions,
				};
			}
			throw new Error(`unexpected fetch: ${href}`);
		});
	}

	beforeEach(() => {
		vi.useRealTimers();
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.restoreAllMocks();
	});

	describe('constructor', () => {
		it('throws when localEvaluation lacks accountId', () => {
			expect(
				() =>
					new FlagshipServerProvider({
						endpoint: 'https://api.example.com/v1/a/apps/b/evaluate',
						localEvaluation: true,
					}),
			).toThrow(/accountId/);
		});

		it('throws when localEvaluation + cacheTtl', () => {
			expect(
				() =>
					new FlagshipServerProvider({
						appId: 'app-1',
						accountId: 'acct-1',
						localEvaluation: true,
						cacheTtl: 1000,
					}),
			).toThrow(/cacheTtl/);
		});

		it('throws when localEvaluation + binding', () => {
			expect(
				() =>
					new FlagshipServerProvider({
						binding: {} as never,
						localEvaluation: true,
					}),
			).toThrow(/binding/);
		});

		it.each([0, -1, NaN, Infinity])('throws on invalid refreshInterval %s', (refreshInterval) => {
			expect(() => new FlagshipServerProvider({ appId: 'a', accountId: 'b', localEvaluation: true, refreshInterval })).toThrow(
				/refreshInterval/,
			);
		});

		it('throws when endpoint cannot derive definitions URL', () => {
			expect(
				() =>
					new FlagshipServerProvider({
						endpoint: 'https://api.example.com/flags',
						accountId: 'acct-1',
						localEvaluation: true,
					}),
			).toThrow(/\/evaluate/);
		});
	});

	describe('initialize', () => {
		it('loads definitions on success', async () => {
			const fetch = mockFetch({});
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
			});

			await provider.initialize();
			expect(fetch).toHaveBeenCalledOnce();

			const result = await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			expect(result).toMatchObject({ value: true, reason: 'TARGETING_MATCH', variant: 'on' });
		});

		it('does not send If-None-Match on the next refresh when the response had no ETag', async () => {
			vi.useFakeTimers();
			const etags: Array<string | null> = [];
			const fetch = mockFetch({
				definitions: () => ({ ok: true, status: 200, headers: new Headers(), json: async () => definitions }) as Response,
				onDefinitions: (req) => etags.push(new Headers(req.headers).get('If-None-Match')),
			});
			const provider = new FlagshipServerProvider({ appId: 'a', accountId: 'b', localEvaluation: true, fetch, refreshInterval: 10 });
			await provider.initialize();
			await vi.advanceTimersByTimeAsync(10);
			await provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			await vi.advanceTimersByTimeAsync(0);
			expect(etags).toEqual([null, null]);
		});

		it('throws on failure', async () => {
			const fetch = mockFetch({
				definitions: () =>
					Promise.resolve({
						ok: false,
						status: 500,
						statusText: 'err',
						headers: new Headers(),
					} as Response),
			});
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				retries: 0,
			});

			await expect(provider.initialize()).rejects.toBeTruthy();
		});
	});

	describe('evaluation', () => {
		async function readyProvider(fetchImpl = mockFetch({})) {
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch: fetchImpl,
				refreshInterval: 30_000,
			});
			await provider.initialize();
			return provider;
		}

		it('returns FLAG_NOT_FOUND for unknown keys', async () => {
			const provider = await readyProvider();
			const result = await provider.resolveBooleanEvaluation('nope', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('returns PARSE_ERROR for missing variation', async () => {
			const provider = await readyProvider();
			const result = await provider.resolveBooleanEvaluation('bad_flag', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.PARSE_ERROR);
		});

		it('returns PARSE_ERROR for malformed flag entries', async () => {
			const malformed = {
				flags: { broken: { key: 'broken', enabled: true, default_variation: 'a' }, ok: definitions.flags.string_flag },
			};
			const provider = await readyProvider(
				mockFetch({ definitions: () => ({ ok: true, status: 200, headers: new Headers(), json: async () => malformed }) as Response }),
			);
			const result = await provider.resolveBooleanEvaluation('broken', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.PARSE_ERROR);
			// A malformed flag doesn't poison the others.
			const ok = await provider.resolveStringEvaluation('ok', 'x', {}, noopLogger);
			expect(ok.value).toBe('hello');
		});

		it.each(['constructor', '__proto__', 'toString', 'hasOwnProperty'])('returns FLAG_NOT_FOUND for prototype key %s', async (key) => {
			const provider = await readyProvider();
			const result = await provider.resolveBooleanEvaluation(key, false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('returns DISABLED default', async () => {
			const provider = await readyProvider();
			const result = await provider.resolveBooleanEvaluation('disabled_flag', true, {}, noopLogger);
			expect(result).toMatchObject({ value: true, reason: 'DISABLED' });
		});

		it('returns TYPE_MISMATCH', async () => {
			const provider = await readyProvider();
			const result = await provider.resolveBooleanEvaluation('string_flag', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.TYPE_MISMATCH);
		});

		it('returns STATIC for rule-less flags', async () => {
			const provider = await readyProvider();
			const result = await provider.resolveStringEvaluation('string_flag', 'x', {}, noopLogger);
			expect(result).toMatchObject({ value: 'hello', reason: 'STATIC', variant: 'a' });
		});

		it('returns PROVIDER_NOT_READY before initialize', async () => {
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch: mockFetch({}),
			});
			const result = await provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.PROVIDER_NOT_READY);
		});
	});

	describe('refresh', () => {
		it('sends If-None-Match and keeps snapshot on 304', async () => {
			let calls = 0;
			const etags: Array<string | null> = [];
			const fetch = mockFetch({
				definitions: () => {
					calls += 1;
					if (calls === 1) {
						return {
							ok: true,
							status: 200,
							headers: new Headers({ etag: '"v1"' }),
							json: async () => definitions,
						} as Response;
					}
					return {
						ok: false,
						status: 304,
						statusText: 'Not Modified',
						headers: new Headers({ etag: '"v1"' }),
					} as Response;
				},
				onDefinitions: (req) => {
					etags.push(new Headers(req.headers).get('If-None-Match'));
				},
			});

			vi.useFakeTimers();
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				refreshInterval: 1000,
			});
			await provider.initialize();
			expect(etags[0]).toBeNull();

			await vi.advanceTimersByTimeAsync(1000);
			const p = provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			await p;
			// allow background refresh to settle
			await vi.advanceTimersByTimeAsync(0);
			await Promise.resolve();
			await Promise.resolve();

			expect(calls).toBeGreaterThanOrEqual(2);
			expect(etags[1]).toBe('"v1"');

			const result = await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			expect(result.value).toBe(true);
		});

		it('swaps snapshot on 200 refresh', async () => {
			let calls = 0;
			const fetch = mockFetch({
				definitions: () => {
					calls += 1;
					if (calls === 1) {
						return {
							ok: true,
							status: 200,
							headers: new Headers({ etag: '"v1"' }),
							json: async () => definitions,
						} as Response;
					}
					return {
						ok: true,
						status: 200,
						headers: new Headers({ etag: '"v2"' }),
						json: async () => ({
							flags: {
								bool_flag: {
									key: 'bool_flag',
									enabled: true,
									default_variation: 'off',
									variations: { on: true, off: false },
									rules: [{ conditions: [{ attribute: 'plan', operator: 'equals', value: 'enterprise' }], serve_variation: 'on' }],
								},
							},
						}),
					} as Response;
				},
			});

			vi.useFakeTimers();
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				refreshInterval: 1000,
			});
			await provider.initialize();

			await vi.advanceTimersByTimeAsync(1000);
			await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			await Promise.resolve();
			await Promise.resolve();
			await Promise.resolve();

			const after = await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			// rule now requires enterprise — pro no longer matches
			expect(after).toMatchObject({ value: false, reason: 'DEFAULT' });
		});

		it('keeps old snapshot when refresh fails', async () => {
			let calls = 0;
			const fetch = mockFetch({
				definitions: () => {
					calls += 1;
					if (calls === 1) {
						return {
							ok: true,
							status: 200,
							headers: new Headers({ etag: '"v1"' }),
							json: async () => definitions,
						} as Response;
					}
					return {
						ok: false,
						status: 500,
						statusText: 'boom',
						headers: new Headers(),
					} as Response;
				},
			});

			vi.useFakeTimers();
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				refreshInterval: 1000,
				retries: 0,
			});
			await provider.initialize();

			await vi.advanceTimersByTimeAsync(1000);
			await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			await Promise.resolve();
			await Promise.resolve();
			await Promise.resolve();

			const result = await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			expect(result.value).toBe(true);
		});

		it('does not start concurrent refreshes', async () => {
			let inFlight = 0;
			let maxInFlight = 0;
			let calls = 0;
			let releaseRefresh!: () => void;
			const refreshGate = new Promise<void>((resolve) => {
				releaseRefresh = resolve;
			});

			const fetch = mockFetch({
				definitions: async () => {
					calls += 1;
					if (calls === 1) {
						return {
							ok: true,
							status: 200,
							headers: new Headers({ etag: '"v1"' }),
							json: async () => definitions,
						} as Response;
					}
					inFlight += 1;
					maxInFlight = Math.max(maxInFlight, inFlight);
					await refreshGate;
					inFlight -= 1;
					return {
						ok: true,
						status: 200,
						headers: new Headers({ etag: `"v${calls}"` }),
						json: async () => definitions,
					} as Response;
				},
			});

			vi.useFakeTimers();
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				refreshInterval: 10,
			});
			await provider.initialize();

			await vi.advanceTimersByTimeAsync(10);
			const a = provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			const b = provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			const c = provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			await Promise.all([a, b, c]);
			// Refresh should be in flight exactly once while gated.
			expect(maxInFlight).toBe(1);
			expect(calls).toBe(2);
			releaseRefresh();
			await Promise.resolve();
			await Promise.resolve();
		});
	});

	describe('onClose', () => {
		it('drops the snapshot', async () => {
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch: mockFetch({}),
			});
			await provider.initialize();
			await provider.onClose();

			const result = await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.PROVIDER_NOT_READY);
		});

		it('can be re-initialized after close', async () => {
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch: mockFetch({}),
			});
			await provider.initialize();
			await provider.onClose();
			await provider.initialize();

			const result = await provider.resolveBooleanEvaluation('bool_flag', false, { plan: 'pro' }, noopLogger);
			expect(result.value).toBe(true);
		});

		it('ignores in-flight refresh after close', async () => {
			let resolveFetch!: (value: Response) => void;
			let calls = 0;
			const fetch = mockFetch({
				definitions: () => {
					calls += 1;
					if (calls === 1) {
						return {
							ok: true,
							status: 200,
							headers: new Headers({ etag: '"v1"' }),
							json: async () => definitions,
						} as Response;
					}
					return new Promise<Response>((resolve) => {
						resolveFetch = resolve;
					});
				},
			});

			vi.useFakeTimers();
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				refreshInterval: 10,
			});
			await provider.initialize();

			await vi.advanceTimersByTimeAsync(10);
			void provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			await Promise.resolve();

			await provider.onClose();
			resolveFetch!({
				ok: true,
				status: 200,
				headers: new Headers({ etag: '"v2"' }),
				json: async () => ({
					flags: {
						bool_flag: {
							key: 'bool_flag',
							enabled: true,
							default_variation: 'off',
							variations: { on: true, off: false },
							rules: [],
						},
					},
				}),
			} as Response);
			await Promise.resolve();
			await Promise.resolve();

			const result = await provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			expect(result.errorCode).toBe(ErrorCode.PROVIDER_NOT_READY);
		});

		it('ignores stale in-flight refresh after close + re-initialize', async () => {
			// Regression: a boolean `closed` flag is reset by initialize(), so a
			// refresh that started before onClose can still write after re-init
			// and silently overwrite the fresh snapshot. An epoch/generation
			// guard is required.
			const staleDefinitions = {
				flags: {
					bool_flag: {
						key: 'bool_flag',
						enabled: true,
						default_variation: 'off',
						variations: { on: true, off: false },
						rules: [],
					},
				},
			};
			const freshDefinitions = {
				flags: {
					bool_flag: {
						key: 'bool_flag',
						enabled: true,
						default_variation: 'on',
						variations: { on: true, off: false },
						rules: [],
					},
				},
			};

			let resolveStale!: (value: Response) => void;
			let calls = 0;
			const fetch = mockFetch({
				definitions: () => {
					calls += 1;
					if (calls === 1) {
						// First initialize — baseline snapshot (default off).
						return {
							ok: true,
							status: 200,
							headers: new Headers({ etag: '"v1"' }),
							json: async () => staleDefinitions,
						} as Response;
					}
					if (calls === 2) {
						// Background refresh started before close — held open.
						return new Promise<Response>((resolve) => {
							resolveStale = resolve;
						});
					}
					// Re-initialize after close — fresh snapshot (default on).
					return {
						ok: true,
						status: 200,
						headers: new Headers({ etag: '"v2"' }),
						json: async () => freshDefinitions,
					} as Response;
				},
			});

			vi.useFakeTimers();
			const provider = new FlagshipServerProvider({
				appId: 'app-1',
				accountId: 'acct-1',
				localEvaluation: true,
				fetch,
				refreshInterval: 10,
			});
			await provider.initialize();

			await vi.advanceTimersByTimeAsync(10);
			await provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			await Promise.resolve();

			await provider.onClose();
			await provider.initialize();

			const before = await provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			expect(before).toMatchObject({ value: true, reason: 'STATIC', variant: 'on' });

			// Stale refresh from the previous session finally resolves with the
			// old definitions. It must not overwrite the fresh snapshot.
			resolveStale!({
				ok: true,
				status: 200,
				headers: new Headers({ etag: '"stale"' }),
				json: async () => staleDefinitions,
			} as Response);
			await Promise.resolve();
			await Promise.resolve();

			const after = await provider.resolveBooleanEvaluation('bool_flag', false, {}, noopLogger);
			expect(after).toMatchObject({ value: true, reason: 'STATIC', variant: 'on' });
		});
	});
});
