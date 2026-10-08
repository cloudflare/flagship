---
'@cloudflare/flagship': minor
---

`FlagshipClientProvider` now evaluates every flag in a single OFREP bulk request on `initialize()` and on every context change, so `prefetchFlags` is no longer needed and is ignored. Cached results keep their evaluation reason instead of `CACHED`, and per-flag failures surface their OpenFeature error code.
