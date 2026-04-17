# Cloudflare Flagship

[![npm version](https://img.shields.io/npm/v/@cloudflare/flagship.svg)](https://www.npmjs.com/package/@cloudflare/flagship)
[![npm downloads](https://img.shields.io/npm/dm/@cloudflare/flagship.svg)](https://www.npmjs.com/package/@cloudflare/flagship)
[![license](https://img.shields.io/npm/l/@cloudflare/flagship.svg)](https://github.com/cloudflare/flagship/blob/main/LICENSE)

Flagship is a globally distributed, low-latency feature flag platform built entirely on Cloudflare. This repository contains the TypeScript SDK — an [OpenFeature](https://openfeature.dev)-compliant provider for evaluating feature flags from server-side (Node.js, Cloudflare Workers) and client-side (browser) environments.

```sh
npm install @cloudflare/flagship @openfeature/server-sdk
```

## Quick Example — Cloudflare Workers (binding, recommended)

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';
import type { FlagshipBinding } from '@cloudflare/flagship/server';

export default {
  async fetch(request: Request, env: { FLAGS: FlagshipBinding }) {
    await OpenFeature.setProviderAndWait(new FlagshipServerProvider({ binding: env.FLAGS }));
    const client = OpenFeature.getClient();
    const enabled = await client.getBooleanValue('dark-mode', false, { targetingKey: 'user-123' });
    return Response.json({ enabled });
  },
};
```

## Quick Example — HTTP

```typescript
import { OpenFeature } from '@openfeature/server-sdk';
import { FlagshipServerProvider } from '@cloudflare/flagship/server';

await OpenFeature.setProviderAndWait(
  new FlagshipServerProvider({ appId: 'your-app-id', accountId: 'your-account-id', authToken: 'your-token' }),
);

const client = OpenFeature.getClient();
const enabled = await client.getBooleanValue('dark-mode', false, {
  userId: 'user-123',
  plan: 'premium',
});
```

## Features

| Feature                   | Description                                                                        |
| ------------------------- | ---------------------------------------------------------------------------------- |
| **OpenFeature compliant** | Implements the CNCF OpenFeature specification                                      |
| **Workers binding**       | Native wrangler binding support — zero HTTP overhead, no auth tokens               |
| **Server providers**      | `FlagshipServerProvider` works via both wrangler binding or HTTP.                  |
| **Server + client**       | Async per-request evaluation (server) and sync cache-based evaluation (browser)    |
| **All flag types**        | Boolean, string, number, and object (JSON)                                         |
| **Authentication**        | `authToken` option adds `Authorization: Bearer` to every request (HTTP only)       |
| **Logging**               | `logging` option surfaces fetch errors and cache misses (off by default)           |
| **Retries + timeouts**    | Configurable retry logic with `AbortController`-based timeouts (HTTP only)         |
| **Hooks**                 | Built-in `LoggingHook` and `TelemetryHook` for observability                       |
| **Tree-shakeable**        | Server and client bundles are fully isolated — importing one never loads the other |
| **TypeScript**            | Strict types throughout                                                            |

## Packages

| Export                                             | Description                      | Peer dependency           |
| -------------------------------------------------- | -------------------------------- | ------------------------- |
| [`@cloudflare/flagship`](packages/flagship)        | Core client, types, errors       | None                      |
| [`@cloudflare/flagship/server`](packages/flagship) | `FlagshipServerProvider` + hooks | `@openfeature/server-sdk` |
| [`@cloudflare/flagship/web`](packages/flagship)    | `FlagshipClientProvider`         | `@openfeature/web-sdk`    |

Each sub-path is a separate bundle so importing one never pulls in the other's OpenFeature dependency.

## Documentation

- [API reference](docs/API.md)
- [OpenFeature specification](https://openfeature.dev/specification/)
- [Examples](packages/flagship/examples/)

## Repository Structure

| Directory                                 | Description                                       |
| ----------------------------------------- | ------------------------------------------------- |
| [`packages/flagship/`](packages/flagship) | `@cloudflare/flagship` — OpenFeature provider SDK |
| [`docs/`](docs)                           | API reference and documentation                   |
| [`.changeset/`](.changeset)               | Changeset config and pending changesets           |
| [`.github/`](.github)                     | CI workflows and issue templates                  |

## Development

Node 22+ and pnpm 10+ required.

```sh
pnpm install         # install all workspace dependencies
pnpm run build       # build all packages
pnpm run check       # full CI check (sherif, format, lint, typecheck)
pnpm run test        # run all tests
```

Changes to published packages need a changeset:

```sh
pnpm changeset
```

See [`AGENTS.md`](AGENTS.md) for deeper contributor guidance.

## Contributing

We welcome contributions. Please open an issue first to discuss what you'd like to change.

- **Bug reports & feature requests** — [open an issue](https://github.com/cloudflare/flagship/issues)

## License

[Apache-2.0](LICENSE)
