using System.Globalization;
using System.Net;
using System.Text.Json;

namespace Cloudflare.Flagship;

/// <summary>A reusable, concurrent HTTP evaluation client. Injected HttpClients remain caller-owned.</summary>
public sealed class FlagshipClient : IDisposable
{
    private readonly FlagshipOptions _options;
    private readonly HttpClient _httpClient;
    private readonly bool _ownsHttpClient;
    private bool _disposed;

    public Uri Endpoint { get; }

    public FlagshipClient(FlagshipOptions options, HttpClient? httpClient = null)
    {
        ArgumentNullException.ThrowIfNull(options);
        if (options.Retries < 0 || options.Retries > 10)
            throw new ArgumentOutOfRangeException(nameof(options), "Retries must be between 0 and 10.");
        if (options.Timeout <= TimeSpan.Zero || options.Timeout.TotalMilliseconds > uint.MaxValue - 1)
            throw new ArgumentOutOfRangeException(nameof(options), "Timeout must be positive and supported by CancellationTokenSource.");
        if (options.RetryDelay < TimeSpan.Zero || options.RetryDelay > TimeSpan.FromSeconds(30))
            throw new ArgumentOutOfRangeException(nameof(options), "RetryDelay must be between zero and 30 seconds.");
        if ((options.Endpoint is null) == string.IsNullOrWhiteSpace(options.AppId))
            throw new ArgumentException("Provide either AppId or Endpoint, not both.", nameof(options));
        if (options.Endpoint is null && string.IsNullOrWhiteSpace(options.AccountId))
            throw new ArgumentException("AccountId is required with AppId.", nameof(options));

        if (options.Endpoint is not null)
            Endpoint = ValidateUri(options.Endpoint);
        else
        {
            var baseUrl = ValidateUri(options.BaseUrl);
            if (!string.IsNullOrEmpty(baseUrl.Query) || !string.IsNullOrEmpty(baseUrl.Fragment))
                throw new ArgumentException("BaseUrl cannot contain a query or fragment.", nameof(options));
            Endpoint = new Uri($"{baseUrl.AbsoluteUri.TrimEnd('/')}/client/v4/accounts/{Uri.EscapeDataString(options.AccountId!)}/flagship/apps/{Uri.EscapeDataString(options.AppId!)}/evaluate");
        }
        _options = options with { Headers = options.Headers is null ? null : new Dictionary<string, string>(options.Headers, StringComparer.OrdinalIgnoreCase) };
        _ownsHttpClient = httpClient is null;
        _httpClient = httpClient ?? new HttpClient { Timeout = System.Threading.Timeout.InfiniteTimeSpan };
    }

    /// <summary>Evaluate using scalar context values; null attributes are omitted.</summary>
    public async Task<EvaluationResponse> EvaluateAsync(string flagKey,
        IReadOnlyDictionary<string, object?>? context = null, CancellationToken cancellationToken = default)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        ArgumentException.ThrowIfNullOrWhiteSpace(flagKey);
        cancellationToken.ThrowIfCancellationRequested();
        var query = new List<string>();
        if (context is not null)
            foreach (var (key, value) in context)
            {
                if (value is null || key == "flagKey") continue;
                query.Add($"{Uri.EscapeDataString(key)}={Uri.EscapeDataString(SerializeContextValue(value))}");
            }
        query.Add($"flagKey={Uri.EscapeDataString(flagKey)}");
        // As in the other SDKs, evaluation parameters replace any endpoint query.
        var uri = new UriBuilder(Endpoint) { Query = string.Join("&", query), Fragment = "" }.Uri;
        for (var attempt = 0; ; attempt++)
        {
            cancellationToken.ThrowIfCancellationRequested();
            try
            {
                return await FetchAsync(uri, flagKey, cancellationToken).ConfigureAwait(false);
            }
            catch (FlagshipException ex) when (attempt < _options.Retries &&
                ex.Code is FlagshipErrorCode.NetworkError or FlagshipErrorCode.TimeoutError or FlagshipErrorCode.General)
            {
                await Task.Delay(_options.RetryDelay, cancellationToken).ConfigureAwait(false);
            }
        }
    }

    private async Task<EvaluationResponse> FetchAsync(Uri uri, string flagKey, CancellationToken cancellationToken)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(_options.Timeout);
        try
        {
            using var request = new HttpRequestMessage(HttpMethod.Get, uri);
            var headers = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
            if (!string.IsNullOrEmpty(_options.AuthToken)) headers["Authorization"] = $"Bearer {_options.AuthToken}";
            if (_options.Headers is not null)
                foreach (var header in _options.Headers) headers[header.Key] = header.Value;
            if (_options.HeadersFactory is not null)
            {
                IReadOnlyDictionary<string, string> dynamicHeaders;
                try
                {
                    dynamicHeaders = await _options.HeadersFactory(timeout.Token).WaitAsync(timeout.Token).ConfigureAwait(false);
                }
                catch (Exception ex) when (ex is not OperationCanceledException)
                {
                    throw new FlagshipException(FlagshipErrorCode.General, "Headers factory failed.", innerException: ex);
                }
                foreach (var header in dynamicHeaders) headers[header.Key] = header.Value;
            }
            foreach (var header in headers) request.Headers.Add(header.Key, header.Value);
            using var response = await _httpClient.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token).ConfigureAwait(false);
            if (!response.IsSuccessStatusCode)
            {
                var code = response.StatusCode switch
                {
                    HttpStatusCode.NotFound => FlagshipErrorCode.FlagNotFound,
                    HttpStatusCode.BadRequest => FlagshipErrorCode.BadRequest,
                    _ => FlagshipErrorCode.General
                };
                throw new FlagshipException(code, $"Flagship returned HTTP {(int)response.StatusCode}.", response.StatusCode);
            }
            await using var stream = await response.Content.ReadAsStreamAsync(timeout.Token).ConfigureAwait(false);
            using var document = await JsonDocument.ParseAsync(stream, cancellationToken: timeout.Token).ConfigureAwait(false);
            var root = document.RootElement;
            if (root.ValueKind != JsonValueKind.Object ||
                !root.TryGetProperty("flagKey", out var key) || key.ValueKind != JsonValueKind.String || key.GetString() != flagKey ||
                !root.TryGetProperty("value", out var value))
                throw new FlagshipException(FlagshipErrorCode.ParseError, "Invalid Flagship evaluation response.");
            var reason = ReadOptionalString(root, "reason") ?? "DEFAULT";
            var variant = ReadOptionalString(root, "variant");
            return new EvaluationResponse(flagKey, value.Clone(), reason, variant);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested) { throw; }
        catch (OperationCanceledException ex)
        {
            throw new FlagshipException(FlagshipErrorCode.TimeoutError, "Flagship evaluation timed out.", innerException: ex);
        }
        catch (HttpRequestException ex)
        {
            throw new FlagshipException(FlagshipErrorCode.NetworkError, "Flagship network request failed.", innerException: ex);
        }
        catch (IOException ex)
        {
            throw new FlagshipException(FlagshipErrorCode.NetworkError, "Flagship response could not be read.", innerException: ex);
        }
        catch (JsonException ex)
        {
            throw new FlagshipException(FlagshipErrorCode.ParseError, "Invalid JSON in Flagship response.", innerException: ex);
        }
    }

    private static string? ReadOptionalString(JsonElement root, string name)
    {
        if (!root.TryGetProperty(name, out var value)) return null;
        if (value.ValueKind != JsonValueKind.String)
            throw new FlagshipException(FlagshipErrorCode.ParseError, $"Invalid response {name}.");
        return value.GetString();
    }

    private static Uri ValidateUri(Uri uri)
    {
        if (!uri.IsAbsoluteUri || (uri.Scheme != Uri.UriSchemeHttp && uri.Scheme != Uri.UriSchemeHttps) || !string.IsNullOrEmpty(uri.UserInfo))
            throw new ArgumentException("Endpoint and BaseUrl must be absolute HTTP(S) URLs without user information.");
        return uri;
    }

    private static string SerializeContextValue(object value) => value switch
    {
        string text => text,
        bool boolean => boolean ? "true" : "false",
        DateTime date => date.ToUniversalTime().ToString("O", CultureInfo.InvariantCulture),
        DateTimeOffset date => date.ToUniversalTime().ToString("O", CultureInfo.InvariantCulture),
        byte or sbyte or short or ushort or int or uint or long or ulong or decimal => ((IFormattable)value).ToString(null, CultureInfo.InvariantCulture),
        float number when float.IsFinite(number) => number.ToString("R", CultureInfo.InvariantCulture),
        double number when double.IsFinite(number) => number.ToString("R", CultureInfo.InvariantCulture),
        _ => throw new FlagshipException(FlagshipErrorCode.InvalidContext, "Context attributes must be finite numbers, strings, booleans, dates or null.")
    };

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        if (_ownsHttpClient) _httpClient.Dispose();
    }
}
