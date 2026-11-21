---
'@cloudflare/flagship-python': minor
---

Consume OFREP configuration-change events. `FlagshipServerProvider` gains opt-in `configuration_events` for local and HTTP evaluation, refreshing state before emitting `PROVIDER_CONFIGURATION_CHANGED`. A new `FlagshipClientProvider` follows the OFREP static-context provider guideline: it caches one bulk evaluation per context, connects to every advertised event stream (ADR-0008), forwards event `etag` and `lastModified`, revalidates with the response `ETag`, honors `Retry-After`, and keeps timer polling off unless `poll_interval` is set (ADR-0010). Use `identify()` for context changes and `pause()`/`resume()` for inactivity.
