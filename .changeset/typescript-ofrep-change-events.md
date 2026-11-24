---
'@cloudflare/flagship': minor
---

Consume OFREP configuration-change events. `FlagshipClientProvider` connects to every event stream the server advertises, refreshes its bulk cache before emitting `PROVIDER_CONFIGURATION_CHANGED`, forwards an event's `etag` and `lastModified` as `flagConfigEtag` and `flagConfigLastModified`, revalidates other refreshes with the bulk response `ETag`, refreshes when the page becomes visible without a stream, closes idle streams and refetches unconditionally on resume, honors `Retry-After`, and moves to `FATAL` on a `401` or `403`. Timer polling stays off unless `pollInterval` is set, per OFREP ADR-0010. The provider also accepts `ofrepBaseUrl` and `headers` for any OFREP server and defaults to a 10-second timeout. `FlagshipServerProvider` gains opt-in `configurationEvents` for remote and local evaluation, and binding providers can invalidate cached results when the binding exposes `getConfigurationVersion()`.
