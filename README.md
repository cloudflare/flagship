# Cloudflare Flagship

[![npm version](https://img.shields.io/npm/v/@cloudflare/flagship.svg)](https://www.npmjs.com/package/@cloudflare/flagship)
[![PyPI version](https://img.shields.io/pypi/v/cloudflare-flagship.svg)](https://pypi.org/project/cloudflare-flagship/)
[![Go Reference](https://pkg.go.dev/badge/github.com/cloudflare/flagship/sdks/go.svg)](https://pkg.go.dev/github.com/cloudflare/flagship/sdks/go)
[![crates.io version](https://img.shields.io/crates/v/cloudflare-flagship.svg)](https://crates.io/crates/cloudflare-flagship)
[![license](https://img.shields.io/npm/l/@cloudflare/flagship.svg)](LICENSE)

Flagship is Cloudflare's feature flag platform. This repository contains the official Flagship SDKs for application developers who want to evaluate feature flags through [OpenFeature](https://openfeature.dev/).

The TypeScript SDK is recommended for most use cases. It supports HTTP evaluation, browser-side caching, and the native Flagship Workers binding — which skips HTTP entirely and requires no auth token configuration. If you are building on Cloudflare Workers, use the TypeScript SDK with the Workers binding. The Python and Go SDKs are available for server-side applications and support HTTP evaluation only. The Rust SDK supports remote HTTP evaluation, local evaluation, and an optional cache, but does not support the Workers binding.

## SDKs

| SDK        | Package                                                                      | Runtime                               | Evaluation modes                              | Docs                                 |
| ---------- | ---------------------------------------------------------------------------- | ------------------------------------- | --------------------------------------------- | ------------------------------------ |
| TypeScript | [`@cloudflare/flagship`](https://www.npmjs.com/package/@cloudflare/flagship) | Node.js, Cloudflare Workers, browsers | Workers binding, HTTP, browser prefetch cache | [`sdks/typescript`](sdks/typescript) |
| Python     | [`cloudflare-flagship`](https://pypi.org/project/cloudflare-flagship/)       | Python server applications            | HTTP                                          | [`sdks/python`](sdks/python)         |
| Go         | [`github.com/cloudflare/flagship/sdks/go`](sdks/go)                          | Go server applications                | HTTP                                          | [`sdks/go`](sdks/go)                 |
| Rust       | [`cloudflare-flagship`](https://crates.io/crates/cloudflare-flagship)        | Rust server applications              | HTTP, local evaluation, cache                 | [`sdks/rust`](sdks/rust)             |

## TypeScript

Install the SDK with the OpenFeature package for your runtime:

```sh
npm install @cloudflare/flagship @openfeature/server-sdk
```

Cloudflare Workers should use the Flagship binding when possible. It avoids HTTP overhead and does not require application-managed auth tokens.

```ts
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider, type FlagshipBinding } from '@cloudflare/flagship/server';

export default {
	async fetch(request: Request, env: { FLAGS: FlagshipBinding }) {
		await OpenFeature.setProviderAndWait(new FlagshipServerProvider({ binding: env.FLAGS }));

		const client = OpenFeature.getClient();
		const enabled = await client.getBooleanValue('dark-mode', false, {
			targetingKey: 'user-123',
		});

		return Response.json({ enabled });
	},
};
```

For Node.js or other server environments, use HTTP mode:

```ts
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(
	new FlagshipServerProvider({
		appId: 'your-app-id',
		accountId: 'your-account-id',
		authToken: 'your-token',
	}),
);

const client = OpenFeature.getClient();
const enabled = await client.getBooleanValue('dark-mode', false, {
	targetingKey: 'user-123',
	plan: 'premium',
});
```

## Python

Install with uv or pip:

```sh
uv add cloudflare-flagship
# or
pip install cloudflare-flagship
```

```py
from openfeature import api
from openfeature.evaluation_context import EvaluationContext
from flagship import FlagshipServerProvider

api.set_provider(
	FlagshipServerProvider(
		app_id='your-app-id',
		account_id='your-account-id',
		auth_token='your-token',
	)
)

client = api.get_client()
enabled = client.get_boolean_value(
	'dark-mode',
	False,
	EvaluationContext(targeting_key='user-123', attributes={'plan': 'premium'}),
)
```

The Python SDK supports HTTP evaluation only.

## Go

Install with `go get`:

```sh
go get github.com/cloudflare/flagship/sdks/go
```

```go
package main

import (
	"context"

	flagship "github.com/cloudflare/flagship/sdks/go"
	"github.com/open-feature/go-sdk/openfeature"
)

func main() {
	provider, err := flagship.NewProvider(flagship.Options{
		AppID:     "your-app-id",
		AccountID: "your-account-id",
		AuthToken: "your-token",
	})
	if err != nil {
		panic(err)
	}

	if err := openfeature.SetProviderAndWait(provider); err != nil {
		panic(err)
	}
	defer openfeature.Shutdown()

	client := openfeature.NewDefaultClient()
	enabled, err := client.BooleanValue(
		context.Background(),
		"dark-mode",
		false,
		openfeature.NewEvaluationContext("user-123", map[string]any{"plan": "premium"}),
	)
	if err != nil {
		panic(err)
	}
	_ = enabled
}
```

The Go SDK supports HTTP evaluation only.

## Rust

Install from crates.io:

```sh
cargo add cloudflare-flagship
```

See the [Rust SDK README](sdks/rust/README.md) for remote evaluation, local evaluation, caching, TLS options, and OpenFeature examples. The Rust SDK is for server applications and does not provide the Cloudflare Workers binding.

## Repository Layout

| Path                                     | Description                                                  |
| ---------------------------------------- | ------------------------------------------------------------ |
| [`sdks/typescript`](sdks/typescript)     | TypeScript SDK source, tests, examples, and package metadata |
| [`sdks/python`](sdks/python)             | Python SDK source, tests, examples, and package metadata     |
| [`sdks/go`](sdks/go)                     | Go SDK source, tests, examples, and module metadata          |
| [`sdks/rust`](sdks/rust)                 | Rust SDK source, tests, examples, and Cargo metadata         |
| [`.changeset`](.changeset)               | Release intent files and Changesets configuration            |
| [`.github/workflows`](.github/workflows) | Pull request checks and publish workflows                    |

## Development

This is a pnpm monorepo. Node.js 22+ and pnpm 12+ are required.

```sh
pnpm install
pnpm run check
```

## Links

- [TypeScript examples](sdks/typescript/examples)
- [Python examples](sdks/python/examples)
- [Go examples](sdks/go/examples)
- [Rust SDK documentation and examples](sdks/rust/README.md)
- [OpenFeature specification](https://openfeature.dev/specification/)
- [Issues](https://github.com/cloudflare/flagship/issues)

## License

[Apache-2.0](LICENSE)
