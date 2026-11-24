---
'@cloudflare/flagship': minor
---

Add per-request `browserOrigin` support to bulk evaluation and definitions discovery. Backends can request fresh, Origin-bound browser SSE tickets without exposing their credentials or changing their own server-provider subscriptions.
