---
'@cloudflare/flagship': minor
'@cloudflare/flagship-python': minor
'@cloudflare/flagship-go': minor
---

Add OpenFeature configuration-change notifications. The browser provider consumes advertised OFREP SSE streams by default with polling fallback, while local TypeScript, Go, and Python providers emit lifecycle events after refreshed state is usable.
