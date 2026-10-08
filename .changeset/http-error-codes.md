---
'@cloudflare/flagship': patch
---

Map the OpenFeature `errorCode` from Flagship HTTP error responses, so `INVALID_CONTEXT`, `PARSE_ERROR` and `TYPE_MISMATCH` are no longer reported as `GENERAL`.
