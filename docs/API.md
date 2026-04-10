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

The `FlagshipServerProvider` evaluates flags by making an HTTP request on each call. It is designed for server environments where the evaluation context changes per request (per-user, per-request targeting).

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
console.log(details.reason); // 'TARGETING_MATCH' | 'SPLIT' | 'DEFAULT' | 'DISABLED' | 'ERROR'
console.log(details.variant); // variation key, e.g. 'on', 'off', 'v2'
console.log(details.errorCode); // set on error, e.g. 'FLAG_NOT_FOUND', 'TYPE_MISMATCH'
console.log(details.errorMessage); // human-readable description of the error
```

### Configuration

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

  timeout: 5000, // request timeout in ms (default: 5000)
  retries: 1, // retry attempts on transient errors (default: 1, max: 10)
  retryDelay: 1000, // delay between retries in ms (default: 1000, max: 30000)
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

The `FlagshipClientProvider` is designed for browsers and other static-context environments. The OpenFeature web SDK requires synchronous flag resolution, so this provider pre-fetches a configured set of flags whenever the evaluation context changes and serves them from an in-memory cache.

### Basic usage

```typescript
import { OpenFeature } from '@openfeature/web-sdk';
import { FlagshipClientProvider } from '@cloudflare/flagship/web';

await OpenFeature.setProviderAndWait(
  new FlagshipClientProvider({
    appId: 'your-app-id',
    accountId: 'your-account-id',
    prefetchFlags: ['dark-mode', 'welcome-message', 'max-uploads'],
    cacheTTL: 60_000, // expire cached entries after 1 minute (0 = never, default)
  }),
);

// Setting context triggers a pre-fetch of all configured flags.
// Flags are fetched for the new context before the promise resolves.
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

| Situation                                       | `reason`  | `errorCode`     | Value returned |
| ----------------------------------------------- | --------- | --------------- | -------------- |
| Flag was pre-fetched and cached                 | `CACHED`  | —               | Cached value   |
| Flag was not in `prefetchFlags`, or TTL expired | `DEFAULT` | —               | Default value  |
| Cached value's type doesn't match the call      | `ERROR`   | `TYPE_MISMATCH` | Default value  |

When the context changes, cache entries for all `prefetchFlags` are **invalidated before re-fetching**. A failed re-fetch returns the default value rather than serving values from the previous context.

### Configuration options

| Option          | Type          | Default                               | Description                                           |
| --------------- | ------------- | ------------------------------------- | ----------------------------------------------------- |
| `appId`         | `string`      | —                                     | Flagship app ID (mutually exclusive with `endpoint`)  |
| `accountId`     | `string`      | —                                     | Account ID (required with `appId`)                    |
| `baseUrl`       | `string`      | `https://api.flagship.cloudflare.dev` | Base URL override (only used with `appId`)            |
| `endpoint`      | `string`      | —                                     | Full evaluation URL (mutually exclusive with `appId`) |
| `prefetchFlags` | `string[]`    | `[]`                                  | Flag keys to pre-fetch on context changes             |
| `cacheTTL`      | `number`      | `0`                                   | Cache TTL in ms (0 = no expiry)                       |
| `timeout`       | `number`      | `5000`                                | Request timeout in ms                                 |
| `retries`       | `number`      | `1`                                   | Retry attempts (max 10)                               |
| `retryDelay`    | `number`      | `1000`                                | Delay between retries in ms (max 30 000)              |
| `fetchOptions`  | `RequestInit` | `{}`                                  | Custom fetch options (headers, credentials, etc.)     |

## Evaluation context

Context attributes are serialized as URL query parameters and sent with each evaluation request. The following value types are supported:

| Type                          | Serialization                                                                                                                                                  |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `string`, `number`, `boolean` | Passed directly as a string                                                                                                                                    |
| `Date`                        | Converted to ISO 8601                                                                                                                                          |
| Objects, arrays               | **Not supported** — the provider throws `INVALID_CONTEXT`. Keys with complex values are dropped with a console warning if using `ContextTransformer` directly. |

`targetingKey` is the standard OpenFeature field for identifying the evaluation subject (user ID, session ID, etc.) and is treated like any other attribute.

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

| Code              | Cause                                                            |
| ----------------- | ---------------------------------------------------------------- |
| `FLAG_NOT_FOUND`  | The flag key does not exist (HTTP 404)                           |
| `TYPE_MISMATCH`   | The flag's resolved value type does not match the requested type |
| `INVALID_CONTEXT` | The evaluation context contains objects or arrays                |
| `PARSE_ERROR`     | The API response was not a valid evaluation response             |
| `GENERAL`         | Network error, timeout, or any other transient failure           |

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

OpenFeature.addHandler(ProviderEvents.Error, ({ message }) => {
  console.error('Provider failed to initialize:', message);
});

await OpenFeature.setProviderAndWait(provider);
```

**Server provider:** During initialization, the provider probes the evaluation endpoint with a health-check request. A 404 response (flag not found) is treated as success — it means the endpoint is reachable. Any network or timeout error causes `ProviderEvents.Error` to be emitted, but `setProviderAndWait` still resolves (does not reject) — the provider transitions to `ERROR` status silently.

**Client provider:** During initialization, the provider pre-fetches all `prefetchFlags` using `Promise.allSettled`. Even if some or all fetches fail, the provider transitions to `READY` status. Failed flags will return default values when resolved.

To shut down the provider and release resources:

```typescript
await provider.onClose();
// provider.status === ProviderStatus.NOT_READY
// Client provider also clears the in-memory cache
```

## Exports

Each sub-path re-exports core utilities alongside its provider-specific classes.

**`@cloudflare/flagship`** (core — no OpenFeature dependency):

- `FlagshipClient` — HTTP client with retry, timeout, AbortController
- `ContextTransformer` — converts evaluation context to query parameters
- `FlagshipError` — error class with `code` and `cause` properties
- `FlagshipErrorCode` — enum: `NETWORK_ERROR`, `TIMEOUT_ERROR`, `PARSE_ERROR`, `INVALID_CONTEXT`
- `FLAGSHIP_DEFAULT_BASE_URL` — default base URL constant
- Types: `FlagshipProviderOptions`, `FlagshipClientProviderOptions`, `FlagshipEvaluationResponse`, `CachedFlag`

**`@cloudflare/flagship/server`** (core value exports + server-relevant types, plus):

- `FlagshipServerProvider` — async per-request provider
- `LoggingHook` — evaluation logging hook
- `TelemetryHook` — evaluation telemetry hook
- Type: `TelemetryEvent`

**`@cloudflare/flagship/web`** (all core exports plus):

- `FlagshipClientProvider` — sync cache-based provider

## Architecture

```
@cloudflare/flagship/server
  FlagshipServerProvider        — OpenFeature Provider interface (server)
    FlagshipClient              — HTTP client with retry + timeout
      ContextTransformer        — EvaluationContext → query parameters
    LoggingHook / TelemetryHook — OpenFeature hooks

@cloudflare/flagship/web
  FlagshipClientProvider        — OpenFeature Provider interface (client)
    FlagshipClient              — HTTP client (same as server)
      ContextTransformer        — EvaluationContext → query parameters
    In-memory cache             — synchronous resolution layer
```
