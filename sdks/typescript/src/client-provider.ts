import type { Provider, ResolutionDetails, EvaluationContext, JsonValue, ProviderMetadata, Logger } from '@openfeature/web-sdk';
import { ErrorCode, OpenFeatureEventEmitter, ProviderEvents, ProviderFatalError, ProviderStatus } from '@openfeature/web-sdk';
import { FlagshipClient, isAuthFailure, retryAfterMs, type BulkFetchOptions } from './client.js';
import { toFlagMetadata } from './metadata.js';
import { SseManager, type RefetchMetadata } from './sse-manager.js';
import { type FlagshipBulkEvaluationResponse, type FlagshipClientProviderOptions, type CachedFlag } from './types.js';

const ERROR_CODES = new Set<string>(Object.values(ErrorCode));
/** The OFREP static-context provider guideline's default request timeout. */
const DEFAULT_TIMEOUT_MS = 10_000;

type RefreshKind = 'discover' | 'hint' | 'revalidate';

/**
 * OpenFeature provider for Flagship (client-side / browser).
 *
 * Evaluates every flag for the current context in a single OFREP bulk request
 * during initialization and on every context change, storing results in an
 * in-memory cache. All `resolve*` methods are synchronous, as required by the
 * OpenFeature web SDK.
 *
 * A flag that is missing from the response returns `ErrorCode.FLAG_NOT_FOUND`
 * with the default value.
 *
 * @example
 * ```typescript
 * import { OpenFeature } from '@openfeature/web-sdk';
 * import { FlagshipClientProvider } from '@cloudflare/flagship/web';
 *
 * await OpenFeature.setProviderAndWait(
 *   new FlagshipClientProvider({
 *     appId: 'app-abc123',
 *     accountId: 'your-account-id',
 *     authToken: 'your-token',
 *   })
 * );
 *
 * await OpenFeature.setContext({ targetingKey: 'user-123', plan: 'premium' });
 *
 * const client = OpenFeature.getClient();
 * const darkMode = client.getBooleanValue('dark-mode', false);
 * ```
 */
export class FlagshipClientProvider implements Provider {
	readonly metadata: ProviderMetadata;
	readonly runsOn = 'client' as const;
	readonly events = new OpenFeatureEventEmitter();

	private cache: Map<string, CachedFlag> = new Map();
	private client: FlagshipClient;
	private readonly logging: boolean;
	private readonly pollInterval: number;
	private readonly changeDetection: 'sse' | 'polling' | 'none';
	private readonly EventSourceImpl?: typeof EventSource;
	private readonly inactivityDelaySec?: number;
	private timer?: ReturnType<typeof setTimeout>;
	private sse?: SseManager;
	private visibility?: () => void;
	private currentStatus: ProviderStatus = ProviderStatus.NOT_READY;
	private currentVersion?: string;
	private context: EvaluationContext = {};
	/** HTTP `ETag` of the cached bulk response, for `If-None-Match`. Distinct from the configuration version. */
	private etag?: string;
	/** No bulk request is sent before this time, per the server's `Retry-After`. */
	private blockedUntil = 0;
	private revalidating?: Promise<string | undefined>;
	private epoch = 0;

	constructor(options: FlagshipClientProviderOptions) {
		this.metadata = { name: 'Flagship Client Provider' };
		const resolved = resolveRelativeEndpoint(resolveOfrepOptions(options));
		this.client = new FlagshipClient({ ...resolved, timeout: options.timeout ?? DEFAULT_TIMEOUT_MS });
		this.logging = options.logging ?? false;
		this.pollInterval = options.pollInterval ?? 0;
		this.changeDetection = options.changeDetection ?? 'sse';
		this.EventSourceImpl = options.eventSource ?? globalThis.EventSource;
		this.inactivityDelaySec = options.inactivityDelaySec;
	}

	get status(): ProviderStatus {
		return this.currentStatus;
	}

	async initialize(context: EvaluationContext = {}): Promise<void> {
		this.installVisibility();
		if (await this.load(context, 'initialization')) this.events.emit(ProviderEvents.Ready);
	}

	async onClose(): Promise<void> {
		this.epoch += 1;
		clearTimeout(this.timer);
		this.sse?.disconnect();
		this.sse = undefined;
		if (this.visibility && typeof document !== 'undefined') document.removeEventListener('visibilitychange', this.visibility);
		this.visibility = undefined;
		this.cache.clear();
		this.currentVersion = undefined;
		this.etag = undefined;
		this.revalidating = undefined;
		this.currentStatus = ProviderStatus.NOT_READY;
	}

	/**
	 * Invalidates the entire cache and re-evaluates every flag for the new
	 * context. Returning a Promise causes the SDK to automatically emit
	 * `ProviderEvents.Reconciling` while this method executes.
	 */
	async onContextChange(_oldContext: EvaluationContext, newContext: EvaluationContext = {}): Promise<void> {
		this.cache.clear();
		await this.load(newContext, 'context change');
	}

	resolveBooleanEvaluation(
		flagKey: string,
		defaultValue: boolean,
		_context: EvaluationContext,
		logger: Logger,
	): ResolutionDetails<boolean> {
		return this.resolveFromCache(flagKey, defaultValue, 'boolean', logger);
	}

	resolveStringEvaluation(flagKey: string, defaultValue: string, _context: EvaluationContext, logger: Logger): ResolutionDetails<string> {
		return this.resolveFromCache(flagKey, defaultValue, 'string', logger);
	}

	resolveNumberEvaluation(flagKey: string, defaultValue: number, _context: EvaluationContext, logger: Logger): ResolutionDetails<number> {
		return this.resolveFromCache(flagKey, defaultValue, 'number', logger);
	}

	resolveObjectEvaluation<T extends JsonValue>(
		flagKey: string,
		defaultValue: T,
		_context: EvaluationContext,
		logger: Logger,
	): ResolutionDetails<T> {
		return this.resolveFromCache(flagKey, defaultValue, 'object', logger);
	}

	/**
	 * Evaluates every flag for a new context. The request is unconditional so a
	 * previous context's `ETag` can never validate another context's results.
	 * Resolves to false when `onClose()` or a newer load superseded this one.
	 */
	private async load(context: EvaluationContext, phase: string): Promise<boolean> {
		const epoch = ++this.epoch;
		clearTimeout(this.timer);
		this.context = context;
		this.etag = undefined;
		this.revalidating = undefined;
		try {
			const response = await this.fetchAll('discover', phase);
			if (epoch !== this.epoch) return false;
			if (response === 'not-modified') throw new Error('Flagship: unexpected 304 for an unconditional bulk evaluation');
			this.apply(response);
			this.currentStatus = ProviderStatus.READY;
			this.reconcileStreams(response, true);
			this.schedulePoll(epoch);
			return true;
		} catch (error) {
			if (epoch === this.epoch) {
				this.sse?.disconnect();
				this.currentStatus = error instanceof ProviderFatalError ? ProviderStatus.FATAL : ProviderStatus.ERROR;
			}
			throw error;
		}
	}

	/**
	 * Connects to advertised event streams. A context change replaces streams whose
	 * URLs changed and closes them when none are advertised; other refreshes keep a
	 * healthy stream and connect only when none is open.
	 */
	private reconcileStreams(response: FlagshipBulkEvaluationResponse, contextChanged: boolean): void {
		if (this.changeDetection !== 'sse' || !this.EventSourceImpl) return;
		const streams = response.eventStreams ?? [];
		if (streams.length === 0) {
			if (contextChanged) this.sse?.disconnect();
			return;
		}
		if (this.sse?.active && (!contextChanged || this.sse.matches(streams))) return;
		this.sse ??= new SseManager(
			{
				onRefetch: (metadata, attempt) => this.refresh('hint', metadata, attempt === 0),
				// Both reasons need a fresh single-use ticket, which a 304 cannot carry, and
				// a resume after inactivity must refetch unconditionally.
				onReconnect: (reason, message) => {
					if (reason === 'error') this.markStale(message);
					return this.refresh('discover');
				},
			},
			this.EventSourceImpl,
			this.inactivityDelaySec,
		);
		this.sse.connect(streams);
	}

	private schedulePoll(epoch: number): void {
		if (this.changeDetection === 'none' || this.pollInterval <= 0) return;
		this.timer = setTimeout(async () => {
			if (epoch !== this.epoch) return;
			// In SSE mode, timer polling is a fallback for when no stream is connected.
			if (!(this.changeDetection === 'sse' && this.sse?.active)) await this.refresh('revalidate');
			if (epoch === this.epoch && this.currentStatus !== ProviderStatus.FATAL) this.schedulePoll(epoch);
		}, this.pollInterval);
	}

	/**
	 * Refreshes when the page becomes visible again. An open or managed event
	 * stream already covers change detection and handles its own resumption.
	 */
	private installVisibility(): void {
		if (this.visibility || this.changeDetection === 'none' || typeof document === 'undefined') return;
		this.visibility = () => {
			if (document.visibilityState !== 'visible') return;
			if (this.currentStatus === ProviderStatus.NOT_READY || this.currentStatus === ProviderStatus.FATAL) return;
			if (this.changeDetection === 'sse' && this.sse?.managing) return;
			void this.refresh('revalidate');
		};
		document.addEventListener('visibilitychange', this.visibility);
	}

	private refresh(kind: RefreshKind, metadata?: RefetchMetadata, configurationEvent = false): Promise<string | undefined> {
		if (kind !== 'revalidate') return this.applyRefresh(kind, metadata, configurationEvent);
		// Coalesce overlapping poll and foreground refreshes.
		this.revalidating ??= this.applyRefresh(kind, undefined, false).finally(() => (this.revalidating = undefined));
		return this.revalidating;
	}

	private async applyRefresh(
		kind: RefreshKind,
		metadata: RefetchMetadata | undefined,
		configurationEvent: boolean,
	): Promise<string | undefined> {
		if (this.currentStatus === ProviderStatus.FATAL) return undefined;
		const epoch = this.epoch;
		try {
			const response = await this.fetchAll(kind, 'refresh', metadata);
			if (epoch !== this.epoch) return undefined;
			this.recover();
			if (response === 'not-modified') return this.currentVersion;
			const previous = this.cache;
			const previousVersion = this.currentVersion;
			this.apply(response);
			const flagsChanged = changedKeys(previous, this.cache);
			const versionChanged = Boolean(response.metadata?.version && response.metadata.version !== previousVersion);
			this.reconcileStreams(response, false);
			if (configurationEvent || versionChanged || flagsChanged.length > 0) {
				this.events.emit(ProviderEvents.ConfigurationChanged, flagsChanged.length > 0 ? { flagsChanged } : undefined);
			}
			return this.currentVersion;
		} catch (error) {
			if (epoch !== this.epoch) return undefined;
			const message = error instanceof Error ? error.message : String(error);
			if (error instanceof ProviderFatalError) this.fail(message);
			else this.markStale(message);
			return undefined;
		}
	}

	private apply(response: FlagshipBulkEvaluationResponse): void {
		this.cache = toCache(response);
		this.etag = response.etag;
		this.currentVersion = response.metadata?.version ?? this.currentVersion;
	}

	private markStale(message: string): void {
		if (this.currentStatus !== ProviderStatus.READY) return;
		this.currentStatus = ProviderStatus.STALE;
		this.events.emit(ProviderEvents.Stale, { message });
	}

	private recover(): void {
		if (this.currentStatus !== ProviderStatus.STALE) return;
		this.currentStatus = ProviderStatus.READY;
		this.events.emit(ProviderEvents.Ready);
	}

	/** Stops background refreshes after the server rejects the credentials. */
	private fail(message: string): void {
		this.currentStatus = ProviderStatus.FATAL;
		clearTimeout(this.timer);
		this.sse?.disconnect();
		this.sse = undefined;
		this.events.emit(ProviderEvents.Error, { message, errorCode: ErrorCode.PROVIDER_FATAL });
	}

	/**
	 * Evaluates every flag for the current context. `discover` is unconditional and
	 * requests an event stream when none is open; `hint` forwards change-event
	 * metadata; `revalidate` sends the stored `ETag`. Throws when the request fails,
	 * with `PROVIDER_FATAL` on a 401 or 403.
	 */
	private async fetchAll(
		kind: RefreshKind,
		phase: string,
		metadata?: RefetchMetadata,
	): Promise<FlagshipBulkEvaluationResponse | 'not-modified'> {
		if (Date.now() < this.blockedUntil) throw new Error('Flagship: bulk evaluation is rate limited until the Retry-After time');
		const wantsStream = this.changeDetection === 'sse' && this.EventSourceImpl !== undefined && !this.sse?.active;
		const request: BulkFetchOptions =
			kind === 'discover'
				? { requestEventStream: wantsStream }
				: kind === 'hint'
					? { ifNoneMatch: this.etag, flagConfigEtag: metadata?.etag, flagConfigLastModified: metadata?.lastModified }
					: { ifNoneMatch: this.etag, requestEventStream: wantsStream };
		try {
			return await this.client.fetchBulk(this.context, request);
		} catch (error) {
			const wait = retryAfterMs(error);
			if (wait !== undefined) this.blockedUntil = Date.now() + wait;
			const message = error instanceof Error ? error.message : String(error);
			if (this.logging) console.warn(`[Flagship] Failed to evaluate flags during ${phase}: ${message}`);
			throw isAuthFailure(error) ? new ProviderFatalError(message) : error;
		}
	}

	private resolveFromCache<T>(flagKey: string, defaultValue: T, expectedType: string, logger: Logger): ResolutionDetails<T> {
		const cached = this.cache.get(flagKey);

		if (!cached) {
			const msg = `Flag "${flagKey}" was not returned by the bulk evaluation`;
			if (this.logging) {
				logger.warn(`[Flagship] ${msg}`);
			}
			return {
				value: defaultValue,
				reason: 'ERROR',
				errorCode: ErrorCode.FLAG_NOT_FOUND,
				errorMessage: msg,
			};
		}

		const flagMetadata = toFlagMetadata(cached.metadata);

		if (cached.errorCode) {
			const msg = cached.errorDetails ?? `Flag "${flagKey}" evaluation failed`;
			if (this.logging) {
				logger.warn(`[Flagship] ${msg}`);
			}
			return {
				value: defaultValue,
				reason: 'ERROR',
				errorCode: ERROR_CODES.has(cached.errorCode) ? (cached.errorCode as ErrorCode) : ErrorCode.GENERAL,
				errorMessage: msg,
			};
		}

		if (cached.reason === 'DISABLED') {
			return { value: defaultValue, reason: 'DISABLED', flagMetadata };
		}

		const actualType = this.getValueType(cached.value);
		if (actualType !== expectedType) {
			const msg = `Flag "${flagKey}" type mismatch: expected ${expectedType}, got ${actualType}`;
			if (this.logging) {
				logger.warn(`[Flagship] ${msg}`);
			}
			return {
				value: defaultValue,
				errorCode: ErrorCode.TYPE_MISMATCH,
				errorMessage: msg,
				reason: 'ERROR',
			};
		}

		return {
			value: cached.value as T,
			reason: cached.reason,
			variant: cached.variant,
			flagMetadata,
		};
	}

	private getValueType(value: unknown): string {
		if (typeof value === 'boolean') return 'boolean';
		if (typeof value === 'string') return 'string';
		if (typeof value === 'number') return 'number';
		return 'object';
	}
}

/**
 * Applies `headers` beneath `fetchOptions.headers` and maps `ofrepBaseUrl` to an
 * endpoint whose OFREP bulk URL is `{ofrepBaseUrl}/ofrep/v1/evaluate/flags`.
 */
function resolveOfrepOptions(options: FlagshipClientProviderOptions): FlagshipClientProviderOptions {
	const { ofrepBaseUrl, headers, ...rest } = options;
	let resolved: FlagshipClientProviderOptions = rest;
	if (headers) {
		const merged = new Headers(headers);
		new Headers(rest.fetchOptions?.headers).forEach((value, key) => merged.set(key, value));
		resolved = { ...rest, fetchOptions: { ...rest.fetchOptions, headers: merged } };
	}
	if (ofrepBaseUrl === undefined) return resolved;
	if (rest.appId || rest.endpoint) throw new Error('Flagship: provide only one of "ofrepBaseUrl", "appId", or "endpoint"');
	let url: URL;
	try {
		url = new URL(ofrepBaseUrl);
	} catch {
		throw new Error(`Flagship: invalid ofrepBaseUrl: ${ofrepBaseUrl}`);
	}
	if (url.protocol !== 'https:' && url.protocol !== 'http:')
		throw new Error(`Flagship: ofrepBaseUrl must use http or https: ${ofrepBaseUrl}`);
	url.search = '';
	url.hash = '';
	url.pathname = `${url.pathname.replace(/\/+$/, '')}/evaluate`;
	return { ...resolved, endpoint: url.toString() };
}

function resolveRelativeEndpoint(options: FlagshipClientProviderOptions): FlagshipClientProviderOptions {
	const { endpoint } = options;
	if (!endpoint || !endpoint.startsWith('/')) return options;

	if (typeof window === 'undefined' || !window.location?.origin) {
		throw new Error(`Flagship: relative endpoint "${endpoint}" requires a browser context with window.location.origin`);
	}

	return { ...options, endpoint: `${window.location.origin}${endpoint}` };
}

function toCache(response: FlagshipBulkEvaluationResponse): Map<string, CachedFlag> {
	return new Map(response.flags.map(({ key, ...flag }) => [key, flag]));
}

function changedKeys(previous: Map<string, CachedFlag>, next: Map<string, CachedFlag>): string[] {
	return [...new Set([...previous.keys(), ...next.keys()])].filter(
		(key) => JSON.stringify(previous.get(key)) !== JSON.stringify(next.get(key)),
	);
}
