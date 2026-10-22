import type { FlagshipEventStream } from './types.js';

type EventSourceConstructor = new (url: string | URL, eventSourceInitDict?: EventSourceInit) => EventSource;
type RefetchMetadata = { etag?: string; lastModified?: number };

type Callbacks = {
	/**
	 * Refetches after a change. `attempt` is 0 for the event itself and increases for
	 * retries. Resolves to the configuration version that was actually observed.
	 */
	onRefetch: (metadata: RefetchMetadata | undefined, attempt: number) => Promise<string | undefined> | void;
	onStale: (message: string) => void;
	onReady: () => void;
};

const DEFAULT_INACTIVITY_SECONDS = 120;
const DEBOUNCE_MS = 50;
/**
 * Retry delays when a refetch still observes an older version, e.g. while an edge
 * cache catches up. Bounded so a missed or superseded version cannot loop forever.
 */
const STALE_RETRY_DELAYS_MS = [1_000, 2_000, 4_000, 8_000, 16_000];

export class SseManager {
	private sources: EventSource[] = [];
	private streams: FlagshipEventStream[] = [];
	private debounce?: ReturnType<typeof setTimeout>;
	private inactivity?: ReturnType<typeof setTimeout>;
	private retry?: ReturnType<typeof setTimeout>;
	private pending?: RefetchMetadata;
	private visibility?: () => void;
	private stale = false;

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

	connect(streams: FlagshipEventStream[]): boolean {
		this.disconnect();
		this.streams = streams.filter((stream) => stream.type === 'sse' && Boolean(resolveUrl(stream)));
		if (this.streams.length === 0) return false;
		for (const stream of this.streams) {
			const url = resolveUrl(stream);
			if (!url) continue;
			const source = new this.EventSourceImpl(url);
			source.addEventListener('open', () => {
				if (this.stale) {
					this.stale = false;
					this.callbacks.onReady();
				}
			});
			source.addEventListener('message', (event) => this.message(event as MessageEvent));
			source.addEventListener('error', () => {
				if (this.stale) return;
				this.stale = true;
				this.callbacks.onStale('SSE connection interrupted');
			});
			this.sources.push(source);
		}
		this.installVisibility();
		return this.sources.length > 0;
	}

	disconnect(): void {
		if (this.debounce) clearTimeout(this.debounce);
		if (this.inactivity) clearTimeout(this.inactivity);
		if (this.retry) clearTimeout(this.retry);
		this.debounce = undefined;
		this.inactivity = undefined;
		this.retry = undefined;
		this.pending = undefined;
		for (const source of this.sources) source.close();
		this.sources = [];
		if (this.visibility && typeof document !== 'undefined') document.removeEventListener('visibilitychange', this.visibility);
		this.visibility = undefined;
	}

	private message(event: MessageEvent): void {
		try {
			const data = JSON.parse(String(event.data)) as { type?: unknown; etag?: unknown; lastModified?: unknown };
			if (data.type !== 'refetchEvaluation') return;
			this.pending = {
				...(typeof data.etag === 'string' ? { etag: data.etag } : {}),
				...(typeof data.lastModified === 'number' ? { lastModified: data.lastModified } : {}),
			};
			if (this.debounce) clearTimeout(this.debounce);
			if (this.retry) clearTimeout(this.retry);
			this.retry = undefined;
			this.debounce = setTimeout(() => {
				const metadata = this.pending;
				this.pending = undefined;
				this.refetch(metadata, 0);
			}, DEBOUNCE_MS);
		} catch {}
	}

	private refetch(metadata: RefetchMetadata | undefined, attempt: number): void {
		void Promise.resolve(this.callbacks.onRefetch(metadata, attempt)).then((observed) => {
			const delay = STALE_RETRY_DELAYS_MS[attempt];
			if (!metadata?.etag || observed === undefined || observed === metadata.etag || delay === undefined) return;
			this.retry = setTimeout(() => this.refetch(metadata, attempt + 1), delay);
		});
	}

	private installVisibility(): void {
		if (typeof document === 'undefined') return;
		this.visibility = () => {
			if (document.visibilityState === 'hidden') {
				const serverDelay = this.streams.find((stream) => stream.inactivityDelaySec !== undefined)?.inactivityDelaySec;
				const delay = this.inactivityOverride ?? serverDelay ?? DEFAULT_INACTIVITY_SECONDS;
				this.inactivity = setTimeout(() => {
					for (const source of this.sources) source.close();
					this.sources = [];
				}, delay * 1_000);
				return;
			}
			if (this.inactivity) clearTimeout(this.inactivity);
			this.inactivity = undefined;
			// Tickets are single-use, so never reopen the old URL: refetching while
			// inactive requests a fresh descriptor and the provider reconnects with it.
			if (this.sources.length === 0) this.refetch(undefined, 0);
		};
		document.addEventListener('visibilitychange', this.visibility);
	}
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
