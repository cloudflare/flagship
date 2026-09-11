using System.Globalization;
using System.Net;
using System.Text;
using Xunit;

namespace Cloudflare.Flagship.Tests;

internal sealed class TestHandler : HttpMessageHandler
{
    private readonly Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> _send;

    public int Calls { get; private set; }

    public bool Disposed { get; private set; }

    public TestHandler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> send) => _send = send;

    protected override Task<HttpResponseMessage> SendAsync(
        HttpRequestMessage request,
        CancellationToken cancellationToken)
    {
        Calls++;

        return _send(request, cancellationToken);
    }

    protected override void Dispose(bool disposing)
    {
        Disposed = true;

        base.Dispose(disposing);
    }

    public static HttpResponseMessage Json(string body, HttpStatusCode status = HttpStatusCode.OK) =>
        new(status)
        {
            Content = new StringContent(body, Encoding.UTF8, "application/json")
        };
}

public sealed class ClientTests
{
    internal static FlagshipOptions Options => new()
    {
        Endpoint = new Uri("https://example.com/evaluate"),
        Retries = 0,
        RetryDelay = TimeSpan.Zero
    };

    internal const string Success = "{\"flagKey\":\"flag\",\"value\":true,\"reason\":\"TARGETING_MATCH\",\"variant\":\"on\"}";

    [Fact]
    public async Task SerializesContextAndAppliesHeaderPrecedenceWithoutMutatingHttpClient()
    {
        using var handler = new TestHandler((request, _) =>
        {
            var query = request.RequestUri!.Query;

            Assert.Contains("flagKey=flag", query);
            Assert.Contains("targetingKey=user%20%26%201", query);
            Assert.Contains("amount=1.25", query);
            Assert.Contains("active=true", query);
            Assert.Contains("date=2026-01-02T03%3A04%3A05.0000000Z", query);
            Assert.DoesNotContain("missing", query);
            Assert.DoesNotContain("attacker", query);
            Assert.Equal("Bearer dynamic", request.Headers.Authorization!.ToString());
            Assert.Equal("yes", Assert.Single(request.Headers.GetValues("X-Custom")));

            return Task.FromResult(TestHandler.Json(Success));
        });

        using var http = new HttpClient(handler);

        var originalCulture = CultureInfo.CurrentCulture;
        CultureInfo.CurrentCulture = CultureInfo.GetCultureInfo("pt-BR");

        try
        {
            using var client = new FlagshipClient(Options with
            {
                AuthToken = "token",
                Headers = new Dictionary<string, string>
                {
                    ["Authorization"] = "Bearer static",
                    ["X-Custom"] = "yes"
                },
                HeadersFactory = _ => Task.FromResult<IReadOnlyDictionary<string, string>>(
                    new Dictionary<string, string>
                    {
                        ["authorization"] = "Bearer dynamic"
                    })
            }, http);

            var response = await client.EvaluateAsync("flag", new Dictionary<string, object?>
            {
                ["targetingKey"] = "user & 1",
                ["amount"] = 1.25,
                ["active"] = true,
                ["date"] = new DateTime(2026, 1, 2, 3, 4, 5, DateTimeKind.Utc),
                ["missing"] = null,
                ["flagKey"] = "attacker"
            }, TestContext.Current.CancellationToken);

            Assert.True(response.Value.GetBoolean());
            Assert.Equal("on", response.Variant);
            Assert.Empty(http.DefaultRequestHeaders);
        }
        finally
        {
            CultureInfo.CurrentCulture = originalCulture;
        }
    }

    [Fact]
    public void ConstructsEndpointAndValidatesOptions()
    {
        using var client = new FlagshipClient(new FlagshipOptions
        {
            AccountId = "account id",
            AppId = "app/id"
        });

        Assert.Equal(
            "https://api.cloudflare.com/client/v4/accounts/account%20id/flagship/apps/app%2Fid/evaluate",
            client.Endpoint.AbsoluteUri);

        Assert.Throws<ArgumentException>(() => new FlagshipClient(new FlagshipOptions()));
        Assert.Throws<ArgumentException>(() => new FlagshipClient(new FlagshipOptions { AppId = "app" }));
        Assert.Throws<ArgumentException>(() => new FlagshipClient(Options with { AppId = "app" }));
        Assert.Throws<ArgumentException>(
            () => new FlagshipClient(Options with { Endpoint = new Uri("ftp://example.com") }));
        Assert.Throws<ArgumentOutOfRangeException>(() => new FlagshipClient(Options with { Retries = 11 }));
        Assert.Throws<ArgumentOutOfRangeException>(() => new FlagshipClient(Options with { Timeout = TimeSpan.Zero }));
    }

    [Theory]
    [InlineData(400, FlagshipErrorCode.BadRequest, 1)]
    [InlineData(404, FlagshipErrorCode.FlagNotFound, 1)]
    [InlineData(429, FlagshipErrorCode.General, 3)]
    [InlineData(503, FlagshipErrorCode.General, 3)]
    public async Task MapsHttpErrorsAndBoundsRetries(int status, FlagshipErrorCode code, int calls)
    {
        using var handler = new TestHandler((_, _) =>
            Task.FromResult(TestHandler.Json("secret response", (HttpStatusCode)status)));
        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options with { Retries = 2 }, http);

        var error = await Assert.ThrowsAsync<FlagshipException>(
            () => client.EvaluateAsync(
                "flag",
                cancellationToken: TestContext.Current.CancellationToken));

        Assert.Equal(code, error.Code);
        Assert.Equal((HttpStatusCode)status, error.StatusCode);
        Assert.DoesNotContain("secret", error.Message);
        Assert.Equal(calls, handler.Calls);
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("[]")]
    [InlineData("{\"flagKey\":\"flag\"}")]
    [InlineData("{\"flagKey\":\"wrong\",\"value\":true}")]
    [InlineData("{\"flagKey\":\"flag\",\"value\":true,\"reason\":42}")]
    [InlineData("{\"flagKey\":\"flag\",\"value\":true,\"variant\":null}")]
    public async Task MalformedResponsesAreNotRetried(string json)
    {
        using var handler = new TestHandler((_, _) => Task.FromResult(TestHandler.Json(json)));
        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options with { Retries = 2 }, http);

        var error = await Assert.ThrowsAsync<FlagshipException>(
            () => client.EvaluateAsync(
                "flag",
                cancellationToken: TestContext.Current.CancellationToken));

        Assert.Equal(FlagshipErrorCode.ParseError, error.Code);
        Assert.Equal(1, handler.Calls);
    }

    [Fact]
    public async Task RejectsComplexAndNonFiniteContextBeforeSending()
    {
        using var handler = new TestHandler((_, _) => Task.FromResult(TestHandler.Json(Success)));
        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options, http);

        foreach (var value in new object[] { new[] { 1 }, new { plan = "pro" }, double.NaN, double.PositiveInfinity })
        {
            var error = await Assert.ThrowsAsync<FlagshipException>(
                () => client.EvaluateAsync(
                    "flag",
                    new Dictionary<string, object?> { ["bad"] = value },
                    TestContext.Current.CancellationToken));

            Assert.Equal(FlagshipErrorCode.InvalidContext, error.Code);
        }

        Assert.Equal(0, handler.Calls);
    }

    [Fact]
    public async Task RetriesNetworkErrorsAndRefreshesDynamicHeaders()
    {
        var attempts = 0;

        using var handler = new TestHandler((request, _) =>
        {
            Assert.Equal($"Bearer {attempts}", request.Headers.Authorization!.ToString());

            if (attempts == 1)
            {
                throw new HttpRequestException("offline");
            }

            return Task.FromResult(TestHandler.Json(Success));
        });

        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options with
        {
            Retries = 1,
            HeadersFactory = _ => Task.FromResult<IReadOnlyDictionary<string, string>>(
                new Dictionary<string, string>
                {
                    ["Authorization"] = $"Bearer {++attempts}"
                })
        }, http);

        var response = await client.EvaluateAsync(
            "flag",
            cancellationToken: TestContext.Current.CancellationToken);

        Assert.True(response.Value.GetBoolean());
        Assert.Equal(2, handler.Calls);
    }

    [Fact]
    public async Task TimeoutAppliesPerAttempt()
    {
        using var handler = new TestHandler(async (_, token) =>
        {
            await Task.Delay(Timeout.InfiniteTimeSpan, token);

            return TestHandler.Json(Success);
        });

        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options with
        {
            Timeout = TimeSpan.FromMilliseconds(30),
            Retries = 1
        }, http);

        var error = await Assert.ThrowsAsync<FlagshipException>(
            () => client.EvaluateAsync(
                "flag",
                cancellationToken: TestContext.Current.CancellationToken));

        Assert.Equal(FlagshipErrorCode.TimeoutError, error.Code);
        Assert.Equal(2, handler.Calls);
    }

    [Fact]
    public async Task CallerCancellationIsNeverRetried()
    {
        using var cancel = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);

        using var handler = new TestHandler(async (_, token) =>
        {
            cancel.Cancel();

            await Task.Delay(Timeout.InfiniteTimeSpan, token);

            return TestHandler.Json(Success);
        });

        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options with { Retries = 2 }, http);

        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => client.EvaluateAsync("flag", cancellationToken: cancel.Token));

        Assert.Equal(1, handler.Calls);
    }

    [Fact]
    public async Task CancellationInterruptsRetryDelay()
    {
        using var cancel = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);

        using var handler = new TestHandler((_, _) =>
        {
            cancel.Cancel();

            return Task.FromResult(TestHandler.Json("{}", HttpStatusCode.ServiceUnavailable));
        });

        using var http = new HttpClient(handler);
        using var client = new FlagshipClient(Options with
        {
            Retries = 2,
            RetryDelay = TimeSpan.FromSeconds(30)
        }, http);

        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => client.EvaluateAsync("flag", cancellationToken: cancel.Token));

        Assert.Equal(1, handler.Calls);
    }

    [Fact]
    public async Task DisposePreservesCallerOwnedTransport()
    {
        using var handler = new TestHandler((_, _) => Task.FromResult(TestHandler.Json(Success)));
        using var http = new HttpClient(handler);
        var client = new FlagshipClient(Options, http);

        client.Dispose();
        client.Dispose();

        Assert.False(handler.Disposed);

        await Assert.ThrowsAsync<ObjectDisposedException>(
            () => client.EvaluateAsync(
                "flag",
                cancellationToken: TestContext.Current.CancellationToken));

        using var response = await http.GetAsync("https://example.com", TestContext.Current.CancellationToken);

        Assert.True(response.IsSuccessStatusCode);
    }
}
