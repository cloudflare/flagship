---
'@cloudflare/flagship-go': minor
---

Consume OFREP configuration-change events. `ServerProvider` gains opt-in `ConfigurationEvents` for local and HTTP evaluation, refreshing state before emitting `PROVIDER_CONFIGURATION_CHANGED`. A new `ClientProvider` follows the OFREP static-context provider guideline: it caches one bulk evaluation per context, connects to every advertised event stream (ADR-0008), forwards event `etag` and `lastModified`, revalidates with the response `ETag`, honors `Retry-After`, and keeps timer polling off unless `PollInterval` is set (ADR-0010). Use `Identify` for context changes and `Pause`/`Resume` for inactivity.
