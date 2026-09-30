---
"@cloudflare/flagship": patch
"@cloudflare/flagship-python": patch
"@cloudflare/flagship-go": patch
---

Upgrade SDK dependencies and tooling across TypeScript, Python, and Go. No public API or runtime behaviour changes.

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
