---
'@cloudflare/flagship-rust': minor
---

Add the Rust SDK (`cloudflare-flagship` on crates.io), an async OpenFeature provider and direct client for Flagship. It supports HTTP evaluation only; Cloudflare Workers should keep using the TypeScript SDK and its native binding.

- **Remote evaluation:** typed boolean, string, integer, float and structured values, targeting context, evaluation details, retries, request timeouts and an optional context-keyed response cache.
- **Local evaluation:** downloads flag definitions at startup and refreshes them in the background. A failed refresh keeps the last good snapshot and reports the provider as `STALE`. Requires `app_id`, `account_id` and a token with app **read** permission, and is incompatible with the response cache.
- **OpenFeature 0.3:** `FlagshipServerProvider` for the server API, plus `FlagshipClient` for direct use. Typed resolutions fail with a type mismatch when the served value doesn't match the requested Rust type.
- **Flag metadata:** returned as OpenFeature `flag_metadata` in remote and local modes, including for disabled flags and cache hits. Only string, boolean and numeric entries are kept.
- **Hooks:** `LoggingHook` and `TelemetryHook`.
- **TLS:** `rustls` (default), `native-tls`, or `rustls-no-provider` for applications that install their own crypto provider.

Requires Rust 1.88 or later.
