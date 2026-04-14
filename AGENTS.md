# AGENTS.md

## Project Overview

`@cloudflare/flagship` — a TypeScript SDK for Cloudflare's Flagship feature flag platform. Provides OpenFeature-compatible providers for both server and browser environments.

This is a **pnpm monorepo** with a single published package today. More packages may be added under `packages/`.

## Repository Structure

```
packages/
  flagship/          # @cloudflare/flagship — OpenFeature provider SDK
    src/
      index.ts       # Core exports (FlagshipClient, types, errors)
      server.ts      # Re-exports core + FlagshipServerProvider + hooks
      web.ts         # Re-exports core + FlagshipClientProvider
      client.ts      # HTTP client with retry, timeout, AbortController
      context.ts     # OpenFeature EvaluationContext → query param transformer
      server-provider.ts  # Async per-request provider (server)
      client-provider.ts  # Sync cache-based provider (browser)
      hooks/         # LoggingHook, TelemetryHook
      types.ts       # Shared types and error codes
    test/            # Vitest unit and integration tests

.changeset/          # Changeset config and pending changesets
.github/             # CI workflows (release, pull-request, bonk), issue templates
```

## Setup

```bash
pnpm install        # install all workspace dependencies
```

Requires Node 22+ and pnpm 10+.

## Commands

Run from the repo root:

| Command              | What it does                                       |
| -------------------- | -------------------------------------------------- |
| `pnpm run build`     | Build all packages                                 |
| `pnpm run test`      | Run all tests                                      |
| `pnpm run check`     | Full CI check: sherif + oxfmt + oxlint + typecheck |
| `pnpm run lint`      | Run oxlint                                         |
| `pnpm run format`    | Format all files with oxfmt                        |
| `pnpm run typecheck` | TypeScript type checking across packages           |

Package-level (run from `packages/flagship/`):

| Command          | What it does                  |
| ---------------- | ----------------------------- |
| `pnpm run build` | Build with tsdown (ESM + CJS) |
| `pnpm run test`  | Run vitest                    |
| `pnpm run dev`   | Watch mode                    |

## SDK Architecture

The SDK has **three sub-path exports** to isolate dependencies:

- `@cloudflare/flagship` — core client, types, errors. Zero OpenFeature dependency.
- `@cloudflare/flagship/server` — `FlagshipServerProvider` + hooks. Requires `@openfeature/server-sdk`.
- `@cloudflare/flagship/web` — `FlagshipClientProvider`. Requires `@openfeature/web-sdk`.

Each sub-path is a separate bundle (built with tsdown) so importing one never pulls in the other's OpenFeature dependency.

### Server providers

`FlagshipServerProvider` supports two modes of operation:

- **HTTP mode** — evaluates flags via HTTP requests to the Flagship API. Requires `appId`/`endpoint`, `accountId`, and optionally `authToken`. Used for Node.js, generic server environments, or Workers without a binding.
- **Binding mode** — evaluates flags via a Cloudflare Workers wrangler binding (`env.FLAGS`). No HTTP overhead, no auth tokens. This is the recommended approach for Cloudflare Workers.

The constructor accepts a discriminated union: provide **either** HTTP config (`appId`, `accountId`, etc.) **or** a `binding` field — never both. Providing both throws immediately.

## Code Standards

### TypeScript

- Strict mode, target ES2021, module ES2022, moduleResolution: Bundler
- `noUnusedLocals`, `noUnusedParameters`, `noImplicitReturns` enabled

### Linting — Oxlint

Config in `.oxlintrc.json`. Plugins: `typescript`, `import`, `unicorn`. Key rules:

- `eqeqeq: "error"` — always use `===`
- `no-explicit-any: "warn"` (off in tests)
- `no-unused-vars: "error"` with `_` prefix ignore pattern

### Formatting — Oxfmt

Config in `.oxfmtrc.json`: tabs, single quotes, semicolons, 140 print width.

## Testing

Tests use **vitest** in Node environment. Test files live in `packages/flagship/test/` mirroring the source structure.

```bash
pnpm run test                    # all tests
pnpm --filter @cloudflare/flagship run test   # SDK tests only
```

## Contributing

### Changesets

Changes to published packages need a changeset:

```bash
pnpm changeset      # interactive prompt — pick packages, semver bump, description
```

### Pull Request Process

CI runs on every PR: `pnpm install → build → check → test`. All checks must pass.

## Boundaries

**Always:**

- Run `pnpm run check` before considering work done
- Keep OpenFeature peer dependencies optional
- Use `import type` for type-only imports

**Ask first:**

- Adding new dependencies to published packages
- Changing the SDK's public API surface

**Never:**

- Hardcode secrets or API keys
- Use `any` without justification
- Modify `node_modules/` or `dist/` directories
- Force push to main
