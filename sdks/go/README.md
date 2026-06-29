# cloudflare/flagship Go SDK

[OpenFeature](https://openfeature.dev)-compliant provider SDK for Cloudflare Flagship in Go server applications.

> The Go SDK supports HTTP mode only. The Cloudflare Workers binding mode is exclusive to the TypeScript SDK.

## Installation

```sh
go get github.com/cloudflare/flagship/sdks/go
```

## Quick Start

```go
package main

import (
	"context"
	"log"
	"time"

	flagship "github.com/cloudflare/flagship/sdks/go"
	"github.com/open-feature/go-sdk/openfeature"
)

func main() {
	ctx := context.Background()

	provider, err := flagship.NewProvider(flagship.Options{
		AppID:     "your-app-id",
		AccountID: "your-account-id",
		AuthToken: "your-token",
		CacheTTL:  30 * time.Second, // cache evaluations per context for 30s (off by default)
	})
	if err != nil {
		log.Fatal(err)
	}

	if err := openfeature.SetProviderAndWait(provider); err != nil {
		log.Fatal(err)
	}
	defer openfeature.Shutdown()

	client := openfeature.NewDefaultClient()
	evalCtx := openfeature.NewEvaluationContext("user-123", map[string]any{
		"plan": "premium",
	})

	enabled, err := client.BooleanValue(ctx, "dark-mode", false, evalCtx)
	if err != nil {
		log.Fatal(err)
	}

	log.Println("dark-mode:", enabled)
}
```

## Configuration

`flagship.NewProvider` and `flagship.NewClient` accept either `AppID` plus `AccountID` or a full `Endpoint` URL.

```go
provider, err := flagship.NewProvider(flagship.Options{
	AppID:     "your-app-id",
	AccountID: "your-account-id",

	// Endpoint: "http://localhost:8787/client/v4/accounts/acct/flagship/apps/app/evaluate",
	// BaseURL:  "http://localhost:8787",

	AuthToken: "your-token",
	Headers: http.Header{
		"X-Custom": []string{"value"},
	},
	HeadersFactory: func(ctx context.Context) (http.Header, error) {
		return http.Header{"Authorization": []string{"Bearer rotated-token"}}, nil
	},

	Timeout:      5 * time.Second,
	Retries:      1,
	RetryDelay:   time.Second,
	CacheTTL:     30 * time.Second,
	CacheMaxSize: 1000,

	Logging: true,
})
```

| Option           | Description                                                                    |
| ---------------- | ------------------------------------------------------------------------------ |
| `AppID`          | Flagship app ID. Mutually exclusive with `Endpoint`.                           |
| `AccountID`      | Required with `AppID`.                                                         |
| `BaseURL`        | Base URL override used with `AppID`; defaults to `https://api.cloudflare.com`. |
| `Endpoint`       | Full absolute evaluation endpoint URL.                                         |
| `AuthToken`      | Adds `Authorization: Bearer <token>` to each request.                          |
| `Headers`        | Static headers. Explicit `Authorization` overrides `AuthToken`.                |
| `HeadersFactory` | Dynamic per-request headers. Values override `Headers` and `AuthToken`.        |
| `HTTPClient`     | Custom HTTP client.                                                            |
| `Timeout`        | Per-attempt timeout; defaults to 5 seconds.                                    |
| `Retries`        | Retry attempts on transient errors; defaults to 1 and is capped at 10.         |
| `DisableRetries` | Disables retries when set to true.                                             |
| `RetryDelay`     | Delay between retries; defaults to 1 second and is capped at 30 seconds.       |
| `CacheTTL`       | Cache TTL; enables response caching when greater than 0.                       |
| `CacheMaxSize`   | Maximum cached entries; defaults to 1000 when `CacheTTL` is set.               |
| `Logging`        | Enables provider debug/error logs; off by default.                             |
| `Logger`         | Optional `slog`-compatible logger.                                             |
| `Hooks`          | Provider-level OpenFeature hooks.                                              |

## Response Caching

The provider can cache evaluations to avoid a network round-trip for repeated flag/context pairs. Caching is **off by default** and enabled by setting `CacheTTL`:

```go
provider, err := flagship.NewProvider(flagship.Options{
	AppID:        "your-app-id",
	AccountID:    "your-account-id",
	AuthToken:    "your-token",
	CacheTTL:     30 * time.Second, // values may be up to this stale
	CacheMaxSize: 1000,             // LRU-evicted beyond this many entries
})
```

Each entry is keyed by flag key, flag type, and the full evaluation context, so distinct contexts never share a value. Cache hits resolve with `reason == openfeature.CachedReason`. Disabled flags, errors, and type mismatches are never cached. Because freshness is TTL-based, a flag change in Flagship takes effect after the entry expires.

The cache is per-provider instance, guarded by a mutex for concurrent use, and cleared on `Shutdown`.

## Evaluation Context

Context attributes are sent as URL query parameters. Supported values are `string`, numeric types, `bool`, and `time.Time`. `nil` values are skipped. Maps, slices, structs, and other complex values return `INVALID_CONTEXT` through OpenFeature and do not trigger an HTTP request.

## Flag Types

All OpenFeature server-side flag types are supported:

```go
enabled, _ := client.BooleanValue(ctx, "new-checkout", false, evalCtx)
variant, _ := client.StringValue(ctx, "homepage-hero", "control", evalCtx)
rate, _ := client.FloatValue(ctx, "sample-rate", 0.1, evalCtx)
limit, _ := client.IntValue(ctx, "upload-limit", 10, evalCtx)
config, _ := client.ObjectValue(ctx, "ui-config", map[string]any{"theme": "light"}, evalCtx)
```

Use `*ValueDetails` methods when you need reason, variant, metadata, or error codes.

## Development

```sh
go test ./...
```

## License

[Apache-2.0](../../LICENSE)
