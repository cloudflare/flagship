---
'@cloudflare/flagship-python': minor
---

Add local in-process evaluation for `FlagshipServerProvider`.

When `local_evaluation=True`, the provider downloads flag definitions at initialize, evaluates flags without a network call per evaluation, and refreshes definitions in the background on a daemon thread. Requires `account_id` and a token with app **read** permission. Incompatible with `cache_ttl`.
