---
'@cloudflare/flagship-go': minor
---

Add local in-process evaluation for `ServerProvider`.

When `LocalEvaluation: true`, the provider downloads flag definitions at initialize, evaluates flags without a network call per evaluation, and refreshes definitions in the background. Requires `AccountID` and a token with app **read** permission. Incompatible with `CacheTTL`.
