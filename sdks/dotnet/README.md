# Cloudflare Flagship .NET SDK

HTTP evaluation for Cloudflare Flagship, written in **C# 10** and targeting **.NET 8 and .NET 10**.
The optional provider implements the [OpenFeature .NET SDK](https://github.com/open-feature/dotnet-sdk) server interface.

| Package                           | Purpose                                           | Dependencies                |
| --------------------------------- | ------------------------------------------------- | --------------------------- |
| `Cloudflare.Flagship`             | Async HTTP client, configuration and typed errors | .NET runtime only           |
| `Cloudflare.Flagship.OpenFeature` | OpenFeature server provider                       | Core client and OpenFeature |

This SDK supports HTTP mode only. Native Workers bindings are exclusive to TypeScript.

## Build and install

The release workflow publishes these packages to nuget.org when the maintainer configures `NUGET_API_KEY`.
Until the first publication, build local packages from this directory:

```sh
dotnet pack Flagship.sln -c Release -o artifacts
# From your application directory, using the absolute path to artifacts:
dotnet nuget add source /path/to/sdks/dotnet/artifacts --name flagship-local
dotnet add package Cloudflare.Flagship.OpenFeature
```

Restore OpenFeature's transitive dependencies from nuget.org (the default NuGet source).
Use `Cloudflare.Flagship` alone if you do not need OpenFeature.

## OpenFeature quick start

```csharp
using Cloudflare.Flagship;
using Cloudflare.Flagship.OpenFeature;
using OpenFeature;
using OpenFeature.Model;

await Api.Instance.SetProviderAsync(new FlagshipServerProvider(new FlagshipOptions
{
    AppId = "your-app-id",
    AccountId = "your-account-id",
    AuthToken = Environment.GetEnvironmentVariable("CLOUDFLARE_API_TOKEN")
}));

var context = EvaluationContext.Builder()
    .SetTargetingKey("user-123")
    .Set("plan", "premium")
    .Build();

var client = Api.Instance.GetClient();
var enabled = await client.GetBooleanValueAsync("dark-mode", false, context);
var details = await client.GetStringDetailsAsync("homepage-hero", "control", context);
Console.WriteLine($"{enabled}: {details.Value} ({details.Reason}, {details.Variant})");

// At application shutdown, after evaluations have finished:
await Api.Instance.ShutdownAsync();
```

Register the provider once during application startup. The SDK's normal OpenFeature hooks, logging,
metrics, evaluation contexts, and domain registration work with this provider. All five typed methods
are supported: boolean, string, integer (`Int32`), double and object (`Value`, including JSON arrays).
Fractional and out-of-range integers produce `TYPE_MISMATCH` instead of being rounded or truncated.
Disabled flags return the caller's default, with reason `DISABLED` and no variant.

A complete runnable example is in [`examples/ConsoleExample`](examples/ConsoleExample):

```sh
# Set FLAGSHIP_APP_ID, CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_API_TOKEN first.
dotnet run --project examples/ConsoleExample --framework net10.0
```

## Standalone HTTP client

```csharp
using var client = new FlagshipClient(new FlagshipOptions
{
    AppId = "your-app-id",
    AccountId = "your-account-id",
    AuthToken = Environment.GetEnvironmentVariable("CLOUDFLARE_API_TOKEN")
});

var result = await client.EvaluateAsync("dark-mode", new Dictionary<string, object?>
{
    ["targetingKey"] = "user-123",
    ["plan"] = "premium"
}, cancellationToken);
bool enabled = result.Value.GetBoolean();
```

The raw client returns the API's JSON value, reason and variant. Its `JsonElement` remains valid after
the HTTP response is disposed. Unlike the provider, the raw client does not apply caller defaults.

## Configuration

Both constructors accept `FlagshipOptions` and an optional `HttpClient`.

| Option               | Default / behavior                                                             |
| -------------------- | ------------------------------------------------------------------------------ |
| `AppId`, `AccountId` | Required together unless `Endpoint` is supplied                                |
| `Endpoint`           | Full absolute HTTP(S) evaluation URL; mutually exclusive with `AppId`          |
| `BaseUrl`            | `https://api.cloudflare.com`; used only with `AppId`                           |
| `AuthToken`          | Optional bearer token                                                          |
| `Headers`            | Static headers; override the bearer token                                      |
| `HeadersFactory`     | Async callback per attempt; overrides static headers, supports rotating tokens |
| `Timeout`            | 5 seconds per attempt, including headers factory and response body             |
| `Retries`            | 1 additional attempt; permitted range 0–10; 0 disables retries                 |
| `RetryDelay`         | 1 second; permitted range 0–30 seconds                                         |

Configuration is captured at construction. Static headers are copied. Use `HeadersFactory` to rotate credentials.
The default endpoint is `/client/v4/accounts/{accountId}/flagship/apps/{appId}/evaluate`; IDs are URL-escaped.
As in the other SDKs, context and `flagKey` replace any query already on the endpoint. The explicit flag key takes precedence.
Context accepts strings, finite numbers, booleans and dates (UTC ISO 8601); null attributes are skipped.
Nested objects and lists produce `INVALID_CONTEXT` before an HTTP request is made.

Use a long-lived client/provider. An injected `HttpClient` remains caller-owned and its default headers
and timeout are never modified. Its own timeout can further limit a request. Disposing the client or
shutting down the provider disposes only internally created transports. Finish outstanding evaluations
before shutdown. Evaluation does not cache responses: each call reads current values from Flagship.

## Errors and cancellation

The raw client throws `FlagshipException` with `Code`, optional `StatusCode` and the original exception
where available. HTTP response bodies, tokens and context values are not copied into error messages.

| Core code                                                     | OpenFeature error |
| ------------------------------------------------------------- | ----------------- |
| `FlagNotFound` (404)                                          | `FLAG_NOT_FOUND`  |
| `InvalidContext`                                              | `INVALID_CONTEXT` |
| `ParseError`                                                  | `PARSE_ERROR`     |
| `BadRequest` (400), `NetworkError`, `TimeoutError`, `General` | `GENERAL`         |

OpenFeature returns the caller's default and error details for these failures. Type mismatches return
`TYPE_MISMATCH`. Resolution after provider shutdown reports `PROVIDER_NOT_READY`.
HTTP 400, 404, malformed responses and invalid contexts are never retried. Network failures, timeouts
and other HTTP failures are retried up to `Retries`, following the existing SDK policy.
Caller cancellation interrupts requests and retry delays, propagates as `OperationCanceledException`
from the raw client/provider resolution methods, and is never retried. The OpenFeature client controls
how cancellation is exposed through its own public evaluation methods.

## Development and release

Install the .NET 10 SDK and .NET 8 runtime, then run from this directory:

```sh
dotnet restore Flagship.sln
dotnet format Flagship.sln --verify-no-changes
dotnet test Flagship.sln -c Release
dotnet pack Flagship.sln -c Release -o artifacts
```

`LangVersion` is fixed at `10.0` for every project, including tests and the compiled example.
Tests use xUnit v3 with the VSTest adapter, and receive the runner cancellation token.
CI runs tests on both target frameworks and builds both NuGet packages. HTTP tests use an injected
message handler; no account credentials are required and they do not contact the live Flagship API.

The private `package.json` participates in the repository's existing Changesets release flow.
`Directory.Build.props` reads its version directly, so both NuGet packages follow the canonical SDK
version without an additional release manifest. NuGet publication is integrated into the release workflow
and is skipped successfully while `NUGET_API_KEY` is absent. See [Publishing to nuget.org](PUBLISHING.md)
for maintainer setup, versioning, retries and troubleshooting.

## License

[Apache-2.0](../../LICENSE)
