import { afterEach, describe, expect, it, vi } from 'vitest';
import { ErrorCode, ProviderEvents } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '../src/server-provider.js';

const flag = {
	key: 'f',
	enabled: true,
	default_variation: 'on',
	variations: { on: true },
	rules: [],
};

const ok = (): Response => Response.json({ flags: { f: flag } }, { headers: { etag: '"v1"' } });
const failure = (status: number): Response => new Response(null, { status });

function localProvider(responses: Array<() => Response>) {
	const fetch = vi.fn(async () => responses.shift()!());
	return new FlagshipServerProvider({
		appId: 'app-1',
		accountId: 'acct-1',
		localEvaluation: true,
		fetch,
		refreshInterval: 1000,
		retries: 0,
	});
}

async function refresh(provider: FlagshipServerProvider): Promise<void> {
	await vi.advanceTimersByTimeAsync(1000);
	await provider.resolveBooleanEvaluation('f', false, {}, console);
	await vi.advanceTimersByTimeAsync(0);
}

describe('FlagshipServerProvider lifecycle', () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it.each([401, 403])('fails initialization with PROVIDER_FATAL on %i', async (status) => {
		const provider = localProvider([() => failure(status)]);
		await expect(provider.initialize()).rejects.toMatchObject({ code: ErrorCode.PROVIDER_FATAL });
	});

	it('fails initialization with the original error on other failures', async () => {
		const provider = localProvider([() => failure(500)]);
		const error = await provider.initialize().catch((e: unknown) => e);
		expect(error).toMatchObject({ message: expect.stringContaining('500') });
		expect(error).not.toMatchObject({ code: ErrorCode.PROVIDER_FATAL });
	});

	it('emits STALE on the first failed refresh, then READY once it recovers', async () => {
		vi.useFakeTimers();
		const provider = localProvider([ok, () => failure(500), () => failure(500), ok]);
		const stale = vi.fn();
		const ready = vi.fn();
		provider.events.addHandler(ProviderEvents.Stale, stale);
		provider.events.addHandler(ProviderEvents.Ready, ready);
		await provider.initialize();

		await refresh(provider);
		await refresh(provider);
		expect(stale).toHaveBeenCalledTimes(1);
		expect(ready).not.toHaveBeenCalled();

		await refresh(provider);
		expect(ready).toHaveBeenCalledTimes(1);
	});
});
