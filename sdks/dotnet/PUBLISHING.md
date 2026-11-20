# Publishing to nuget.org

The release workflow is ready to publish `Cloudflare.Flagship` and
`Cloudflare.Flagship.OpenFeature` to nuget.org. Both packages contain .NET 8 and
.NET 10 assemblies compiled with C# 10. No workflow edits are needed to enable
publication: configure the `NUGET_API_KEY` GitHub Actions secret as described below.

## Maintainer setup

1. Sign in to the nuget.org account authorized to publish packages for Cloudflare.
   Ensure that account can publish both package IDs: `Cloudflare.Flagship` and
   `Cloudflare.Flagship.OpenFeature`. For the first publication, the IDs must be
   available or their reserved prefix must belong to the account/organization.
2. Open [nuget.org API keys](https://www.nuget.org/account/apikeys) and create a key
   with **Push new packages and package versions** permission. Select the appropriate
   package owner and scope the key to `Cloudflare.Flagship*` so it covers both new
   packages. Choose an expiration date and arrange rotation before it expires.
   See [NuGet scoped API keys](https://learn.microsoft.com/en-us/nuget/nuget-org/scoped-api-keys).
3. In `cloudflare/flagship`, open **Settings → Secrets and variables → Actions →
   New repository secret**. Set the name to **`NUGET_API_KEY`** and paste the key as
   its value. An organization Actions secret with access granted to this repository
   also works. Use an Actions secret, not a variable or an environment-only secret;
   the workflow does not require a GitHub environment.
4. Merge the next Changesets release PR, or re-run **all jobs** of the most recent
   eligible **Release** workflow run if it previously skipped NuGet publication.
   Adding a secret alone does not start a workflow run.

The existing `GITHUB_TOKEN` creates the .NET publication tag; no additional GitHub
PAT is required. The reusable workflow requests `contents: write` for that job.
Repository/organization rules must permit the workflow to create `sdks/dotnet/v*`
tags, just as the existing release workflow creates Python and Go tags.

## Release behavior

1. A changeset targeting `@cloudflare/flagship-dotnet` participates in the existing
   shared versioning process. `Directory.Build.props` reads the synchronized
   `sdks/dotnet/package.json` version for both NuGet packages.
2. When the Changesets release PR is merged, `.github/release-sdks.ts` detects .NET
   source/build changes since the last successful `sdks/dotnet/v*` tag. Ordinary
   feature PR merges do not publish an unbumped version.
3. `release.yml` calls the reusable `publish-nuget.yml` workflow for an eligible
   .NET release. It explicitly passes the optional `NUGET_API_KEY` secret.
4. A read-only credential check runs first. If the secret is absent or empty, it
   succeeds, adds an explanation to the run summary, and leaves the **Publish** job
   **skipped**. It does not build, upload packages, or create a publication tag.
5. With a configured key, the workflow packs the two library projects in Release
   configuration, pushes the core package followed by the OpenFeature package to
   `https://api.nuget.org/v3/index.json`, then creates `sdks/dotnet/v<version>` only
   after both pushes succeed. PR CI remains responsible for tests and checks.

Before the first successful publication, the .NET SDK remains eligible across
later canonical releases, even if several releases skipped publication because
credentials were not configured. After a successful publication, documentation,
examples, test changes and mechanical version bumps alone do not trigger another
NuGet publication. A .NET-only release still creates the canonical release tag
without publishing an unchanged TypeScript package to npm.

The release workflow retains its existing `cloudflare` owner restriction, so
ordinary pushes to a personal fork do not publish packages.

## Retries and troubleshooting

- **Missing secret:** expected, non-failing skip. Configure `NUGET_API_KEY`, then
  re-run all jobs of the most recent eligible release run or wait for the next
  Changesets release. Do not create a .NET publication tag manually to mark a skip.
- **Expired key, incorrect scope, permission error, or network failure:** publication
  fails visibly. Only missing credentials are treated as a skip. Replace the secret
  or resolve the failure and re-run the release.
- **One package uploaded before a failure:** re-run the same release. Both pushes
  use `--skip-duplicate`, so already published versions are skipped and the missing
  package can complete. The tag is written only after both commands succeed.
- **Tag push failed after uploads:** resolve the GitHub permission/ruleset issue and
  re-run the same release; duplicate NuGet versions are skipped before retrying the tag.
- **Tag already exists on another commit:** the workflow fails before uploading.
  Investigate the release history rather than moving an existing publication tag.
- **Package content needs a correction:** add a new changeset and release a new
  version. nuget.org does not let this workflow replace a previously published
  version. `--skip-duplicate` is retry handling, not a content update mechanism.

Avoid re-running an older release after a newer version has been published; use
the latest eligible release run. Verify both package pages and the publication tag
after enabling the secret. nuget.org indexing can take time after a successful push.

## Local packaging without publication

From `sdks/dotnet`:

```sh
dotnet pack src/Cloudflare.Flagship/Cloudflare.Flagship.csproj -c Release -o artifacts
dotnet pack src/Cloudflare.Flagship.OpenFeature/Cloudflare.Flagship.OpenFeature.csproj -c Release -o artifacts
```

These commands require no publishing credentials and do not upload anything.
The implementation is validated with local packages and simulated release/command
execution; a live nuget.org upload requires the maintainer's secret.
