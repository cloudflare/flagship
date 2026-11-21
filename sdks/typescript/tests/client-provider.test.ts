import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Logger } from '@openfeature/web-sdk';
import { ErrorCode, ProviderEvents, ProviderStatus } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '../src/client-provider.js';
import { FlagshipClient } from '../src/client.js';
import { FlagshipError, FlagshipErrorCode, type FlagshipBulkEvaluationResult } from '../src/types.js';

const noopLogger: Logger = { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} };
const spyLogger = (): Logger => ({ debug: vi.fn(), info: vi.fn(), warn: vi.fn(), error: vi.fn() });
const endpoint = 'https://api.example.com/evaluate';

class FakeEventSource {
	static instances: FakeEventSource[] = [];
	closed = false;
	private readonly listeners = new Map<string, Array<(event: Event | MessageEvent) => void>>();
	constructor(readonly url: string | URL) {
		FakeEventSource.instances.push(this);
	}
	addEventListener(type: string, listener: EventListenerOrEventListenerObject): void {
		const callback = typeof listener === 'function' ? listener : (event: Event) => listener.handleEvent(event);
		const listeners = this.listeners.get(type) ?? [];
		listeners.push(callback);
		this.listeners.set(type, listeners);
	}
	close(): void {
		this.closed = true;
	}
	emit(type: string, data?: string): void {
		const event = type === 'message' ? new MessageEvent(type, { data }) : new Event(type);
		for (const listener of this.listeners.get(type) ?? []) listener(event);
	}
}

vi.mock('../src/client.js', async (importOriginal) => ({
	...(await importOriginal<typeof import('../src/client.js')>()),
	FlagshipClient: vi.fn().mockImplementation(function () {
		return { fetchBulk: vi.fn().mockResolvedValue({ flags: [] }) };
	}),
}));

function mockBulk(...responses: Array<FlagshipBulkEvaluationResult[] | Error>) {
	const evaluateAll = vi.fn();
	for (const response of responses) {
		if (response instanceof Error) evaluateAll.mockRejectedValueOnce(response);
		else evaluateAll.mockResolvedValueOnce({ flags: response });
	}
	(FlagshipClient as any).mockImplementation(function () {
		return { fetchBulk: evaluateAll };
	});
	return evaluateAll;
}

async function initialized(flags: FlagshipBulkEvaluationResult[], options: { logging?: boolean } = {}) {
	mockBulk(flags);
	const provider = new FlagshipClientProvider({ endpoint, ...options });
	await provider.initialize({});
	return provider;
}

describe('FlagshipClientProvider', () => {
	beforeEach(() => {
		vi.clearAllMocks();
		FakeEventSource.instances = [];
	});

	describe('constructor', () => {
		it('creates a client runtime provider', () => {
			const provider = new FlagshipClientProvider({ endpoint });

			expect(provider.metadata.name).toBe('Flagship Client Provider');
			expect(provider.runsOn).toBe('client');
		});

		it('resolves a relative endpoint against window.location.origin', () => {
			vi.stubGlobal('window', { location: { origin: 'https://app.example.com' } });

			new FlagshipClientProvider({ endpoint: '/api/flagship/evaluate' });

			expect(FlagshipClient).toHaveBeenCalledWith(expect.objectContaining({ endpoint: 'https://app.example.com/api/flagship/evaluate' }));

			vi.unstubAllGlobals();
		});

		it('leaves an absolute endpoint untouched', () => {
			vi.stubGlobal('window', { location: { origin: 'https://app.example.com' } });

			new FlagshipClientProvider({ endpoint });

			expect(FlagshipClient).toHaveBeenCalledWith(expect.objectContaining({ endpoint }));

			vi.unstubAllGlobals();
		});

		it('throws when a relative endpoint is used without a browser context', () => {
			vi.stubGlobal('window', undefined);

			expect(() => new FlagshipClientProvider({ endpoint: '/api/flagship/evaluate' })).toThrow(/requires a browser context/);

			vi.unstubAllGlobals();
		});
	});

	describe('initialize', () => {
		it('evaluates every flag once with the given context', async () => {
			const evaluateAll = mockBulk([{ key: 'dark-mode', value: true, reason: 'TARGETING_MATCH', variant: 'on' }]);
			const provider = new FlagshipClientProvider({ endpoint });

			await provider.initialize({ targetingKey: 'user-123' });

			expect(evaluateAll).toHaveBeenCalledTimes(1);
			expect(evaluateAll).toHaveBeenCalledWith({ targetingKey: 'user-123' }, expect.anything());
			expect(provider.status).toBe(ProviderStatus.READY);
		});

		it('emits ProviderEvents.Ready', async () => {
			mockBulk([]);
			const provider = new FlagshipClientProvider({ endpoint });
			const handler = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, handler);

			await provider.initialize();

			expect(handler).toHaveBeenCalled();
		});

		it('rejects when the request fails', async () => {
			mockBulk(new Error('network'));
			const provider = new FlagshipClientProvider({ endpoint });

			await expect(provider.initialize()).rejects.toThrow('network');
			expect(provider.status).toBe(ProviderStatus.ERROR);
		});

		it.each([401, 403])('rejects with PROVIDER_FATAL on %i', async (status) => {
			mockBulk(new FlagshipError(`HTTP ${status}`, FlagshipErrorCode.NETWORK_ERROR, { status }));
			const provider = new FlagshipClientProvider({ endpoint });

			await expect(provider.initialize()).rejects.toMatchObject({ code: ErrorCode.PROVIDER_FATAL });
			expect(provider.status).toBe(ProviderStatus.FATAL);
		});

		it('logs the failure only when logging is enabled', async () => {
			const consoleSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);

			mockBulk(new Error('timeout'), new Error('timeout'));
			await expect(new FlagshipClientProvider({ endpoint }).initialize()).rejects.toThrow();
			expect(consoleSpy).not.toHaveBeenCalled();

			await expect(new FlagshipClientProvider({ endpoint, logging: true }).initialize()).rejects.toThrow();
			expect(consoleSpy).toHaveBeenCalledWith(expect.stringContaining('timeout'));

			consoleSpy.mockRestore();
		});
	});

	describe('onContextChange', () => {
		it('re-evaluates every flag for the new context', async () => {
			const evaluateAll = mockBulk(
				[{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }],
				[{ key: 'f', value: false, reason: 'DEFAULT', variant: 'off' }],
			);
			const provider = new FlagshipClientProvider({ endpoint });

			await provider.onContextChange({}, { targetingKey: 'user-1' });
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);

			await provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' });
			expect(provider.resolveBooleanEvaluation('f', true, {}, noopLogger).value).toBe(false);
			expect(evaluateAll).toHaveBeenLastCalledWith({ targetingKey: 'user-2' }, expect.anything());
		});

		it('rejects without serving values from the previous context', async () => {
			mockBulk([{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }], new Error('network'));
			const provider = new FlagshipClientProvider({ endpoint });

			await provider.onContextChange({}, { targetingKey: 'user-1' });
			await expect(provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' })).rejects.toThrow('network');

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
			expect(provider.status).toBe(ProviderStatus.ERROR);
		});

		it('returns to READY once a later context change succeeds', async () => {
			mockBulk(new Error('network'), [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }]);
			const provider = new FlagshipClientProvider({ endpoint });

			await expect(provider.onContextChange({}, { targetingKey: 'user-1' })).rejects.toThrow('network');
			await provider.onContextChange({ targetingKey: 'user-1' }, { targetingKey: 'user-2' });

			expect(provider.status).toBe(ProviderStatus.READY);
		});
	});

	describe('resolution', () => {
		it('returns the evaluated value, variant and reason for each type', async () => {
			const theme = { primary: '#007bff' };
			const provider = await initialized([
				{ key: 'bool', value: true, reason: 'TARGETING_MATCH', variant: 'on' },
				{ key: 'str', value: 'Hello!', reason: 'DEFAULT', variant: 'default' },
				{ key: 'num', value: 10, reason: 'SPLIT', variant: 'ten' },
				{ key: 'obj', value: theme, reason: 'STATIC', variant: 'default' },
				{ key: 'nil', value: null, reason: 'DEFAULT', variant: 'default' },
			]);

			expect(provider.resolveBooleanEvaluation('bool', false, {}, noopLogger)).toMatchObject({
				value: true,
				reason: 'TARGETING_MATCH',
				variant: 'on',
			});
			expect(provider.resolveStringEvaluation('str', 'fallback', {}, noopLogger)).toMatchObject({ value: 'Hello!', reason: 'DEFAULT' });
			expect(provider.resolveNumberEvaluation('num', 5, {}, noopLogger)).toMatchObject({ value: 10, reason: 'SPLIT' });
			expect(provider.resolveObjectEvaluation('obj', {}, {}, noopLogger).value).toEqual(theme);
			const nil = provider.resolveObjectEvaluation('nil', {}, {}, noopLogger);
			expect(nil.value).toBeNull();
			expect(nil.errorCode).toBeUndefined();
		});

		it('passes flag metadata through', async () => {
			const provider = await initialized([
				{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on', metadata: { owner: 'web', tier: 2 } },
			]);

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).flagMetadata).toEqual({ owner: 'web', tier: 2 });
		});

		it('returns the caller default for a disabled flag', async () => {
			const provider = await initialized([{ key: 'f', reason: 'DISABLED', metadata: { owner: 'web' } }]);

			expect(provider.resolveBooleanEvaluation('f', true, {}, noopLogger)).toEqual({
				value: true,
				reason: 'DISABLED',
				flagMetadata: { owner: 'web' },
			});
		});

		it('maps a per-flag failure to its OpenFeature error code', async () => {
			const provider = await initialized([{ key: 'f', errorCode: 'PARSE_ERROR', errorDetails: 'bad config' }]);

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger)).toEqual({
				value: false,
				reason: 'ERROR',
				errorCode: ErrorCode.PARSE_ERROR,
				errorMessage: 'bad config',
			});
		});

		it('maps an unknown per-flag error code to GENERAL', async () => {
			const provider = await initialized([{ key: 'f', errorCode: 'SOMETHING_NEW' }]);

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.GENERAL);
		});

		it('returns TYPE_MISMATCH when the value type differs from the request', async () => {
			const provider = await initialized([{ key: 'f', value: 'string-value', reason: 'DEFAULT', variant: 'v' }]);

			const result = provider.resolveBooleanEvaluation('f', false, {}, noopLogger);

			expect(result).toMatchObject({ value: false, errorCode: ErrorCode.TYPE_MISMATCH, reason: 'ERROR' });
			expect(result.errorMessage).toContain('expected boolean, got string');
		});

		it('returns FLAG_NOT_FOUND for a flag missing from the response', async () => {
			const provider = await initialized([]);

			expect(provider.resolveBooleanEvaluation('missing', false, {}, noopLogger)).toMatchObject({
				value: false,
				reason: 'ERROR',
				errorCode: ErrorCode.FLAG_NOT_FOUND,
			});
		});

		it('logs warnings through the injected logger only when logging is enabled', async () => {
			const entries: FlagshipBulkEvaluationResult[] = [{ key: 'f', value: 'x', reason: 'DEFAULT', variant: 'v' }];
			const quiet = spyLogger();
			const loud = spyLogger();

			const silent = await initialized(entries);
			silent.resolveBooleanEvaluation('missing', false, {}, quiet);
			silent.resolveBooleanEvaluation('f', false, {}, quiet);
			expect(quiet.warn).not.toHaveBeenCalled();

			const verbose = await initialized(entries, { logging: true });
			verbose.resolveBooleanEvaluation('missing', false, {}, loud);
			verbose.resolveBooleanEvaluation('f', false, {}, loud);
			expect(loud.warn).toHaveBeenCalledWith(expect.stringContaining('missing'));
			expect(loud.warn).toHaveBeenCalledWith(expect.stringContaining('type mismatch'));
		});
	});

	describe('lifecycle', () => {
		it('is NOT_READY before initialize and after onClose', async () => {
			const provider = await initialized([]);
			expect(provider.status).toBe(ProviderStatus.READY);

			await provider.onClose();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
		});

		it('clears the cache on close', async () => {
			const provider = await initialized([{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }]);

			await provider.onClose();

			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('discards an in-flight initialize on close without emitting READY', async () => {
			let resolveEvaluate!: (response: { flags: FlagshipBulkEvaluationResult[] }) => void;
			const evaluateAll = mockBulk();
			evaluateAll.mockReturnValueOnce(new Promise((resolve) => (resolveEvaluate = resolve)));
			const provider = new FlagshipClientProvider({ endpoint });
			const onReady = vi.fn();
			provider.events.addHandler(ProviderEvents.Ready, onReady);

			const initializing = provider.initialize({});
			await provider.onClose();
			resolveEvaluate({ flags: [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }] });
			await initializing;

			expect(onReady).not.toHaveBeenCalled();
			expect(provider.status).toBe(ProviderStatus.NOT_READY);
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
		});

		it('onClose is idempotent', async () => {
			const provider = await initialized([]);
			await provider.onClose();
			await expect(provider.onClose()).resolves.toBeUndefined();
		});
	});

	describe('SSE change detection', () => {
		it('refreshes the notified version before emitting configuration changed', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce({
					flags: [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }],
					metadata: { version: 'v1' },
					eventStreams: [{ type: 'sse', url: 'https://events.example.com/ticket' }],
				})
				.mockResolvedValueOnce({
					flags: [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }],
					metadata: { version: 'v2' },
					eventStreams: [{ type: 'sse', url: 'https://events.example.com/ticket-2' }],
				});
			(FlagshipClient as any).mockImplementation(function () {
				return { fetchBulk };
			});
			const provider = new FlagshipClientProvider({ endpoint, eventSource: FakeEventSource as unknown as typeof EventSource });
			const onChanged = vi.fn();
			provider.events.addHandler(ProviderEvents.ConfigurationChanged, onChanged);
			await provider.initialize({ targetingKey: 'user' });

			FakeEventSource.instances[0]!.emit('message', JSON.stringify({ type: 'refetchEvaluation', etag: 'v2' }));
			await vi.waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1));

			// The healthy stream is kept: no new ticket is requested and no reconnect happens.
			expect(fetchBulk).toHaveBeenLastCalledWith(
				{ targetingKey: 'user' },
				{ flagConfigEtag: 'v2', flagConfigLastModified: undefined, ifNoneMatch: undefined },
			);
			expect(FakeEventSource.instances).toHaveLength(1);
			await provider.onClose();
		});

		it('requests a fresh single-use ticket and reconnects after the stream drops', async () => {
			const stream = (ticket: string) => [{ type: 'sse', url: `https://events.example.com/${ticket}` }];
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce({ flags: [], metadata: { version: 'v1' }, eventStreams: stream('ticket-1') })
				.mockResolvedValueOnce({ flags: [], metadata: { version: 'v1' }, eventStreams: stream('ticket-2') });
			(FlagshipClient as any).mockImplementation(function () {
				return { fetchBulk };
			});
			const provider = new FlagshipClientProvider({ endpoint, eventSource: FakeEventSource as unknown as typeof EventSource });
			await provider.initialize({});

			FakeEventSource.instances[0]!.emit('error');
			await vi.waitFor(() => expect(FakeEventSource.instances).toHaveLength(2));

			expect(FakeEventSource.instances[0]!.closed).toBe(true);
			expect(fetchBulk).toHaveBeenLastCalledWith({}, { requestEventStream: true });
			expect(String(FakeEventSource.instances[1]!.url)).toBe('https://events.example.com/ticket-2');
			await provider.onClose();
		});

		it('does not retry a consumed ticket or report READY after a failed reconnect refresh', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce({
					flags: [],
					metadata: { version: 'v1' },
					eventStreams: [{ type: 'sse', url: 'https://events.example.com/first-ticket' }],
				})
				.mockRejectedValueOnce(new Error('discovery unavailable'))
				.mockResolvedValueOnce({
					flags: [],
					metadata: { version: 'v2' },
					eventStreams: [{ type: 'sse', url: 'https://events.example.com/second-ticket' }],
				});
			(FlagshipClient as any).mockImplementation(function () {
				return { fetchBulk };
			});
			const provider = new FlagshipClientProvider({ endpoint, eventSource: FakeEventSource as unknown as typeof EventSource });
			await provider.initialize({});
			FakeEventSource.instances[0]!.emit('error');
			expect(FakeEventSource.instances[0]!.closed).toBe(true);
			await vi.waitFor(() => expect(provider.status).toBe(ProviderStatus.STALE));
			FakeEventSource.instances[0]!.emit('open');
			expect(provider.status).toBe(ProviderStatus.STALE);
			await vi.waitFor(() => expect(FakeEventSource.instances).toHaveLength(2), { timeout: 5_000 });
			expect(fetchBulk).toHaveBeenCalledTimes(3);
			await provider.onClose();
		});

		it('retries with backoff while the refetch still observes an older version', async () => {
			vi.useFakeTimers();
			try {
				const fetchBulk = vi
					.fn()
					.mockResolvedValueOnce({ flags: [], metadata: { version: 'v1' }, eventStreams: [{ type: 'sse', url: 'https://e.example/t' }] })
					.mockResolvedValueOnce({ flags: [], metadata: { version: 'v1' } })
					.mockResolvedValueOnce({ flags: [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }], metadata: { version: 'v2' } });
				(FlagshipClient as any).mockImplementation(function () {
					return { fetchBulk };
				});
				const provider = new FlagshipClientProvider({ endpoint, eventSource: FakeEventSource as unknown as typeof EventSource });
				await provider.initialize({});

				FakeEventSource.instances[0]!.emit('message', JSON.stringify({ type: 'refetchEvaluation', etag: 'v2' }));
				await vi.advanceTimersByTimeAsync(100);
				expect(fetchBulk).toHaveBeenCalledTimes(2);
				await vi.advanceTimersByTimeAsync(1_000);
				expect(fetchBulk).toHaveBeenCalledTimes(3);
				await vi.advanceTimersByTimeAsync(60_000);
				expect(fetchBulk).toHaveBeenCalledTimes(3);
				expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);
				await provider.onClose();
			} finally {
				vi.useRealTimers();
			}
		});

		it('can disable background change detection', async () => {
			const fetchBulk = vi.fn().mockResolvedValue({
				flags: [],
				metadata: { version: 'v1' },
				eventStreams: [{ type: 'sse', url: 'https://events.example.com/ticket' }],
			});
			(FlagshipClient as any).mockImplementation(function () {
				return { fetchBulk };
			});
			await new FlagshipClientProvider({
				endpoint,
				changeDetection: 'none',
				eventSource: FakeEventSource as unknown as typeof EventSource,
			}).initialize({});
			expect(FakeEventSource.instances).toHaveLength(0);
		});
	});

	describe('OFREP static-context conformance', () => {
		const es = FakeEventSource as unknown as typeof EventSource;
		const streams = (...tickets: string[]) => tickets.map((ticket) => ({ type: 'sse', url: `https://events.example.com/${ticket}` }));
		const bulk = (overrides: Record<string, unknown> = {}) => ({ flags: [], metadata: { version: 'v1' }, etag: '"e1"', ...overrides });
		const withClient = (fetchBulk: ReturnType<typeof vi.fn>) =>
			(FlagshipClient as any).mockImplementation(function () {
				return { fetchBulk };
			});

		class FakeDocument {
			visibilityState: 'visible' | 'hidden' = 'visible';
			private readonly listeners = new Set<() => void>();
			addEventListener(_type: string, listener: () => void): void {
				this.listeners.add(listener);
			}
			removeEventListener(_type: string, listener: () => void): void {
				this.listeners.delete(listener);
			}
			set(state: 'visible' | 'hidden'): void {
				this.visibilityState = state;
				for (const listener of this.listeners) listener();
			}
		}

		afterEach(() => {
			vi.useRealTimers();
			vi.unstubAllGlobals();
		});

		it('applies the static-provider default timeout without overriding an explicit one', () => {
			new FlagshipClientProvider({ endpoint });
			expect(FlagshipClient).toHaveBeenLastCalledWith(expect.objectContaining({ timeout: 10_000 }));

			new FlagshipClientProvider({ endpoint, timeout: 2_000 });
			expect(FlagshipClient).toHaveBeenLastCalledWith(expect.objectContaining({ timeout: 2_000 }));
		});

		it('accepts a generic OFREP base URL and headers', () => {
			new FlagshipClientProvider({
				ofrepBaseUrl: 'https://flags.example.com/api/',
				headers: { Authorization: 'Bearer generic', 'X-Team': 'web' },
				fetchOptions: { headers: { 'X-Team': 'override' } },
			});

			const options = (FlagshipClient as any).mock.calls.at(-1)[0];
			expect(options.endpoint).toBe('https://flags.example.com/api/evaluate');
			expect(options.fetchOptions.headers.get('Authorization')).toBe('Bearer generic');
			expect(options.fetchOptions.headers.get('X-Team')).toBe('override');
			expect(() => new FlagshipClientProvider({ ofrepBaseUrl: 'https://flags.example.com', endpoint })).toThrow(/only one of/);
			expect(() => new FlagshipClientProvider({ ofrepBaseUrl: 'ftp://flags.example.com' })).toThrow(/http or https/);
		});

		it('forwards etag and lastModified from the event, separately from the response ETag', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk({ eventStreams: streams('t1') }))
				.mockResolvedValueOnce(bulk({ metadata: { version: 'v2' }, etag: '"e2"' }))
				.mockResolvedValueOnce(bulk({ metadata: { version: 'v3' }, etag: '"e3"' }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es });
			await provider.initialize({});

			FakeEventSource.instances[0]!.emit('message', JSON.stringify({ type: 'refetchEvaluation', etag: 'v2', lastModified: 1771622898 }));
			await vi.waitFor(() => expect(fetchBulk).toHaveBeenCalledTimes(2));
			expect(fetchBulk).toHaveBeenLastCalledWith({}, { ifNoneMatch: '"e1"', flagConfigEtag: 'v2', flagConfigLastModified: 1771622898 });

			FakeEventSource.instances[0]!.emit(
				'message',
				JSON.stringify({ type: 'refetchEvaluation', etag: 'v3', lastModified: '2026-02-20T21:28:18Z' }),
			);
			await vi.waitFor(() => expect(fetchBulk).toHaveBeenCalledTimes(3));
			expect(fetchBulk).toHaveBeenLastCalledWith(
				{},
				{ ifNoneMatch: '"e2"', flagConfigEtag: 'v3', flagConfigLastModified: '2026-02-20T21:28:18Z' },
			);
			await provider.onClose();
		});

		it('drops invalid lastModified values and ignores unknown event types', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk({ eventStreams: streams('t1') }))
				.mockResolvedValueOnce(bulk({ metadata: { version: 'v2' } }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es });
			await provider.initialize({});

			FakeEventSource.instances[0]!.emit('message', JSON.stringify({ type: 'fullConfig', etag: 'v9' }));
			FakeEventSource.instances[0]!.emit('message', JSON.stringify({ type: 'refetchEvaluation', etag: 'v2', lastModified: 'not a date' }));
			await vi.waitFor(() => expect(fetchBulk).toHaveBeenCalledTimes(2));
			expect(fetchBulk.mock.calls[1][1].flagConfigLastModified).toBeUndefined();
			await provider.onClose();
		});

		it('connects to every advertised SSE stream, ignores unknown types, and coalesces a burst', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(
					bulk({
						eventStreams: [
							...streams('global'),
							{ type: 'websocket', url: 'wss://events.example.com/ignored' },
							{ type: 'sse', endpoint: { origin: 'https://user.example.com', requestUri: '/stream?ch=1' } },
						],
					}),
				)
				.mockResolvedValueOnce(bulk({ metadata: { version: 'v2' } }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es });
			await provider.initialize({});

			expect(FakeEventSource.instances.map((source) => String(source.url))).toEqual([
				'https://events.example.com/global',
				'https://user.example.com/stream?ch=1',
			]);
			for (const source of FakeEventSource.instances) source.emit('message', JSON.stringify({ type: 'refetchEvaluation', etag: 'v2' }));
			await vi.waitFor(() => expect(fetchBulk).toHaveBeenCalledTimes(2));
			await new Promise((resolve) => setTimeout(resolve, 100));
			expect(fetchBulk).toHaveBeenCalledTimes(2);
			await provider.onClose();
		});

		it('keeps the cache and emits nothing when a revalidation returns 304', async () => {
			vi.useFakeTimers();
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk({ flags: [{ key: 'f', value: true, reason: 'DEFAULT', variant: 'on' }] }))
				.mockResolvedValueOnce('not-modified');
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval: 1_000, changeDetection: 'polling' });
			const onChanged = vi.fn();
			provider.events.addHandler(ProviderEvents.ConfigurationChanged, onChanged);
			await provider.initialize({});

			await vi.advanceTimersByTimeAsync(1_000);

			expect(fetchBulk).toHaveBeenLastCalledWith({}, { ifNoneMatch: '"e1"', requestEventStream: false });
			expect(onChanged).not.toHaveBeenCalled();
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);
			await provider.onClose();
		});

		it('fetches a new context unconditionally and closes streams the new response omits', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk({ eventStreams: streams('t1') }))
				.mockResolvedValueOnce(bulk({ etag: '"e2"' }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es });
			await provider.initialize({ targetingKey: 'a' });

			await provider.onContextChange({ targetingKey: 'a' }, { targetingKey: 'b' });

			expect(fetchBulk).toHaveBeenLastCalledWith({ targetingKey: 'b' }, { requestEventStream: false });
			expect(FakeEventSource.instances[0]!.closed).toBe(true);
			await provider.onClose();
		});

		it('replaces a stream whose URL changes on a context change and keeps an unchanged one', async () => {
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk({ eventStreams: streams('shared') }))
				.mockResolvedValueOnce(bulk({ eventStreams: streams('shared') }))
				.mockResolvedValueOnce(bulk({ eventStreams: streams('user-c') }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es });
			await provider.initialize({ targetingKey: 'a' });

			await provider.onContextChange({ targetingKey: 'a' }, { targetingKey: 'b' });
			expect(FakeEventSource.instances).toHaveLength(1);
			expect(FakeEventSource.instances[0]!.closed).toBe(false);

			await provider.onContextChange({ targetingKey: 'b' }, { targetingKey: 'c' });
			expect(FakeEventSource.instances).toHaveLength(2);
			expect(FakeEventSource.instances[0]!.closed).toBe(true);
			expect(String(FakeEventSource.instances[1]!.url)).toBe('https://events.example.com/user-c');
			await provider.onClose();
		});

		it('stops background refreshes with PROVIDER_FATAL after a 401', async () => {
			vi.useFakeTimers();
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk())
				.mockRejectedValueOnce(new FlagshipError('HTTP 401', FlagshipErrorCode.NETWORK_ERROR, { status: 401 }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval: 1_000 });
			const onError = vi.fn();
			provider.events.addHandler(ProviderEvents.Error, onError);
			await provider.initialize({});

			await vi.advanceTimersByTimeAsync(10_000);

			expect(provider.status).toBe(ProviderStatus.FATAL);
			expect(onError).toHaveBeenCalledWith(expect.objectContaining({ errorCode: ErrorCode.PROVIDER_FATAL }));
			expect(fetchBulk).toHaveBeenCalledTimes(2);
		});

		it('sends no bulk request before a 429 Retry-After time', async () => {
			vi.useFakeTimers();
			const limited = new FlagshipError(
				'HTTP 429',
				FlagshipErrorCode.NETWORK_ERROR,
				new Response(null, { status: 429, headers: { 'Retry-After': '5' } }),
			);
			const fetchBulk = vi.fn().mockResolvedValueOnce(bulk()).mockRejectedValueOnce(limited).mockResolvedValue('not-modified');
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval: 1_000 });
			await provider.initialize({});

			await vi.advanceTimersByTimeAsync(1_000);
			expect(fetchBulk).toHaveBeenCalledTimes(2);
			await vi.advanceTimersByTimeAsync(4_000);
			expect(fetchBulk).toHaveBeenCalledTimes(2);
			await vi.advanceTimersByTimeAsync(1_000);
			expect(fetchBulk).toHaveBeenCalledTimes(3);
			await provider.onClose();
		});

		it('does not poll by default and skips the poll while a stream is connected', async () => {
			vi.useFakeTimers();
			const fetchBulk = vi.fn().mockResolvedValue(bulk({ eventStreams: streams('t1') }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es, pollInterval: 1_000 });
			await provider.initialize({});

			await vi.advanceTimersByTimeAsync(5_000);

			expect(fetchBulk).toHaveBeenCalledTimes(1);
			await provider.onClose();
		});

		it('revalidates on foreground only when no stream covers change detection', async () => {
			const doc = new FakeDocument();
			vi.stubGlobal('document', doc);
			const fetchBulk = vi.fn().mockResolvedValueOnce(bulk()).mockResolvedValue('not-modified');
			withClient(fetchBulk);
			const withoutStream = new FlagshipClientProvider({ endpoint, eventSource: es });
			await withoutStream.initialize({});

			doc.set('hidden');
			doc.set('visible');
			await vi.waitFor(() => expect(fetchBulk).toHaveBeenCalledTimes(2));
			expect(fetchBulk).toHaveBeenLastCalledWith({}, { ifNoneMatch: '"e1"', requestEventStream: true });
			await withoutStream.onClose();

			const streamed = vi.fn().mockResolvedValue(bulk({ eventStreams: streams('t1') }));
			withClient(streamed);
			const withStream = new FlagshipClientProvider({ endpoint, eventSource: es });
			await withStream.initialize({});
			doc.set('hidden');
			doc.set('visible');
			await new Promise((resolve) => setTimeout(resolve, 20));
			expect(streamed).toHaveBeenCalledTimes(1);
			await withStream.onClose();

			const none = vi.fn().mockResolvedValue(bulk());
			withClient(none);
			const disabled = new FlagshipClientProvider({ endpoint, changeDetection: 'none' });
			await disabled.initialize({});
			doc.set('hidden');
			doc.set('visible');
			await new Promise((resolve) => setTimeout(resolve, 20));
			expect(none).toHaveBeenCalledTimes(1);
			await disabled.onClose();
		});

		it('closes an idle stream and resumes with an unconditional refetch and a fresh ticket', async () => {
			vi.useFakeTimers();
			const doc = new FakeDocument();
			vi.stubGlobal('document', doc);
			const fetchBulk = vi
				.fn()
				.mockResolvedValueOnce(bulk({ eventStreams: [{ type: 'sse', url: 'https://events.example.com/t1', inactivityDelaySec: 30 }] }))
				.mockResolvedValueOnce(bulk({ eventStreams: streams('t2'), etag: '"e2"' }));
			withClient(fetchBulk);
			const provider = new FlagshipClientProvider({ endpoint, eventSource: es });
			const onStale = vi.fn();
			provider.events.addHandler(ProviderEvents.Stale, onStale);
			await provider.initialize({});

			doc.set('hidden');
			await vi.advanceTimersByTimeAsync(29_000);
			expect(FakeEventSource.instances[0]!.closed).toBe(false);
			await vi.advanceTimersByTimeAsync(1_000);
			expect(FakeEventSource.instances[0]!.closed).toBe(true);

			doc.set('visible');
			await vi.advanceTimersByTimeAsync(0);

			expect(fetchBulk).toHaveBeenLastCalledWith({}, { requestEventStream: true });
			expect(String(FakeEventSource.instances[1]!.url)).toBe('https://events.example.com/t2');
			expect(onStale).not.toHaveBeenCalled();
			await provider.onClose();
		});
	});

	describe('polling', () => {
		const pollInterval = 1000;
		const flag = (variant: string): FlagshipBulkEvaluationResult => ({ key: 'f', value: variant === 'on', reason: 'DEFAULT', variant });

		beforeEach(() => vi.useFakeTimers());
		afterEach(() => vi.useRealTimers());

		it('does not poll unless pollInterval is set', async () => {
			const evaluateAll = mockBulk([flag('on')]);
			await new FlagshipClientProvider({ endpoint }).initialize({});

			await vi.advanceTimersByTimeAsync(10 * pollInterval);

			expect(evaluateAll).toHaveBeenCalledTimes(1);
		});

		it('emits CONFIGURATION_CHANGED with the changed keys only when results differ', async () => {
			const evaluateAll = mockBulk([flag('on')], [flag('on')], [flag('off')]);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval });
			const onChanged = vi.fn();
			provider.events.addHandler(ProviderEvents.ConfigurationChanged, onChanged);
			await provider.initialize({});

			await vi.advanceTimersByTimeAsync(pollInterval);
			expect(onChanged).not.toHaveBeenCalled();

			await vi.advanceTimersByTimeAsync(pollInterval);
			expect(onChanged).toHaveBeenCalledWith(expect.objectContaining({ flagsChanged: ['f'] }));
			expect(provider.resolveBooleanEvaluation('f', true, {}, noopLogger).value).toBe(false);
			expect(evaluateAll).toHaveBeenCalledTimes(3);
		});

		it('goes STALE once when refreshes fail, keeps serving the cache, and recovers to READY', async () => {
			const evaluateAll = mockBulk([flag('on')], new Error('network'), new Error('network'), [flag('off')]);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval });
			const onStale = vi.fn();
			const onReady = vi.fn();
			provider.events.addHandler(ProviderEvents.Stale, onStale);
			await provider.initialize({});
			provider.events.addHandler(ProviderEvents.Ready, onReady);

			await vi.advanceTimersByTimeAsync(2 * pollInterval);
			expect(provider.status).toBe(ProviderStatus.STALE);
			expect(onStale).toHaveBeenCalledTimes(1);
			expect(onStale).toHaveBeenCalledWith(expect.objectContaining({ message: 'network' }));
			expect(provider.resolveBooleanEvaluation('f', false, {}, noopLogger).value).toBe(true);

			await vi.advanceTimersByTimeAsync(pollInterval);
			expect(provider.status).toBe(ProviderStatus.READY);
			expect(onReady).toHaveBeenCalledTimes(1);
			expect(provider.resolveBooleanEvaluation('f', true, {}, noopLogger).value).toBe(false);
			expect(evaluateAll).toHaveBeenCalledTimes(4);
		});

		it('stops polling on close', async () => {
			const evaluateAll = mockBulk([flag('on')]);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval });
			await provider.initialize({});

			await provider.onClose();
			await vi.advanceTimersByTimeAsync(10 * pollInterval);

			expect(evaluateAll).toHaveBeenCalledTimes(1);
		});

		it('polls with the context of the latest load', async () => {
			const evaluateAll = mockBulk([flag('on')], [flag('on')], [flag('on')]);
			const provider = new FlagshipClientProvider({ endpoint, pollInterval });
			await provider.initialize({ targetingKey: 'a' });
			await provider.onContextChange({ targetingKey: 'a' }, { targetingKey: 'b' });

			await vi.advanceTimersByTimeAsync(pollInterval);

			expect(evaluateAll).toHaveBeenCalledTimes(3);
			expect(evaluateAll).toHaveBeenLastCalledWith({ targetingKey: 'b' }, expect.anything());
		});
	});
});
