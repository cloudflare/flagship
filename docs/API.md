# @cloudflare/flagship — API Reference

OpenFeature provider SDK for Flagship feature flags. Supports server-side (Node.js, Cloudflare Workers) and client-side (browser) environments through separate sub-path exports with no shared dependencies between them.

## Installation

**Server-side** (Node.js, Cloudflare Workers):

```bash
npm install @cloudflare/flagship @openfeature/server-sdk
```

**Client-side** (browser):

```bash
npm install @cloudflare/flagship @openfeature/web-sdk
```

---

## Server-side usage

`FlagshipServerProvider` evaluates flags by making an HTTP request on each call. Designed for server environments where the evaluation context changes per-request (per-user targeting).

### Quick start

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(
  new FlagshipServerProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
  }),
);

const client = OpenFeature.getClient();

const enabled = await client.getBooleanValue('dark-mode', false, {
  targetingKey: 'user-123',
  email: 'user@example.com',
  plan: 'premium',
});
```

### Flag types

All four OpenFeature flag types are supported:

```typescript
const enabled  = await client.getBooleanValue('new-checkout', false, context);
const variant  = await client.getStringValue('homepage-hero', 'control', context);
const limit    = await client.getNumberValue('upload-limit-mb', 10, context);
const config   = await client.getObjectValue('ui-config', { theme: 'light' }, context);
```

### Evaluation details

Use the `*Details` methods when you need the full resolution result — reason, variant, and error information alongside the value:

```typescript
const details = await client.getBooleanDetails('my-flag', false, context);

console.log(details.value);        // resolved value (or default on error)
console.log(details.reason);       // 'TARGETING_MATCH' | 'SPLIT' | 'DEFAULT' | 'DISABLED' | 'ERROR'
console.log(details.variant);      // variation key, e.g. 'on', 'off', 'v2'
console.log(details.errorCode);    // set on error, e.g. 'FLAG_NOT_FOUND', 'TYPE_MISMATCH'
console.log(details.errorMessage); // human-readable error description
```

### Configuration

```typescript
new FlagshipServerProvider({
  // Option A (recommended): provide appId + accountId — the SDK constructs
  // the evaluation URL automatically.
  appId: 'your-app-id',
  accountId: 'your-account-id',

  // Override the base URL for local development or staging.
  // baseUrl: 'http://localhost:8787',

  // Option B (advanced): provide the full evaluation URL directly.
  // Mutually exclusive with appId.
  // endpoint: 'http://localhost:8787/v1/acct/apps/app-id/evaluate',

  // Authentication
  token: 'your-token', // adds Authorization: Bearer <token> to every request

  // Reliability
  timeout: 5000,    // request timeout in ms (default: 5000)
  retries: 1,       // retry attempts on transient errors (default: 1, max: 10)
  retryDelay: 1000, // delay between retries in ms (default: 1000, max: 30 000)

  // Logging — controls logs emitted by the Flagship SDK itself.
  // Does not affect OpenFeature framework logs (use OpenFeature.setLogger() for those).
  logging: false, // default: false

  // Advanced: custom fetch options passed to every request (e.g. custom headers)
  fetchOptions: { headers: { 'X-Custom-Header': 'value' } },
});
```

404 and 400 responses are never retried. Only transient server errors (5xx) and network failures trigger the retry logic.

### Cloudflare Workers example

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
    const userId = new URL(request.url).searchParams.get('userId') ?? 'anonymous';

    const darkMode = await client.getBooleanValue('dark-mode', false, {
      targetingKey: userId,
      country: request.headers.get('cf-ipcountry') ?? 'unknown',
    });

    return Response.json({ darkMode });
  },
};
```

---

## Client-side usage

`FlagshipClientProvider` is designed for browsers and other static-context environments. The OpenFeature web SDK requires synchronous flag resolution — this provider fetches all flags listed in `prefetchFlags` upfront during initialization and on every context change, then serves them from an in-memory cache.

This matches the pattern used by other production OpenFeature client providers (`ofrep-web`, `flagd-web`).

### Quick start

```typescript
import { OpenFeature } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '@cloudflare/flagship/web';

// 1. Initialize — fetches all prefetchFlags with empty context
await OpenFeature.setProviderAndWait(
  new FlagshipClientProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
    token: 'your-token',
    prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads'],
    logging: true, // log fetch errors and cache misses to the console
  }),
);

// 2. Set context — re-fetches all prefetchFlags for this user before resolving
await OpenFeature.setContext({
  targetingKey: 'user-123',
  plan: 'premium',
});

// 3. Resolve synchronously from cache
const client = OpenFeature.getClient();
const darkMode  = client.getBooleanValue('dark-mode', false);
const message   = client.getStringValue('welcome-message', 'Welcome!');
const uploads   = client.getNumberValue('max-uploads', 5);
```

### How the cache works

The cache is populated in two situations:

1. **`setProviderAndWait`** — all `prefetchFlags` are fetched with an empty context.
2. **`setContext(...)`** — the cache is cleared and all `prefetchFlags` are re-fetched for the new context.

All resolution methods (`getBooleanValue`, etc.) are **synchronous** and read from the cache only — no network requests happen at resolution time.

#### Cache miss — FLAG_NOT_FOUND

Any flag key **not** listed in `prefetchFlags`, or whose fetch failed, returns `FLAG_NOT_FOUND` at resolution time with the default value. Add it to `prefetchFlags` to have it cached.

| Situation                                    | `reason` | `errorCode`      | Value returned |
| -------------------------------------------- | -------- | ---------------- | -------------- |
| Flag fetched and cached                      | `CACHED` | —                | Cached value   |
| Flag not in `prefetchFlags`, or fetch failed | `ERROR`  | `FLAG_NOT_FOUND` | Default value  |
| Cached type doesn't match resolution type    | `ERROR`  | `TYPE_MISMATCH`  | Default value  |

```typescript
const details = client.getBooleanDetails('dark-mode', false);

if (details.errorCode === 'FLAG_NOT_FOUND') {
  // Flag was not in prefetchFlags or its fetch failed.
  // Add 'dark-mode' to prefetchFlags to fix this.
}

if (details.reason === 'CACHED') {
  // Happy path — value was served from cache.
}
```

### Configuration

```typescript
new FlagshipClientProvider({
  // Endpoint — same options as FlagshipServerProvider
  appId: 'your-app-id',
  accountId: 'your-account-id',
  // baseUrl: 'http://localhost:8787',
  // endpoint: 'http://localhost:8787/v1/...',

  // Authentication
  token: 'your-token',

  // Flag keys to fetch on initialization and on every context change.
  // Any key not listed here returns FLAG_NOT_FOUND at resolution time.
  prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads'],

  // Logging — controls logs emitted by the Flagship SDK (fetch errors, cache misses).
  // Does not affect OpenFeature framework logs (use OpenFeature.setLogger() for those).
  logging: false, // default: false

  // Reliability
  timeout: 5000,
  retries: 1,
  retryDelay: 1000,

  // Advanced: custom fetch options
  fetchOptions: { credentials: 'include' },
});
```

---

## Evaluation context

Context attributes are serialized as URL query parameters and sent with each evaluation request.

| Type                          | Serialization                                |
| ----------------------------- | -------------------------------------------- |
| `string`, `number`, `boolean` | Passed directly as a query parameter value   |
| `Date`                        | Converted to ISO 8601                        |
| Objects, arrays               | **Not supported** — throws `INVALID_CONTEXT` |

`targetingKey` is the standard OpenFeature field for the evaluation subject (user ID, session ID, etc.) and is treated like any other attribute.

---

## Authentication

All providers support the `token` option, which adds an `Authorization: Bearer <token>` header to every request:

```typescript
new FlagshipServerProvider({
  appId: 'your-app-id',
  accountId: 'your-account-id',
  token: 'your-secret-token',
});
```

If you also provide an `Authorization` header via `fetchOptions.headers`, the explicit header takes precedence and `token` is ignored for that slot.

---

## Logging

The `logging` option controls logs emitted directly by the Flagship SDK. It is `false` by default and applies to both providers.

```typescript
new FlagshipServerProvider({ ..., logging: true });
new FlagshipClientProvider({ ..., logging: true });
```

When enabled:

- **Server provider** — logs `debug` on every evaluation, `warn` on type mismatches, `error` on network/API failures, all via the OpenFeature-injected `Logger`.
- **Client provider** — logs `warn` via `console` for any flag that fails to fetch during initialization or context change, and for any cache miss at resolution time.

> **Note:** `logging` only controls Flagship SDK logs. OpenFeature's own framework-level logs are controlled separately via `OpenFeature.setLogger(myLogger)`.

---

## Error handling

Providers always return a valid `ResolutionDetails` — they never throw. On error, the default value is returned alongside `errorCode` and `errorMessage`.

```typescript
const details = await client.getBooleanDetails('my-flag', false, context);

if (details.errorCode) {
  console.error(`[${details.errorCode}] ${details.errorMessage}`);
}
```

### Error codes

| Code              | Cause                                                                     |
| ----------------- | ------------------------------------------------------------------------- |
| `FLAG_NOT_FOUND`  | Flag key not found (HTTP 404) or not in `prefetchFlags` (client provider) |
| `TYPE_MISMATCH`   | The flag's resolved value type does not match the requested type          |
| `INVALID_CONTEXT` | Evaluation context contains objects or arrays                             |
| `PARSE_ERROR`     | API response was not a valid evaluation response                          |
| `GENERAL`         | Network error, timeout, or other transient failure                        |

---

## Hooks

Two built-in hooks are available from `@cloudflare/flagship/server`.

### LoggingHook

Logs flag key, default value, context, resolved value, reason, and variant for every evaluation.

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider, LoggingHook } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(new FlagshipServerProvider({ appId: '...', accountId: '...' }));

OpenFeature.addHooks(new LoggingHook());

// Or pass a custom log function
OpenFeature.addHooks(new LoggingHook((message, ...args) => logger.debug(message, ...args)));
```

### TelemetryHook

Calls a user-supplied callback after each evaluation with timing and outcome data.

```typescript
import { TelemetryHook } from '@cloudflare/flagship/server';

OpenFeature.addHooks(
  new TelemetryHook((event) => {
    // event.type         — 'evaluation' | 'error'
    // event.flagKey      — flag key
    // event.timestamp    — Unix timestamp (ms)
    // event.duration     — evaluation duration in ms
    // event.value        — resolved value
    // event.reason       — resolution reason
    // event.variant      — variation key
    // event.errorCode    — OpenFeature error code (if resolution errored)
    // event.errorMessage
    // event.context      — evaluation context
    // event.hints        — hook hints from EvaluationOptions (optional)

    analytics.track('flag_evaluated', event);
  }),
);
```

---

## Provider events

```typescript
import { OpenFeature, ProviderEvents } from '@openfeature/server-sdk';

OpenFeature.addHandler(ProviderEvents.Ready, () => {
  console.log('Provider initialized and ready');
});

OpenFeature.addHandler(ProviderEvents.Error, ({ message }) => {
  console.error('Provider error:', message);
});

await OpenFeature.setProviderAndWait(provider);
```

**Server provider:** Probes the evaluation endpoint during initialization. A 404 response (flag not found) is treated as success — it confirms the endpoint is reachable. Network or timeout errors emit `ProviderEvents.Error` but `setProviderAndWait` still resolves.

**Client provider:** Fetches all `prefetchFlags` using `Promise.allSettled`. Even if some or all fetches fail, the provider transitions to `READY`. Failed flags return `FLAG_NOT_FOUND` when resolved.

---

## Exports

### `@cloudflare/flagship` (core — no OpenFeature dependency)

- `FlagshipClient` — HTTP client with retry, timeout, AbortController
- `ContextTransformer` — converts evaluation context to query parameters
- `FlagshipError` — error class with `code` and `cause` properties
- `FlagshipErrorCode` — `NETWORK_ERROR` | `TIMEOUT_ERROR` | `PARSE_ERROR` | `INVALID_CONTEXT`
- `FLAGSHIP_DEFAULT_BASE_URL` — default base URL constant
- Types: `FlagshipProviderOptions`, `FlagshipClientProviderOptions`, `FlagshipEvaluationResponse`, `CachedFlag`

### `@cloudflare/flagship/server`

Everything from core, plus:

- `FlagshipServerProvider`
- `LoggingHook`
- `TelemetryHook`
- Type: `TelemetryEvent`

### `@cloudflare/flagship/web`

Everything from core, plus:

- `FlagshipClientProvider`

---

## Architecture

```
@cloudflare/flagship/server
  FlagshipServerProvider        — OpenFeature Provider (server, async per-request)
    FlagshipClient              — HTTP client with retry + timeout
      ContextTransformer        — EvaluationContext → query parameters
    LoggingHook / TelemetryHook — OpenFeature hooks

@cloudflare/flagship/web
  FlagshipClientProvider        — OpenFeature Provider (client, sync from cache)
    FlagshipClient              — HTTP client (shared with server)
      ContextTransformer        — EvaluationContext → query parameters
    In-memory cache             — populated on initialize() and onContextChange()
```
