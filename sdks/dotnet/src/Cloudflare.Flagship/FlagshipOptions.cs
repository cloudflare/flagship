namespace Cloudflare.Flagship;

/// <summary>HTTP evaluation settings. Configure before constructing the client.</summary>
public sealed record FlagshipOptions
{
    public string? AppId { get; init; }
    public string? AccountId { get; init; }
    public Uri? Endpoint { get; init; }
    public Uri BaseUrl { get; init; } = new("https://api.cloudflare.com");
    public string? AuthToken { get; init; }
    public IReadOnlyDictionary<string, string>? Headers { get; init; }
    public Func<CancellationToken, Task<IReadOnlyDictionary<string, string>>>? HeadersFactory { get; init; }
    public TimeSpan Timeout { get; init; } = TimeSpan.FromSeconds(5);
    public int Retries { get; init; } = 1;
    public TimeSpan RetryDelay { get; init; } = TimeSpan.FromSeconds(1);
}
