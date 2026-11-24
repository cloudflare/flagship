import type { FlagshipEventStream } from './types.js';

type EventSourceConstructor = new (url: string | URL, eventSourceInitDict?: EventSourceInit) => EventSource;

/** Change-event metadata to forward as `flagConfigEtag` and `flagConfigLastModified`. */
export type RefetchMetadata = { etag?: string; lastModified?: number | string };

/**
 * Why the manager needs a fresh stream: `'error'` after a dropped or failed
 * connection, `'resume'` after it closed an idle connection and the page became
 * visible again. OFREP requires a full unconditional refetch on `'resume'`.
 */
export type ReconnectReason = 'error' | 'resume';

type Callbacks = {
	/**
	 * Refetches after a change. `attempt` is 0 for the event itself and increases for
	 * retries. Resolves to the configuration version that was actually observed.
	 */
	onRefetch: (metadata: RefetchMetadata | undefined, attempt: number) => Promise<string | undefined> | void;
	/**
	 * Refetches and requests fresh stream descriptors. Tickets are single-use, so
	 * the provider must call `connect()` with the new descriptors.
	 */
	onReconnect: (reason: ReconnectReason, message: string) => Promise<unknown> | void;
};

const DEFAULT_INACTIVITY_SECONDS = 120;
const DEBOUNCE_MS = 50;
const MAX_METADATA_LENGTH = 256;
/**
 * Retry delays when a refetch still observes an older version, e.g. while an edge
 * cache catches up. Bounded so a missed or superseded version cannot loop forever.
 */
const STALE_RETRY_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 16_000];

export class SseManager {
	private sources: EventSource[] = [];
	private streams: FlagshipEventStream[] = [];
	private urls: string[] = [];
	private debounce?: ReturnType<typeof setTimeout>;
	private inactivity?: ReturnType<typeof setTimeout>;
	private retry?: ReturnType<typeof setTimeout>;
	private reconnect?: ReturnType<typeof setTimeout>;
	private reconnectAttempt = 0;
	private generation = 0;
	private pending?: RefetchMetadata;
	private visibility?: () => void;
	private stale = false;
	private idle = false;

	constructor(
		private readonly callbacks: Callbacks,
		private readonly EventSourceImpl: EventSourceConstructor,
		private readonly inactivityOverride?: number,
	) {}

	/**
	 * True while a stream is open and healthy. Streams persist across configuration
	 * versions, so providers neither request a new ticket nor reconnect while active.
	 */
	get active(): boolean {
		return this.sources.length > 0 && !this.stale;
	}

	/**
	 * True while the manager owns stream descriptors, including while it is closed
	 * for inactivity or waiting to reconnect. The provider then leaves visibility
	 * and recovery handling to the manager.
	 */
	get managing(): boolean {
		return this.streams.length > 0;
	}

	/** True when `streams` resolves to exactly the URLs this manager is connected to. */
	matches(streams: FlagshipEventStream[]): boolean {
		const next = usableStreams(streams).map(resolveUrl);
		return next.length === this.urls.length && next.every((url) => this.urls.includes(url!));
	}

	connect(streams: FlagshipEventStream[]): boolean {
		this.disconnect();
		this.streams = usableStreams(streams);
		this.urls = this.streams.map((stream) => resolveUrl(stream)!);
		if (this.streams.length === 0) return false;
		for (const url of this.urls) {
			let source: EventSource;
			try {
				source = new this.EventSourceImpl(url);
			} catch {
				continue;
			}
			source.addEventListener('message', (event) => {
				if (this.sources.includes(source)) this.message(event as MessageEvent);
			});
			source.addEventListener('error', () => {
				if (!this.sources.includes(source)) return;
				for (const item of this.sources) item.close();
				this.sources = [];
				this.recoverConnection('error', 'SSE connection interrupted');
			});
			this.sources.push(source);
		}
		this.installVisibility();
		if (this.sources.length === 0) {
			this.recoverConnection('error', 'SSE connection unavailable');
			return false;
		}
		this.stale = false;
		this.reconnectAttempt = 0;
		return true;
	}

	disconnect(): void {
		this.generation++;
		if (this.debounce) clearTimeout(this.debounce);
		if (this.inactivity) clearTimeout(this.inactivity);
		if (this.retry) clearTimeout(this.retry);
		if (this.reconnect) clearTimeout(this.reconnect);
		this.debounce = undefined;
		this.inactivity = undefined;
		this.retry = undefined;
		this.reconnect = undefined;
		this.pending = undefined;
		this.idle = false;
		for (const source of this.sources) source.close();
		this.sources = [];
		this.streams = [];
		this.urls = [];
		if (this.visibility && typeof document !== 'undefined') document.removeEventListener('visibilitychange', this.visibility);
		this.visibility = undefined;
	}

	private message(event: MessageEvent): void {
		const data = parseEvent(event.data);
		if (data?.type !== 'refetchEvaluation') return;
		this.pending = {
			...(isMetadataString(data.etag) ? { etag: data.etag } : {}),
			...(isLastModified(data.lastModified) ? { lastModified: data.lastModified } : {}),
		};
		if (this.debounce) clearTimeout(this.debounce);
		if (this.retry) clearTimeout(this.retry);
		this.retry = undefined;
		// Coalesce bursts, including the same change arriving on several streams.
		this.debounce = setTimeout(() => {
			const metadata = this.pending;
			this.pending = undefined;
			this.refetch(metadata, 0);
		}, DEBOUNCE_MS);
	}

	private refetch(metadata: RefetchMetadata | undefined, attempt: number): void {
		const generation = this.generation;
		void Promise.resolve()
			.then(async () => this.callbacks.onRefetch(metadata, attempt))
			.then((observed) => {
				if (this.generation !== generation) return;
				const delay = STALE_RETRY_DELAYS_MS[attempt];
				if (!metadata?.etag || observed === metadata.etag || delay === undefined) return;
				this.retry = setTimeout(() => this.refetch(metadata, attempt + 1), delay);
			})
			.catch(() => this.recoverConnection('error', 'SSE refresh failed'));
	}

	private recoverConnection(reason: ReconnectReason, message: string): void {
		if (this.reconnect) return;
		if (this.retry) clearTimeout(this.retry);
		this.retry = undefined;
		this.stale = true;
		const generation = this.generation;
		void Promise.resolve()
			.then(async () => this.callbacks.onReconnect(reason, message))
			.catch(() => undefined)
			.then(() => {
				if (this.generation !== generation || this.sources.length > 0 || this.reconnect || this.idle) return;
				const delay = Math.min(30_000, 500 * 2 ** Math.min(this.reconnectAttempt++, 6)) * (0.75 + Math.random() * 0.5);
				this.reconnect = setTimeout(() => {
					this.reconnect = undefined;
					if (this.generation === generation) this.recoverConnection('error', 'SSE reconnect failed');
				}, delay);
			});
	}

	private installVisibility(): void {
		if (typeof document === 'undefined' || this.visibility) return;
		this.visibility = () => {
			if (document.visibilityState === 'hidden') {
				const serverDelay = this.streams.find((stream) => stream.inactivityDelaySec !== undefined)?.inactivityDelaySec;
				const delay = this.inactivityOverride ?? serverDelay ?? DEFAULT_INACTIVITY_SECONDS;
				if (this.inactivity) clearTimeout(this.inactivity);
				this.inactivity = setTimeout(() => {
					this.inactivity = undefined;
					this.idle = true;
					if (this.reconnect) clearTimeout(this.reconnect);
					this.reconnect = undefined;
					for (const source of this.sources) source.close();
					this.sources = [];
				}, delay * 1_000);
				return;
			}
			if (this.inactivity) clearTimeout(this.inactivity);
			this.inactivity = undefined;
			if (!this.idle) return;
			// Tickets are single-use, so never reopen the old URL: the provider refetches
			// unconditionally, requests fresh descriptors, and reconnects with them.
			this.idle = false;
			this.recoverConnection('resume', 'SSE resumed after inactivity');
		};
		document.addEventListener('visibilitychange', this.visibility);
	}
}

function usableStreams(streams: FlagshipEventStream[]): FlagshipEventStream[] {
	return streams.filter((stream) => stream.type === 'sse' && Boolean(resolveUrl(stream)));
}

function resolveUrl(stream: FlagshipEventStream): string | undefined {
	if (stream.url) return stream.url;
	if (!stream.endpoint) return undefined;
	try {
		return new URL(stream.endpoint.requestUri, stream.endpoint.origin ?? globalThis.location?.origin).toString();
	} catch {
		return undefined;
	}
}

/** Parses string `data` as JSON and passes already-decoded data through, as OFREP specifies. */
function parseEvent(data: unknown): { type?: unknown; etag?: unknown; lastModified?: unknown } | undefined {
	try {
		const value: unknown = typeof data === 'string' ? JSON.parse(data) : data;
		return value && typeof value === 'object' ? (value as { type?: unknown }) : undefined;
	} catch {
		return undefined;
	}
}

function isMetadataString(value: unknown): value is string {
	return typeof value === 'string' && value.length > 0 && value.length <= MAX_METADATA_LENGTH;
}

/** OFREP allows Unix seconds or an ISO 8601 date-time string. */
function isLastModified(value: unknown): value is number | string {
	if (typeof value === 'number') return Number.isSafeInteger(value) && value >= 0;
	return isMetadataString(value) && !Number.isNaN(Date.parse(value));
}
