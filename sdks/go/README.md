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

	flagship "github.com/cloudflare/flagship/sdks/go"
	"github.com/open-feature/go-sdk/openfeature"
)

func main() {
	ctx := context.Background()

	provider, err := flagship.NewProvider(flagship.Options{
		AppID:     "your-app-id",
		AccountID: "your-account-id",
		AuthToken: "your-token",
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

	Timeout:    5 * time.Second,
	Retries:    1,
	RetryDelay: time.Second,

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
| `Logging`        | Enables provider debug/error logs; off by default.                             |
| `Logger`         | Optional `slog`-compatible logger.                                             |
| `Hooks`          | Provider-level OpenFeature hooks.                                              |

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
