---
'@cloudflare/flagship-rust': minor
---

Add `FlagshipClientProvider`, an OFREP static-context provider that caches one bulk evaluation per context and detects changes through OFREP event streams (ADR-0008) or opt-in polling, with timer polling off by default (ADR-0010). Use `identify` for context changes and `subscribe` for change notifications, which `open-feature` 0.3 cannot deliver. `FlagshipClientOptionsBuilder::ofrep_base_url` targets any OFREP server.
