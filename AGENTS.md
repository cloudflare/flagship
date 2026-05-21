# AGENTS.md

## Project Overview

Cloudflare Flagship SDKs — OpenFeature-compatible SDKs for Cloudflare's Flagship feature flag platform.

This is a **pnpm monorepo** with SDKs organized by implementation language under `sdks/<language>/`. The TypeScript SDK is the recommended SDK, especially for Cloudflare Workers because it supports the native Flagship Workers binding. The Python and Go SDKs support HTTP evaluation only.

## Repository Structure

```
sdks/
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
    examples/        # Node.js, browser, and Cloudflare Workers examples

  python/            # cloudflare-flagship — OpenFeature provider SDK (HTTP only)
    src/flagship/    # Client, provider, context serialization, hooks
    tests/           # pytest unit and integration tests
    examples/        # Sync and async OpenFeature examples
    pyproject.toml   # Python package metadata and uv build config
    uv.lock          # Python dependency lockfile

  go/                # github.com/cloudflare/flagship/sdks/go — OpenFeature provider SDK (HTTP only)
    *.go             # Client, provider, context serialization, hooks
    *_test.go        # Go unit and integration tests
    examples/        # OpenFeature usage examples
    go.mod           # Go module metadata

.changeset/          # Changeset config and pending changesets
.github/             # CI workflows (pull-request, release, publish-pypi, bonk, semgrep), issue templates
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

Root pnpm commands cover the repo-level TypeScript/JavaScript workspace. Python checks are run separately from `sdks/python/`.

Package-level (run from `sdks/typescript/`):

| Command          | What it does                  |
| ---------------- | ----------------------------- |
| `pnpm run build` | Build with tsdown (ESM + CJS) |
| `pnpm run test`  | Run vitest                    |
| `pnpm run dev`   | Watch mode                    |

Python SDK (run from `sdks/python/`):

| Command                        | What it does               |
| ------------------------------ | -------------------------- |
| `uv sync --group dev`          | Install Python dev deps    |
| `uv run ruff format --check .` | Check Python formatting    |
| `uv run ruff check .`          | Run Python linting         |
| `uv run --group dev mypy`      | Run Python type checking   |
| `uv run --group dev pytest`    | Run Python tests           |
| `uv build`                     | Build Python wheel + sdist |

Go SDK (run from `sdks/go/`):

| Command         | What it does      |
| --------------- | ----------------- |
| `gofmt -w .`    | Format Go files   |
| `go test ./...` | Run Go tests      |
| `go vet ./...`  | Run Go vet checks |

## SDK Architecture

### TypeScript

The TypeScript SDK has **three sub-path exports** to isolate dependencies:

- `@cloudflare/flagship` — core client, types, errors. Zero OpenFeature dependency.
- `@cloudflare/flagship/server` — `FlagshipServerProvider` + hooks. Requires `@openfeature/server-sdk`.
- `@cloudflare/flagship/web` — `FlagshipClientProvider`. Requires `@openfeature/web-sdk`.

Each sub-path is a separate bundle (built with tsdown) so importing one never pulls in the other's OpenFeature dependency.

#### Server providers

`FlagshipServerProvider` supports two modes of operation:

- **HTTP mode** — evaluates flags via HTTP requests to the Flagship API. Requires `appId`/`endpoint`, `accountId`, and optionally `authToken`. Used for Node.js, generic server environments, or Workers without a binding.
- **Binding mode** — evaluates flags via a Cloudflare Workers wrangler binding (`env.FLAGS`). No HTTP overhead, no auth tokens. This is the recommended approach for Cloudflare Workers.

The constructor accepts a discriminated union: provide **either** HTTP config (`appId`, `accountId`, etc.) **or** a `binding` field — never both. Providing both throws immediately.

### Python

The Python SDK is published as `cloudflare-flagship` and supports HTTP evaluation only.

- `FlagshipClient` handles endpoint construction, auth headers, retries, timeouts, and response parsing.
- `FlagshipServerProvider` implements the OpenFeature Python provider interface.
- Sync and async evaluation APIs are supported.
- Native Cloudflare Workers binding mode is not available in Python.

### Go

The Go SDK is published through Go modules at `github.com/cloudflare/flagship/sdks/go` and supports HTTP evaluation only.

- `FlagshipClient` handles endpoint construction, auth headers, retries, timeouts, and response parsing.
- `ServerProvider` implements the OpenFeature Go provider interface, including context-aware initialization, provider events, and typed resolution details.
- Built-in `LoggingHook` and `TelemetryHook` mirror the other SDK hook conveniences.
- Native Cloudflare Workers binding mode is not available in Go.

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

Python files under `sdks/python/**` are excluded from oxfmt and formatted with Ruff.

Go files under `sdks/go/**` are formatted with `gofmt`.

### Python

- Package metadata and build backend live in `sdks/python/pyproject.toml`.
- Python source is typed (`py.typed`) and checked with mypy in strict mode.
- Python requires `>=3.10`.

## Testing

TypeScript tests use **vitest** in Node environment. Test files live in `sdks/typescript/tests/` mirroring the source structure.

```bash
pnpm run test                    # all tests
pnpm --filter @cloudflare/flagship run test   # SDK tests only
```

Python tests use **pytest**:

```bash
cd sdks/python
uv run --group dev pytest
```

Go tests use the standard Go toolchain:

```bash
cd sdks/go
go test ./...
```

## Contributing

### Changesets

Changes to published packages need a changeset:

```bash
pnpm changeset      # interactive prompt — pick packages, semver bump, description
```

Pick the SDK package you changed. During release, SDK changesets are rewritten to the canonical public package (`@cloudflare/flagship`) so the release PR has one changelog section and one tag. Private SDK versions are then synchronized to the same version.

The release pipeline runs `.github/changeset-version.ts`, which:

1. Validates every pending changeset — fails fast on unknown packages, non-SDK-only changesets, or SDK entries with a `none` bump.
2. Rewrites SDK changesets to `@cloudflare/flagship` using the highest SDK bump in each changeset.
3. Runs `pnpm changeset version`, producing one PR titled `chore(release): version SDK packages` with the TypeScript SDK changelog update.
4. Syncs the canonical version into private SDK `package.json` files and native manifests beside each SDK:
   - `pyproject.toml` — updates `[project].version` (PEP 621, used by uv/hatch/flit/pdm and Poetry 2.0+) and/or `[tool.poetry].version` (legacy Poetry 1.x), whichever fields are present. Fails if neither exists.
   - `Cargo.toml` — updates `[package].version` only. Dependency `version` fields are left untouched.
   - `go.mod` — no file sync. Go modules are versioned exclusively via git tags.
5. Deletes duplicate changelogs generated for private SDK packages.
6. Re-runs `pnpm install` to refresh the lockfile.

After merge the same workflow runs `pnpm changeset publish` and:

- Publishes public npm SDKs (currently `@cloudflare/flagship`) and creates the canonical `@cloudflare/flagship@*` git tag.
- Skips npm publish and tag creation for `private: true` SDKs (`privatePackages.tag: false`). The Python PyPI workflow subscribes to the canonical `@cloudflare/flagship@*` tag and publishes via PyPI trusted publishing (OIDC, no PyPI token). For Go, the canonical release tag is the version signal — no additional file sync is needed.

Every releasable SDK must have a `package.json` so Changesets can discover and version it, even if the actual package is published to PyPI, crates.io, Go modules, or another registry. Non-npm SDK packages should use `private: true` and keep their native manifest beside it:

| Language | Native manifest  | Version sync                                                              |
| -------- | ---------------- | ------------------------------------------------------------------------- |
| Python   | `pyproject.toml` | `[project].version` and/or `[tool.poetry].version`, whichever are present |
| Rust     | `Cargo.toml`     | `[package].version` only — dependency versions are not touched            |
| Go       | `go.mod`         | No file sync — version is the git tag only                                |

PR CI runs `pnpm run changeset:validate` (the `Changesets` job) so malformed, non-SDK, or `none`-bumped SDK changesets fail before merge.

Changesets should remain the only release intent file. Do not add release-please, semantic-release, or language-specific release manifests unless the release workflow is explicitly changed to derive them from Changesets.

### Pull Request Process

`pull-request.yml` runs on every PR with all checks in parallel:

- **Repo-wide:** changeset validation.
- **TypeScript:** format, lint, typecheck, test, build, publish preview (`pkg-pr-new`).
- **Python:** format, lint, typecheck, test, build.

A `CI Success` aggregator job depends on all of the above and is the single required status check for branch protection on `main`.

Publishing is split across two workflows; `pull-request.yml` is the source of truth for correctness and is never re-run during release:

- `release.yml` runs on pushes to `main`. Changesets opens or updates a release PR that bumps versions; merging that PR triggers the same workflow, which then publishes npm packages and reports `published: true`.
- `publish-pypi.yml` is a reusable workflow (`workflow_call`) invoked by `release.yml` only when `published == 'true'`. It builds the Python SDK and publishes to PyPI via OIDC trusted publishing — never runs on every push.

## Boundaries

**Always:**

- Run `pnpm run check` before considering work done
- For Python SDK changes, also run `uv run ruff format --check .`, `uv run ruff check .`, `uv run --group dev mypy`, and `uv run --group dev pytest` from `sdks/python/`
- For Go SDK changes, also run `gofmt`, `go vet ./...`, and `go test ./...` from `sdks/go/`
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
