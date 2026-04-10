# @cloudflare/flagship

[![npm version](https://img.shields.io/npm/v/@cloudflare/flagship.svg)](https://www.npmjs.com/package/@cloudflare/flagship)
[![npm downloads](https://img.shields.io/npm/dm/@cloudflare/flagship.svg)](https://www.npmjs.com/package/@cloudflare/flagship)
[![license](https://img.shields.io/npm/l/@cloudflare/flagship.svg)](https://github.com/cloudflare/flagship/blob/main/LICENSE)

[OpenFeature](https://openfeature.dev)-compliant provider SDK for [Flagship](https://github.com/cloudflare/flagship), Cloudflare's globally distributed, low-latency feature flag platform.

Server-side (Node.js, Cloudflare Workers) and client-side (browser) support via isolated sub-path exports. Tree-shakeable — importing `@cloudflare/flagship/server` never loads `@openfeature/web-sdk` and vice versa.

## Install

**Server-side** (Node.js, Cloudflare Workers):

```bash
npm install @cloudflare/flagship @openfeature/server-sdk
```

**Client-side** (browser):

```bash
npm install @cloudflare/flagship @openfeature/web-sdk
```

## Quick start — server

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(
  new FlagshipServerProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
    token: 'your-token',
  }),
);

const client = OpenFeature.getClient();
const enabled = await client.getBooleanValue('dark-mode', false, {
  targetingKey: 'user-123',
  plan: 'premium',
});
```

## Quick start — Cloudflare Workers

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

let initialized = false;

export default {
  async fetch(request: Request): Promise<Response> {
    if (!initialized) {
      await OpenFeature.setProviderAndWait(
        new FlagshipServerProvider({
          appId: 'your-app-id',
          accountId: 'your-account-id',
          token: 'your-token',
        }),
      );
      initialized = true;
    }

    const client = OpenFeature.getClient();
    const darkMode = await client.getBooleanValue('dark-mode', false, {
      targetingKey: new URL(request.url).searchParams.get('userId') ?? 'anonymous',
    });

    return Response.json({ darkMode });
  },
};
```

## Quick start — browser

```typescript
import { OpenFeature } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '@cloudflare/flagship/web';

// 1. Initialize — fetches all prefetchFlags with empty context
await OpenFeature.setProviderAndWait(
  new FlagshipClientProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
    token: 'your-token',
    // List every flag your app uses. Flags not listed here return FLAG_NOT_FOUND.
    prefetchFlags: ['dark-mode', 'welcome-message'],
  }),
);

// 2. Set context — re-fetches all prefetchFlags for this user
await OpenFeature.setContext({ targetingKey: 'user-123', plan: 'premium' });

// 3. Resolve synchronously from cache
const client = OpenFeature.getClient();
const darkMode = client.getBooleanValue('dark-mode', false); // reason: 'CACHED'
```

## Features

| Feature               | Description                                                              |
| --------------------- | ------------------------------------------------------------------------ |
| OpenFeature compliant | Implements the CNCF OpenFeature specification                            |
| Server + client       | Async per-request (server) and sync cache-based (browser) providers      |
| All flag types        | Boolean, string, number, and object (JSON)                               |
| Authentication        | `token` option adds `Authorization: Bearer` to every request             |
| Logging               | `logging` option surfaces fetch errors and cache misses (off by default) |
| Retries + timeouts    | Configurable retry logic with `AbortController`-based timeouts           |
| Hooks                 | Built-in `LoggingHook` and `TelemetryHook`                               |
| Tree-shakeable        | Server and client bundles are fully isolated                             |
| TypeScript            | Strict types throughout                                                  |

## Packages

| Export                        | Description                      | Peer dependency           |
| ----------------------------- | -------------------------------- | ------------------------- |
| `@cloudflare/flagship`        | Core client, types, errors       | None                      |
| `@cloudflare/flagship/server` | `FlagshipServerProvider` + hooks | `@openfeature/server-sdk` |
| `@cloudflare/flagship/web`    | `FlagshipClientProvider`         | `@openfeature/web-sdk`    |

## Client provider — how the cache works

The `FlagshipClientProvider` follows the same pattern as other production OpenFeature client providers (`ofrep-web`, `flagd-web`):

1. All flags listed in `prefetchFlags` are fetched in parallel during `initialize()` and on every `setContext()` call.
2. Resolution methods (`getBooleanValue`, etc.) are **synchronous** and read from the in-memory cache only — no network at resolution time.
3. Any flag **not** in `prefetchFlags` returns `FLAG_NOT_FOUND` immediately.

| `reason` | `errorCode`      | Meaning                                          |
| -------- | ---------------- | ------------------------------------------------ |
| `CACHED` | —                | Flag fetched and served from cache               |
| `ERROR`  | `FLAG_NOT_FOUND` | Flag not in `prefetchFlags`, or its fetch failed |
| `ERROR`  | `TYPE_MISMATCH`  | Cached type doesn't match the resolution type    |

## Documentation

- [Full API reference](../../docs/API.md)
- [OpenFeature specification](https://openfeature.dev/specification/)
- [Examples](./examples/)

## Development

```bash
pnpm install         # install dependencies
pnpm run dev         # watch mode
pnpm run test        # run tests
pnpm run build       # build for distribution
```

## Contributing

Contributions are welcome. Please open an issue first to discuss what you'd like to change. See the [repository](https://github.com/cloudflare/flagship) for more details.

## License

[Apache-2.0](../../LICENSE)
