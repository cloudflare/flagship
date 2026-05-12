# AGENTS.md

## Project Overview

`@cloudflare/flagship` — the TypeScript SDK for Cloudflare's Flagship feature flag platform. Provides OpenFeature-compatible providers for both server and browser environments.

This is a **pnpm monorepo** with SDKs organized by implementation language under `packages/<language>/`. The current published SDK is the TypeScript package in `packages/typescript`.

## Repository Structure

```
packages/
  typescript/        # @cloudflare/flagship — OpenFeature provider SDK
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
    tests/           # Vitest unit and integration tests

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

Package-level (run from `packages/typescript/`):

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

Tests use **vitest** in Node environment. Test files live in `packages/typescript/tests/` mirroring the source structure.

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

Pick only the SDK package(s) you actually changed. The release workflow expands every release-bound changeset so all SDK packages are bumped to the same version. The highest bump in the changeset (`patch` < `minor` < `major`) becomes the bump for the rest.

The release pipeline runs `.github/changeset-version.ts`, which:

1. Validates every pending changeset — fails fast on unknown packages, non-SDK-only changesets, or SDK entries with a `none` bump.
2. Expands the changeset to include every SDK package so they share the bump.
3. Runs `pnpm changeset version`, producing one PR titled `chore(release): version SDK packages` with all SDK `package.json` and `CHANGELOG.md` updates.
4. Syncs the new version into native manifests beside each SDK:
   - `pyproject.toml` / `Cargo.toml` — all `version = "..."` lines in the file are updated in-place. The manifest must contain at least one such line or the release fails.
   - `go.mod` — no file sync. Go modules are versioned exclusively via git tags; the script logs this and moves on.
5. Re-runs `pnpm install` to refresh the lockfile.

After merge the same workflow runs `pnpm changeset publish` and:

- Publishes public npm SDKs (currently `@cloudflare/flagship`).
- Skips npm publish for `private: true` SDKs but still creates a git tag (`privatePackages.tag: true`). Language-specific publish workflows (PyPI, crates.io, etc.) should subscribe to those tags. For Go, the git tag is the version — no additional file sync is needed.

Every releasable SDK must have a `package.json` so Changesets can discover and version it, even if the actual package is published to PyPI, crates.io, Go modules, or another registry. Non-npm SDK packages should use `private: true` plus `flagship.language` and keep their native manifest beside it:

| Language | Native manifest  | Version sync                                          |
| -------- | ---------------- | ----------------------------------------------------- |
| Python   | `pyproject.toml` | All `version = "..."` lines updated by release script |
| Rust     | `Cargo.toml`     | All `version = "..."` lines updated by release script |
| Go       | `go.mod`         | No file sync — version is the git tag only            |

PR CI runs `pnpm run changeset:validate` (the `Changesets` job) so malformed, non-SDK, or `none`-bumped SDK changesets fail before merge.

Changesets should remain the only release intent file. Do not add release-please, semantic-release, or language-specific release manifests unless the release workflow is explicitly changed to derive them from Changesets.

### Pull Request Process

CI runs on every PR: `pnpm install → build → check → test → changeset:validate`. All checks must pass.

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
