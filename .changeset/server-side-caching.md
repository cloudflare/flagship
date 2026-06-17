---
"@cloudflare/flagship": minor
"@cloudflare/flagship-python": minor
---

Add opt-in server-side response caching to the server providers. Set `cacheTtl` (TypeScript) or `cache_ttl` (Python) to enable a TTL + LRU cache keyed by flag key, type, and evaluation context. Cache hits resolve with reason `CACHED`; disabled flags and errors are never cached. Caching is off by default.
