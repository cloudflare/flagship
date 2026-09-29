---
'@cloudflare/flagship': minor
---

Support nested objects, arrays, null, and recursively serialized dates in binding and HTTP evaluation context.

Context values other than strings, numbers, booleans, `null`, dates, arrays and plain objects now resolve with `INVALID_CONTEXT` instead of being silently dropped.
