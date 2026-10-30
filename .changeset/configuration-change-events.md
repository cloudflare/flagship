---
'@cloudflare/flagship': minor
'@cloudflare/flagship-python': minor
'@cloudflare/flagship-go': minor
---

Add OpenFeature configuration-change notifications. Event-capable TypeScript providers explicitly request and consume OFREP SSE streams with polling fallback, while local TypeScript, Go, and Python providers emit lifecycle events after refreshed state is usable.
