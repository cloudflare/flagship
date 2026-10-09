# @cloudflare/flagship

## 0.7.0

### Minor Changes

- [#50](https://github.com/cloudflare/flagship/pull/50) [`f4ef40e`](https://github.com/cloudflare/flagship/commit/f4ef40e52d52ad1739f8ad6070bb2bbcc6081a9e) Thanks [@akshitsinha](https://github.com/akshitsinha)! - `FlagshipClientProvider` now evaluates every flag in a single OFREP bulk request on `initialize()` and on every context change, so `prefetchFlags` is no longer needed and is ignored. Cached results keep their evaluation reason instead of `CACHED`, and per-flag failures surface their OpenFeature error code. The new `pollInterval` option re-evaluates flags in the background and emits `PROVIDER_CONFIGURATION_CHANGED` when results change.

- [#49](https://github.com/cloudflare/flagship/pull/49) [`0d46808`](https://github.com/cloudflare/flagship/commit/0d46808667b5977135875e8e0113d48654319a46) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Report provider state accurately. Local-evaluation server providers emit `PROVIDER_STALE` when a definitions refresh fails and `PROVIDER_READY` when it recovers, and fail initialization with `PROVIDER_FATAL` on a 401 or 403. The client provider now rejects `initialize()` and `onContextChange()` when every flag fetch fails, instead of reporting `READY`, and `onClose()` now discards a load that is still in flight.

### Patch Changes

- [#48](https://github.com/cloudflare/flagship/pull/48) [`c6c6864`](https://github.com/cloudflare/flagship/commit/c6c6864979962f3977cf18f9716aef4036f8d49a) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Map the OpenFeature `errorCode` from Flagship HTTP error responses, so `INVALID_CONTEXT`, `PARSE_ERROR` and `TYPE_MISMATCH` are no longer reported as `GENERAL`.

## 0.6.0

### Minor Changes

- [#37](https://github.com/cloudflare/flagship/pull/37) [`cd59b99`](https://github.com/cloudflare/flagship/commit/cd59b99bdc9f873481498b3d52917bfd1ec72fa5) Thanks [@karishnu](https://github.com/karishnu)! - Support nested objects, arrays, null, and recursively serialized dates in binding and HTTP evaluation context.
  
  Context values other than strings, numbers, booleans, `null`, dates, arrays and plain objects now resolve with `INVALID_CONTEXT` instead of being silently dropped.

- [#40](https://github.com/cloudflare/flagship/pull/40) [`fcc1094`](https://github.com/cloudflare/flagship/commit/fcc10940441a8420aaf63828e59f2aa426480d26) Thanks [@karishnu](https://github.com/karishnu)! - Support nested objects, arrays, null, and recursively serialized times in HTTP evaluation context.

- [#45](https://github.com/cloudflare/flagship/pull/45) [`7d3af0b`](https://github.com/cloudflare/flagship/commit/7d3af0bebfddbaffa1dc4433c67f4b20a0a9ab94) Thanks [@vaibhavshn](https://github.com/vaibhavshn)! - Add local in-process evaluation for `ServerProvider`.
  
  When `LocalEvaluation: true`, the provider downloads flag definitions at initialize, evaluates flags without a network call per evaluation, and refreshes definitions in the background. Requires `AccountID` and a token with app **read** permission. Incompatible with `CacheTTL`.

- [#44](https://github.com/cloudflare/flagship/pull/44) [`7d2d2dc`](https://github.com/cloudflare/flagship/commit/7d2d2dc8ed6901e7c68642e745ba560cce968368) Thanks [@vaibhavshn](https://github.com/vaibhavshn)! - Add local in-process evaluation for `FlagshipServerProvider`.
  
  When `local_evaluation=True`, the provider downloads flag definitions at initialize, evaluates flags without a network call per evaluation, and refreshes definitions in the background on a daemon thread. Requires `account_id` and a token with app **read** permission. Incompatible with `cache_ttl`.

- [#43](https://github.com/cloudflare/flagship/pull/43) [`8a873d9`](https://github.com/cloudflare/flagship/commit/8a873d938f1486d8a201b23bccc6ff834e20d308) Thanks [@vaibhavshn](https://github.com/vaibhavshn)! - Add local in-process evaluation for `FlagshipServerProvider`.
  
  When `localEvaluation: true`, the provider downloads flag definitions at initialize, evaluates flags without a network call per evaluation, and refreshes definitions lazily (stale-while-revalidate). Requires `accountId` and a token with app **read** permission. Incompatible with binding mode and `cacheTtl`.
  
  Also maps reason `STATIC` (flag has no rules) on evaluation responses.

- [#39](https://github.com/cloudflare/flagship/pull/39) [`a942982`](https://github.com/cloudflare/flagship/commit/a942982d2d761c0ff1c3b85b4230f06d1ae29352) Thanks [@karishnu](https://github.com/karishnu)! - Support nested objects, arrays, null, and recursively serialized datetimes in HTTP evaluation context.

### Patch Changes

- [#42](https://github.com/cloudflare/flagship/pull/42) [`84fd1c4`](https://github.com/cloudflare/flagship/commit/84fd1c45920efdbbf3d631a2e253211734b19dce) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Add the `STATIC` evaluation reason to the SDK types and docs. Flagship now reports `STATIC` instead of `DEFAULT` for an enabled flag with no targeting rules. The SDKs already passed unknown reasons through unchanged, so runtime behaviour is the same.
  
  - TypeScript: `FlagshipEvaluationResponse['reason']` includes `'STATIC'`.
  - Python: `EvaluationReason` includes `"STATIC"`; it resolves to `Reason.STATIC`.
  - Go: new `ReasonStatic` constant, mapped to `openfeature.StaticReason`.
  
  Code that checks `reason === 'DEFAULT'` to detect rule-less flags should also handle `STATIC`.

- [`86a4dda`](https://github.com/cloudflare/flagship/commit/86a4dda72ff80e248ef1292328c01981fca568e2) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Upgrade SDK dependencies and tooling across TypeScript, Python, and Go. No public API or runtime behaviour changes.
  
  TypeScript:
  
  - `lru-cache` 11.5.2 → 11.5.3 (runtime dependency).
  - `typescript` 5.9.3 → 7.0.2, `tsdown` 0.22.14 → 0.23.0, `vitest` and `@vitest/coverage-v8` 4.1.10 → 5.0.2, `@cloudflare/workers-types` 4 → 5, `tsx` 4.23.8 → 4.23.15, `@types/node` 24.13.3 → 24.19.0 (Node stays on 24).
  - Repo tooling: `oxlint` 1.77 → 1.85, `oxfmt` 0.62 → 0.70, `lint-staged` 17.3 → 17.6, `pkg-pr-new` 0.0.87 → 0.0.88, `@decimalturn/toml-patch` 2.1 → 3.3, `@changesets/cli` 2.31 → 3.0, `@changesets/read` 0.6 → 1.0, `@changesets/types` 6.1 → 7.0, `@changesets/changelog-github` 0.7 → 1.0.
  - pnpm 11 → 12, with the lockfile regenerated.
  - Declaration output changes shape only: classes are now exported inline. The exported names and types are unchanged for both ESM and CJS consumers.
  
  Python:
  
  - The lockfile moves to `cachetools` 7.2.0, `pytest-mock` 3.16.0, `ruff` 0.16.9, and `ty` 0.0.84. The published dependency ranges are unchanged.
  - Build backend `uv_build` moves to `>=0.12.19,<0.13.0`.
  - Ruff no longer formats Markdown code blocks, so README examples keep their hand-aligned layout.
  
  Go:
  
  - `github.com/open-feature/go-sdk` v1.17.2 → v1.18.0. The minimum Go version stays at 1.25.
  - `go-logr/logr` 1.4.3 → 1.4.4. It is no longer a direct requirement.
  
  CI:
  
  - `actions/checkout` v7, `actions/setup-node` v7, `actions/setup-go` v7, `actions/cache` v6.
  - `astral-sh/setup-uv` v10.2.0, pinned by SHA.
  - `pnpm/action-setup` v6.1.0, the first release with pnpm 12 support.
  - `changesets/action` v1.9.0 → v2.1.2, required by Changesets CLI 3. The release workflow uses the renamed inputs and outputs and keeps pushing through the Git CLI. The release script now calls `changeset git-tag`, the new name of `changeset tag`.

- [#38](https://github.com/cloudflare/flagship/pull/38) [`19e8220`](https://github.com/cloudflare/flagship/commit/19e8220577dd9c961a94fca179e19aa76820fbe5) Thanks [@karishnu](https://github.com/karishnu)! - Preserve provider readiness, fatal, and missing targeting-key error codes returned by the Workers binding.

## 0.5.0

### Minor Changes

- [#30](https://github.com/cloudflare/flagship/pull/30) [`6f75421`](https://github.com/cloudflare/flagship/commit/6f75421e75b612fde1de7c94b1ddc984d60b8344) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Add an injectable `fetch` transport and caller `AbortSignal` propagation to `FlagshipClient`

  - `FlagshipProviderOptions.fetch?: typeof globalThis.fetch` sets the transport for a client. It defaults to `globalThis.fetch`, resolved at call time, and the SDK never assigns to the global — so routing evaluations through a Workers service binding or stubbing the transport in tests no longer requires mutating `globalThis.fetch` and exposing unrelated traffic in the same isolate.
  - `evaluate(flagKey, context, { fetch?, signal? })` adds per-call overrides. `signal` is merged with the request timeout and with `fetchOptions.signal` (previously silently discarded), so a caller abort now aborts the in-flight HTTP request instead of only abandoning the promise. An already-aborted signal rejects without issuing a request.
  - New `FlagshipErrorCode.ABORTED` distinguishes caller cancellation from `TIMEOUT_ERROR`. Caller aborts interrupt in-flight requests and retry delays and are never retried; timeout aborts are still retried as before.
  - New `FlagshipError.retryable` reports whether a failure was transient. `408`, `425`, `429`, `5xx`, connection failures, timeouts, and malformed bodies are retryable; other non-2xx responses (`400`, `401`, `403`, `404`, `422`, …) and caller aborts are terminal. This lets consumers implement fail-closed-without-caching instead of guessing from `NETWORK_ERROR` alone.
  - `FlagshipServerProvider` and `FlagshipClientProvider` accept and forward `fetch` in HTTP mode; combining it with `binding` throws like the other HTTP-only options.

  Behaviour changes for existing callers, who are otherwise unaffected:

  - Previously every non-2xx except `400` and `404` was retried. Definitively terminal statuses such as `401`, `403`, and `422` are now propagated immediately.
  - The request timeout now also covers reading the response body, so a stalled body read no longer holds the request open past `timeout`.

## 0.4.2

### Patch Changes

- [`0900845`](https://github.com/cloudflare/flagship/commit/090084580c921d3be010ae0013d25f77894367d9) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Publish `sdks/go/vX.Y.Z` Git tag on release so the Go module proxy resolves a proper semver version instead of a pseudo-version.

## 0.4.1

### Patch Changes

- [#20](https://github.com/cloudflare/flagship/pull/20) [`52765bb`](https://github.com/cloudflare/flagship/commit/52765bbec08a49d362c6079edc5102360cb83395) Thanks [@thebongy](https://github.com/thebongy)! - Add the Go OpenFeature provider SDK for Flagship HTTP evaluation.

- [#27](https://github.com/cloudflare/flagship/pull/27) [`674d7c9`](https://github.com/cloudflare/flagship/commit/674d7c995cff822fbdbc4383747b8c966f05db8a) Thanks [@thebongy](https://github.com/thebongy)! - Add opt-in TTL and LRU response caching to the Go provider.

## 0.4.0

### Minor Changes

- [#22](https://github.com/cloudflare/flagship/pull/22) [`b53a7ce`](https://github.com/cloudflare/flagship/commit/b53a7cef43ad360e45f90e833e35c6b2ddfdc779) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Add opt-in server-side response caching to the server providers. Set `cacheTtl` (TypeScript) or `cache_ttl` (Python) to enable a TTL + LRU cache keyed by flag key, type, and evaluation context. Cache hits resolve with reason `CACHED`; disabled flags and errors are never cached. Caching is off by default.

### Patch Changes

- [#24](https://github.com/cloudflare/flagship/pull/24) [`122535b`](https://github.com/cloudflare/flagship/commit/122535b477db2034144c9c511fea2e0b41dffcab) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Upgrade TypeScript SDK dependencies and switch the Python SDK type checker from mypy to ty.

- [#21](https://github.com/cloudflare/flagship/pull/21) [`176c228`](https://github.com/cloudflare/flagship/commit/176c228123161d981e06af89fe7ea47d17367123) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Remove provider initialization health-check evaluations.

## 0.3.1

### Patch Changes

- [#18](https://github.com/cloudflare/flagship/pull/18) [`52c04ed`](https://github.com/cloudflare/flagship/commit/52c04eda5dde01aa905bd260b96945fcf45f1e61) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Fix release pipeline: Python checks on push to main, canonical single-package release notes, no private SDK tags, and reliable PyPI publish trigger.

## 0.3.0

### Minor Changes

- [#17](https://github.com/cloudflare/flagship/pull/17) [`ec78037`](https://github.com/cloudflare/flagship/commit/ec780373866160cf93d56ae99d61a7e93e7da6a6) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Add Python SDK (`cloudflare-flagship` on PyPI) — OpenFeature provider for Cloudflare Flagship.

### Patch Changes

- [#13](https://github.com/cloudflare/flagship/pull/13) [`139ec19`](https://github.com/cloudflare/flagship/commit/139ec19d2d3d91e0ad08695faeeb106cacce0d7a) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Restructure repository as a multi-language SDK monorepo under `packages/<language>/`. No API changes.

## 0.2.1

### Patch Changes

- [#10](https://github.com/cloudflare/flagship/pull/10) [`5a3a2ca`](https://github.com/cloudflare/flagship/commit/5a3a2ca2e327ef58507d29595af958846bf8471f) Thanks [@akshitsinha](https://github.com/akshitsinha)! - return SDK default value when flag is disabled

## 0.2.0

### Minor Changes

- [#4](https://github.com/cloudflare/flagship/pull/4) [`4460e58`](https://github.com/cloudflare/flagship/commit/4460e58addd822f9a93bfb90755c6eca5502a63f) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Add support to use env.FLAGS bindings in FlagshipServerProvider

### Patch Changes

- [#8](https://github.com/cloudflare/flagship/pull/8) [`3f71661`](https://github.com/cloudflare/flagship/commit/3f716613caf0999ba67ee19c7e35bf03573cfb5f) Thanks [@akshitsinha](https://github.com/akshitsinha)! - Add relative endpoint resolving support for FlagshipClientProvider
