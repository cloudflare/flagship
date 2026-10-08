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

## Server-side usage

`FlagshipServerProvider` supports three modes of operation:

- **Binding mode** (recommended for Cloudflare Workers) — evaluates flags via a wrangler binding (`env.FLAGS`). No HTTP overhead, no auth tokens.
- **HTTP mode** — evaluates flags via HTTP requests to the Flagship API. Works in any server environment.
- **Local evaluation mode** — downloads flag definitions once, evaluates in-process, and refreshes definitions lazily. Requires `localEvaluation: true` plus HTTP config.

The constructor accepts a discriminated union: provide **either** a `binding` **or** HTTP config (`appId`/`endpoint`, `accountId`, etc.) — never both. Providing both throws immediately. Local evaluation is HTTP-only and incompatible with `binding` and `cacheTtl`.

### Quick start — Cloudflare Workers

Configure the Flagship binding in `wrangler.json`:

```jsonc
{
  "flagship": [{ "binding": "FLAGS", "app_id": "<your-app-id>" }]
}
```

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';
import type { FlagshipBinding } from '@cloudflare/flagship/server';

export default {
  async fetch(request: Request, env: { FLAGS: FlagshipBinding }): Promise<Response> {
    await OpenFeature.setProviderAndWait(
      new FlagshipServerProvider({ binding: env.FLAGS }),
    );

    const client = OpenFeature.getClient();
    const enabled = await client.getBooleanValue('dark-mode', false, {
      targetingKey: 'user-123',
    });

    return Response.json({ enabled });
  },
};
```

### Quick start — HTTP

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(
  new FlagshipServerProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
    authToken: 'your-token'
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
// Boolean — feature on/off
const enabled = await client.getBooleanValue('new-checkout', false, context);

// String — A/B test variants, copy experiments
const variant = await client.getStringValue('homepage-hero', 'control', context);

// Number — rate limits, thresholds, percentages
const limit = await client.getNumberValue('upload-limit-mb', 10, context);

// Object — complex configuration (JSON)
const config = await client.getObjectValue('ui-config', { theme: 'light' }, context);
```

### Evaluation details

Use the `*Details` methods when you need the full resolution result — reason, variant, and error information alongside the value:

```typescript
const details = await client.getBooleanDetails('my-flag', false, context);

console.log(details.value); // resolved value (or default on error)
console.log(details.reason); // 'STATIC' | 'TARGETING_MATCH' | 'SPLIT' | 'DEFAULT' | 'DISABLED' | 'ERROR'
console.log(details.variant); // variation key, e.g. 'on', 'off', 'v2'
console.log(details.errorCode); // set on error, e.g. 'FLAG_NOT_FOUND', 'TYPE_MISMATCH'
console.log(details.errorMessage); // human-readable description of the error
```

### Configuration — binding mode

```typescript
// Binding mode: just pass the binding from env. That's it.
new FlagshipServerProvider({
  binding: env.FLAGS,

  // Logging — controls logs emitted by the Flagship SDK itself (default: false)
  logging: false,

  // Caching — opt-in, off by default. See "Caching" below.
  cacheTtl: 30000, // ms; enables the cache when > 0
  cacheMaxSize: 1000, // max cached entries (default: 1000)
});
```

No `appId`, `accountId`, `authToken`, timeout, or retry settings are needed — the binding handles all of that internally.

### Configuration — HTTP mode

```typescript
new FlagshipServerProvider({
  // Option A (recommended): provide appId + accountId and the SDK
  // constructs the evaluation URL automatically.
  appId: 'your-app-id',
  accountId: 'your-account-id',

  // Optional: override the base URL for local development or staging.
  // baseUrl: 'http://localhost:8787',

  // Option B (advanced): provide the full evaluation URL directly.
  // Mutually exclusive with appId.
  // endpoint: 'http://localhost:8787/v1/acct/apps/app-id/evaluate',

  // Authentication
  authToken: 'your-token', // adds Authorization: Bearer <token> to every request

  // Logging — controls logs emitted by the Flagship SDK itself (default: false)
  // Does not affect OpenFeature framework logs (use OpenFeature.setLogger() for those).
  logging: false,

  timeout: 5000, // request timeout in ms (default: 5000)
  retries: 1, // retry attempts on transient errors (default: 1, max: 10)
  retryDelay: 1000, // delay between retries in ms (default: 1000, max: 30000)

  // Custom transport (default: globalThis.fetch, resolved at call time).
  // Useful for routing evaluations through a service binding, or for tests.
  // fetch: env.FLAGS_SERVICE.fetch.bind(env.FLAGS_SERVICE),

  // Caching — opt-in, off by default. See "Caching" below.
  // Incompatible with localEvaluation.
  cacheTtl: 30000, // ms; enables the cache when > 0
  cacheMaxSize: 1000, // max cached entries (default: 1000)

  // Local evaluation — download definitions once and evaluate in-process.
  // Requires a token with app **read** permission. See "Local evaluation" below.
  // localEvaluation: true,
  // refreshInterval: 30000, // ms between lazy background refreshes (default 30000)
});
```

Only transient failures are retried. Everything else is treated as a definitive answer and propagated immediately:

| Outcome                                    | Retried? | `FlagshipError.code` | `retryable` |
| ------------------------------------------ | -------- | -------------------- | ----------- |
| 408, 425, 429                              | yes      | `NETWORK_ERROR`      | `true`      |
| 5xx                                        | yes      | `NETWORK_ERROR`      | `true`      |
| Connection failure                         | yes      | `NETWORK_ERROR`      | `true`      |
| Timeout                                    | yes      | `TIMEOUT_ERROR`      | `true`      |
| Malformed response body                    | yes      | `PARSE_ERROR`        | `true`      |
| Other non-2xx (400, 401, 403, 404, 422, …) | no       | `NETWORK_ERROR`      | `false`     |
| Caller abort                               | no       | `ABORTED`            | `false`     |
| Unserializable evaluation context          | no       | `INVALID_CONTEXT`    | `false`     |

For HTTP failures the `FlagshipError.cause` is the underlying `Response`, so the status is available for inspection. Only a `retryable: false` failure is a definitive answer that is safe to cache; a `retryable: true` failure means "ask again later".

### Caching

The server provider can cache evaluations in both HTTP and binding modes. Caching is **off by default** and enabled by setting `cacheTtl` to a positive number of milliseconds.

```typescript
new FlagshipServerProvider({
  appId: 'your-app-id',
  accountId: 'your-account-id',
  cacheTtl: 30000,
  cacheMaxSize: 1000,
});
```

| Option         | Type     | Default | Description                                                       |
| -------------- | -------- | ------- | ----------------------------------------------------------------- |
| `cacheTtl`     | `number` | —       | Time-to-live per entry in ms. Enables caching when `> 0`.         |
| `cacheMaxSize` | `number` | `1000`  | Maximum cached entries; the least-recently-used entry is evicted. |

| Situation                                     | `reason`                           | Cached?           |
| --------------------------------------------- | ---------------------------------- | ----------------- |
| Successful evaluation served from API/binding | original (`TARGETING_MATCH`, etc.) | yes               |
| Same flag + context within the TTL            | `CACHED`                           | served from cache |
| Disabled flag                                 | `DISABLED`                         | no                |
| Any error (not found, timeout, etc.)          | `ERROR`                            | no                |

Each entry is keyed by flag key, expected type, and the **full evaluation context**, so distinct contexts never share a cached value. Because freshness is TTL-based, a flag change in Flagship takes effect once the entry expires (up to `cacheTtl` later). The cache is per-provider-instance and is cleared on `onClose()`.

### Local evaluation

When `localEvaluation: true`, the provider fetches flag definitions during `initialize()` (blocking), evaluates flags in-process, and refreshes definitions lazily on evaluate once `refreshInterval` has elapsed. Refresh uses `If-None-Match` / `304`. Failures keep the last good snapshot and emit `PROVIDER_STALE`; the next successful refresh emits `PROVIDER_READY`. An initial fetch that fails rejects `initialize()`, with `PROVIDER_FATAL` on a 401 or 403.

```typescript
new FlagshipServerProvider({
  appId: 'your-app-id',
  accountId: 'your-account-id', // required — rollout hash seed
  authToken: 'your-read-token', // app **read** permission
  localEvaluation: true,
  refreshInterval: 30_000, // default 30_000
});
```

| Option            | Type      | Default  | Description                                                            |
| ----------------- | --------- | -------- | ---------------------------------------------------------------------- |
| `localEvaluation` | `boolean` | `false`  | Enable in-process evaluation from downloaded definitions.              |
| `refreshInterval` | `number`  | `30_000` | Minimum ms between background definition refreshes (lazy on evaluate). |

| Constraint                           | Behaviour                                                  |
| ------------------------------------ | ---------------------------------------------------------- |
| `localEvaluation` + `binding`        | Throws at construction                                     |
| `localEvaluation` + `cacheTtl`       | Throws at construction                                     |
| Missing `accountId`                  | Throws at construction                                     |
| `endpoint` not ending in `/evaluate` | Throws at construction (definitions URL cannot be derived) |

Error mapping in local mode:

| Condition                                   | OpenFeature error    |
| ------------------------------------------- | -------------------- |
| Flag key not in snapshot                    | `FLAG_NOT_FOUND`     |
| Missing / invalid variation on the flag     | `PARSE_ERROR`        |
| No snapshot yet (before init / after close) | `PROVIDER_NOT_READY` |
| Wrong type vs requested OpenFeature type    | `TYPE_MISMATCH`      |

Notes:

- Local evaluations do **not** appear in server-side analytics.
- Context is normalized with the same rules as HTTP mode (dates → ISO strings). Server request-size limits are **not** re-applied locally — a known difference from the network path.
- Reason `STATIC` is returned when a flag has no rules (also surfaced in HTTP mode when the API returns it).

### Cloudflare Workers example (binding)

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';
import type { FlagshipBinding } from '@cloudflare/flagship/server';

export default {
  async fetch(request: Request, env: { FLAGS: FlagshipBinding }): Promise<Response> {
    await OpenFeature.setProviderAndWait(
      new FlagshipServerProvider({ binding: env.FLAGS }),
    );

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

### Cloudflare Workers example (HTTP)

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

let initialized = false;

export default {
  async fetch(request: Request): Promise<Response> {
    if (!initialized) {
      await OpenFeature.setProviderAndWait(
        new FlagshipServerProvider({ appId: 'your-app-id', accountId: 'your-account-id' }),
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

## Client-side usage

The `FlagshipClientProvider` is designed for browsers and other static-context environments. The OpenFeature web SDK requires synchronous flag resolution, so this provider evaluates every flag for the current context in one [OFREP](https://github.com/open-feature/protocol) bulk request whenever the context changes and serves the results from an in-memory cache.

### Basic usage

```typescript
import { OpenFeature } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '@cloudflare/flagship/web';

await OpenFeature.setProviderAndWait(
  new FlagshipClientProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
    authToken: 'your-token',
    logging: true, // log fetch errors and cache misses to the console
  }),
);

// Setting context evaluates all flags for the new context before the promise resolves.
await OpenFeature.setContext({
  targetingKey: 'user-123',
  plan: 'premium',
});

const client = OpenFeature.getClient();

// All resolution is synchronous — values come from the cache.
const darkMode = client.getBooleanValue('dark-mode', false);
const message = client.getStringValue('welcome-message', 'Welcome!');
const uploads = client.getNumberValue('max-uploads', 5);
```

### Cache behavior

| Situation                                  | `reason`              | `errorCode`             | Value returned  |
| ------------------------------------------ | --------------------- | ----------------------- | --------------- |
| Flag evaluated successfully                | The evaluation reason | —                       | Evaluated value |
| Flag is disabled                           | `DISABLED`            | —                       | Default value   |
| Flag failed to evaluate on the server      | `ERROR`               | The server's error code | Default value   |
| Flag missing from the response             | `ERROR`               | `FLAG_NOT_FOUND`        | Default value   |
| Cached value's type doesn't match the call | `ERROR`               | `TYPE_MISMATCH`         | Default value   |

When the context changes, the entire cache is **cleared before re-evaluating**. A failed request returns `FLAG_NOT_FOUND` rather than serving stale values from the previous context, and reports `PROVIDER_ERROR` (`PROVIDER_FATAL` on a 401 or 403).

The bulk request requires an endpoint ending in `/evaluate`, or `appId` and `accountId`.

### Configuration options

| Option         | Type          | Default                      | Description                                           |
| -------------- | ------------- | ---------------------------- | ----------------------------------------------------- |
| `appId`        | `string`      | —                            | Flagship app ID (mutually exclusive with `endpoint`)  |
| `accountId`    | `string`      | —                            | Account ID (required with `appId`)                    |
| `baseUrl`      | `string`      | `https://api.cloudflare.com` | Base URL override (only used with `appId`)            |
| `endpoint`     | `string`      | —                            | Full evaluation URL (mutually exclusive with `appId`) |
| `authToken`    | `string`      | —                            | Bearer token — adds `Authorization: Bearer` header    |
| `logging`      | `boolean`     | `false`                      | Log fetch errors and cache misses to the console      |
| `timeout`      | `number`      | `5000`                       | Request timeout in ms                                 |
| `retries`      | `number`      | `1`                          | Retry attempts (max 10)                               |
| `retryDelay`   | `number`      | `1000`                       | Delay between retries in ms (max 30 000)              |
| `fetchOptions` | `RequestInit` | `{}`                         | Custom fetch options (headers, credentials, etc.)     |

## Evaluation context

Primitive-only context is serialized as URL query parameters. Context containing objects, arrays, or `null` is sent as a JSON request body, which adds a CORS preflight request in browsers.

| Type                          | Serialization                                                |
| ----------------------------- | ------------------------------------------------------------ |
| `string`, `number`, `boolean` | Preserved; primitive-only context uses GET query parameters  |
| `Date`                        | Recursively converted to ISO 8601 strings                    |
| Objects, arrays, `null`       | Preserved recursively; evaluations use POST with a JSON body |
| Other values, cyclic values   | Rejected before transport with `INVALID_CONTEXT`             |

`targetingKey` is the standard field for identifying the evaluation subject (user ID, session ID, etc.) and is treated like any other attribute.

## Authentication

All providers support the `authToken` option, which adds an `Authorization: Bearer <token>` header to every request:

```typescript
new FlagshipServerProvider({ appId: 'your-app-id', accountId: 'your-account-id', authToken: 'your-secret-token' });
```

If you also provide an `Authorization` header via `fetchOptions.headers`, the explicit header takes precedence and `authToken` is ignored for that slot.

## Logging

The `logging` option controls logs emitted directly by the Flagship SDK. It is `false` by default and applies to both providers.

```typescript
new FlagshipServerProvider({ ..., logging: true });
new FlagshipClientProvider({ ..., logging: true });
```

When enabled, the server provider logs via the OpenFeature-injected `Logger` (debug on evaluation, warn on type mismatch, error on failures). The client provider logs `console.warn` when the bulk request fails and for any cache miss at resolution time.

> Note: `logging` only controls Flagship SDK logs. OpenFeature's own framework-level logs are controlled separately via `OpenFeature.setLogger(myLogger)`.

## Custom transport and cancellation

`FlagshipClient` resolves its transport from `options.fetch`, falling back to `globalThis.fetch` at call time. The SDK never assigns to `globalThis.fetch`, so injecting a transport cannot affect unrelated traffic in the same isolate.

`evaluate()` also accepts per-call overrides:

```typescript
import { FlagshipClient, FlagshipErrorCode, FlagshipError } from '@cloudflare/flagship';

const client = new FlagshipClient({ appId: 'your-app-id', accountId: 'your-account-id' });

try {
  const result = await client.evaluate('my-flag', context, {
    // Aborting this signal aborts the in-flight HTTP request.
    signal: request.signal,
    // Optional per-call transport override.
    fetch: env.FLAGS_SERVICE.fetch.bind(env.FLAGS_SERVICE),
  });
} catch (error) {
  if (error instanceof FlagshipError && error.code === FlagshipErrorCode.ABORTED) {
    // The caller cancelled — not a Flagship failure, and never retried.
  }
}
```

Signal semantics:

- A caller signal is **merged** with the request timeout and with `fetchOptions.signal` — whichever fires first aborts the request. None of them is discarded.
- An already-aborted signal rejects with `ABORTED` before any request is issued.
- A caller abort is never retried; a timeout abort is retried as usual.
- Caller aborts (`ABORTED`) and timeouts (`TIMEOUT_ERROR`) are distinct error codes.

Both providers accept `fetch` in HTTP mode and forward it to the underlying client. It must not be combined with `binding`.

## Error handling

The provider always returns a valid `ResolutionDetails` — it never throws. On error, the default value is returned alongside an `errorCode` and `errorMessage` describing what went wrong.

```typescript
const details = await client.getBooleanDetails('my-flag', false, context);

if (details.errorCode) {
  // The default value was returned. Inspect errorCode to understand why.
  console.error(`[${details.errorCode}] ${details.errorMessage}`);
}
```

### Error codes

| Code              | Cause                                                                                   |
| ----------------- | --------------------------------------------------------------------------------------- |
| `FLAG_NOT_FOUND`  | Flag key does not exist (HTTP 404), or missing from the bulk response (client provider) |
| `TYPE_MISMATCH`   | The flag's resolved value type does not match the requested type                        |
| `INVALID_CONTEXT` | The evaluation context contains objects or arrays                                       |
| `PARSE_ERROR`     | The API response was not a valid evaluation response                                    |
| `GENERAL`         | Network error, timeout, caller abort, or any other transient failure                    |

## Hooks

OpenFeature hooks run at defined points in the evaluation lifecycle. Two built-in hooks are available from `@cloudflare/flagship/server`.

### LoggingHook

Logs flag key, default value, context, resolved value, reason, and variant for every evaluation.

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider, LoggingHook } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(new FlagshipServerProvider({ appId: '...', accountId: '...' }));

// Uses console.log by default
OpenFeature.addHooks(new LoggingHook());

// Or pass a custom log function (message: string, ...args: unknown[])
OpenFeature.addHooks(new LoggingHook((message, ...args) => logger.debug(message, ...args)));
```

### TelemetryHook

Calls a user-supplied callback after each evaluation with timing and outcome data. Useful for sending flag evaluation metrics to an analytics or observability service.

```typescript
import { TelemetryHook } from '@cloudflare/flagship/server';

OpenFeature.addHooks(
  new TelemetryHook((event) => {
    // event.type        — 'evaluation' | 'error'
    // event.flagKey     — flag key
    // event.timestamp   — Unix timestamp (ms)
    // event.duration    — evaluation duration in ms
    // event.value       — resolved value (evaluation events only)
    // event.reason      — resolution reason
    // event.variant     — variation key
    // event.errorCode   — OpenFeature error code (set on evaluation events when the resolution produced an error)
    // event.errorMessage
    // event.context     — evaluation context
    // event.hints       — hook hints from EvaluationOptions (optional)

    analytics.track('flag_evaluated', event);
  }),
);
```

## Provider events

The provider emits standard OpenFeature events during its lifecycle:

```typescript
import { OpenFeature, ProviderEvents } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

const provider = new FlagshipServerProvider({ appId: 'your-app-id', accountId: 'your-account-id' });

OpenFeature.addHandler(ProviderEvents.Ready, () => {
  console.log('Provider initialized and ready');
});

await OpenFeature.setProviderAndWait(provider);
```

**Server provider:** Initialization does not perform network I/O. Flag evaluation requests happen only when resolving flags.

**Server provider (binding mode):** Initialization does not call binding methods. Binding evaluation requests happen only when resolving flags.

**Client provider:** During initialization, the provider evaluates every flag in one bulk request. If the request fails, `initialize()` rejects and the provider reports `PROVIDER_ERROR` (`PROVIDER_FATAL` on a 401 or 403). `onContextChange` behaves the same way.

To shut down providers and release resources:

```typescript
await OpenFeature.clearProviders();
// Client provider also clears the in-memory cache
```

## Exports

Each sub-path re-exports core utilities alongside its provider-specific classes.

**`@cloudflare/flagship`** (core — no OpenFeature dependency):

- `FlagshipClient` — HTTP client with retry, timeout, AbortController
- `ContextTransformer` — normalizes context and selects query or JSON transport
- `FlagshipError` — error class with `code`, `cause`, and `retryable` properties
- `FlagshipErrorCode` — enum: `NETWORK_ERROR`, `TIMEOUT_ERROR`, `ABORTED`, `PARSE_ERROR`, `INVALID_CONTEXT`
- `isBindingOptions()` — type guard for binding options
- `FLAGSHIP_DEFAULT_BASE_URL` — default base URL constant
- Types: `FlagshipProviderOptions`, `FlagshipRequestOptions`, `FlagshipClientProviderOptions`, `FlagshipEvaluationResponse`, `CachedFlag`, `FlagshipBinding`, `FlagshipBindingEvaluationDetails`, `FlagshipBindingProviderOptions`, `FlagshipServerProviderOptions`, `FlagshipCacheOptions`

**`@cloudflare/flagship/server`** (core value exports + server-relevant types, plus):

- `FlagshipServerProvider` — dual-mode provider (HTTP or binding)
- `LoggingHook` — evaluation logging hook
- `TelemetryHook` — evaluation telemetry hook
- Type: `TelemetryEvent`

**`@cloudflare/flagship/web`** (all core exports plus):

- `FlagshipClientProvider` — sync cache-based provider

## Architecture

```
@cloudflare/flagship/server
  FlagshipServerProvider             — OpenFeature Provider interface (server)
    ├─ TTL + LRU cache (opt-in)      — keyed by flag key, type, and context
    ├─ Binding mode (Workers)        — delegates to env.FLAGS via RPC
    │   EvaluationContext → recursive JSON values over RPC
    └─ HTTP mode (Node.js, etc.)     — delegates to FlagshipClient
        FlagshipClient               — HTTP client with retry + timeout
          ContextTransformer         — primitive context → GET; structured context → JSON POST
    LoggingHook / TelemetryHook      — OpenFeature hooks

@cloudflare/flagship/web
  FlagshipClientProvider             — OpenFeature Provider interface (client)
    FlagshipClient                   — HTTP client (same as server), bulk OFREP evaluation
      ContextTransformer             — primitive context → GET; structured context → JSON POST
    In-memory cache                  — synchronous resolution layer
```
