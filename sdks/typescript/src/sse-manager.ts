import type { FlagshipEventStream } from './types.js';

type EventSourceConstructor = new (url: string | URL, eventSourceInitDict?: EventSourceInit) => EventSource;
type RefetchMetadata = { etag?: string; lastModified?: number };

type Callbacks = {
	onRefetch: (metadata?: RefetchMetadata) => void;
	onStale: (message: string) => void;
	onReady: () => void;
};

const DEFAULT_INACTIVITY_SECONDS = 120;
const DEBOUNCE_MS = 50;

export class SseManager {
	private sources: EventSource[] = [];
	private streams: FlagshipEventStream[] = [];
	private debounce?: ReturnType<typeof setTimeout>;
	private inactivity?: ReturnType<typeof setTimeout>;
	private pending?: RefetchMetadata;
	private visibility?: () => void;
	private stale = false;

	constructor(
		private readonly callbacks: Callbacks,
		private readonly EventSourceImpl: EventSourceConstructor,
		private readonly inactivityOverride?: number,
	) {}

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
		this.debounce = undefined;
		this.inactivity = undefined;
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
			this.debounce = setTimeout(() => {
				const metadata = this.pending;
				this.pending = undefined;
				this.callbacks.onRefetch(metadata);
			}, DEBOUNCE_MS);
		} catch {}
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
			if (this.sources.length === 0) {
				this.connect(this.streams);
				this.callbacks.onRefetch();
			}
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
