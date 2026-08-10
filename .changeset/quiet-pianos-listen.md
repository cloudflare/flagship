---
'@cloudflare/flagship': minor
---

Add an injectable `fetch` transport and caller `AbortSignal` propagation to `FlagshipClient`

- `FlagshipProviderOptions.fetch?: typeof globalThis.fetch` sets the transport for a client. It defaults to `globalThis.fetch`, resolved at call time, and the SDK never assigns to the global — so routing evaluations through a Workers service binding or stubbing the transport in tests no longer requires mutating `globalThis.fetch` and exposing unrelated traffic in the same isolate.
- `evaluate(flagKey, context, { fetch?, signal? })` adds per-call overrides. `signal` is merged with the request timeout and with `fetchOptions.signal` (previously silently discarded), so a caller abort now aborts the in-flight HTTP request instead of only abandoning the promise. An already-aborted signal rejects without issuing a request.
- New `FlagshipErrorCode.ABORTED` distinguishes caller cancellation from `TIMEOUT_ERROR`. Caller aborts interrupt in-flight requests and retry delays and are never retried; timeout aborts are still retried as before.
- New `FlagshipError.retryable` reports whether a failure was transient. `408`, `425`, `429`, `5xx`, connection failures, timeouts, and malformed bodies are retryable; other non-2xx responses (`400`, `401`, `403`, `404`, `422`, …) and caller aborts are terminal. This lets consumers implement fail-closed-without-caching instead of guessing from `NETWORK_ERROR` alone.
- `FlagshipServerProvider` and `FlagshipClientProvider` accept and forward `fetch` in HTTP mode; combining it with `binding` throws like the other HTTP-only options.

Behaviour changes for existing callers, who are otherwise unaffected:

- Previously every non-2xx except `400` and `404` was retried. Definitively terminal statuses such as `401`, `403`, and `422` are now propagated immediately.
- The request timeout now also covers reading the response body, so a stalled body read no longer holds the request open past `timeout`.
