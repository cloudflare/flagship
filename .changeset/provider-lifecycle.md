---
'@cloudflare/flagship': minor
---

Report provider state accurately. Local-evaluation server providers emit `PROVIDER_STALE` when a definitions refresh fails and `PROVIDER_READY` when it recovers, and fail initialization with `PROVIDER_FATAL` on a 401 or 403. The client provider now rejects `initialize()` and `onContextChange()` when every flag fetch fails, instead of reporting `READY`, and `onClose()` now discards a load that is still in flight.
