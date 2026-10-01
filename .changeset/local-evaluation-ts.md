---
'@cloudflare/flagship': minor
---

Add local in-process evaluation for `FlagshipServerProvider`.

When `localEvaluation: true`, the provider downloads flag definitions at initialize, evaluates flags without a network call per evaluation, and refreshes definitions lazily (stale-while-revalidate). Requires `accountId` and a token with app **read** permission. Incompatible with binding mode and `cacheTtl`.

Also maps reason `STATIC` (flag has no rules) on evaluation responses.
