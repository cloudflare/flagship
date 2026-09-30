---
'@cloudflare/flagship': patch
'@cloudflare/flagship-python': patch
'@cloudflare/flagship-go': patch
---

Add the `STATIC` evaluation reason to the SDK types and docs. Flagship now reports `STATIC` instead of `DEFAULT` for an enabled flag with no targeting rules. The SDKs already passed unknown reasons through unchanged, so runtime behaviour is the same.

- TypeScript: `FlagshipEvaluationResponse['reason']` includes `'STATIC'`.
- Python: `EvaluationReason` includes `"STATIC"`; it resolves to `Reason.STATIC`.
- Go: new `ReasonStatic` constant, mapped to `openfeature.StaticReason`.

Code that checks `reason === 'DEFAULT'` to detect rule-less flags should also handle `STATIC`.
