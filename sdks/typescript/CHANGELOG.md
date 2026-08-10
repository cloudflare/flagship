# @cloudflare/flagship

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
