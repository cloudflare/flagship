---
'@cloudflare/flagship-rust': patch
---

Keep combining marks such as accents visible in sanitized error messages, whichever Rust version built the crate. Control, format, private-use, and non-ASCII separator characters are still escaped as `\u{...}`.
