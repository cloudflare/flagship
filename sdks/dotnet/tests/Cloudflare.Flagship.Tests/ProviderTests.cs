// Isolated APIs keep integration tests independent; this experimental API is test-only.
#pragma warning disable OFISO001

using System.Net;
using Cloudflare.Flagship.OpenFeature;
using OpenFeature.Constant;
using OpenFeature.Isolated;
using OpenFeature.Model;
using Xunit;

namespace Cloudflare.Flagship.Tests;

public sealed class ProviderTests
{
    [Fact]
    public async Task OfficialOpenFeatureClientResolvesAllTypesAndContext()
    {
        using var handler = new TestHandler((request, _) =>
        {
            Assert.Contains("targetingKey=user-123", request.RequestUri!.Query);
            Assert.Contains("plan=pro", request.RequestUri.Query);

            var key = request.RequestUri.Query.Split("flagKey=")[1];
            var value = key switch
            {
                "boolean" => "true",
                "string" => "\"hello\"",
                "integer" => "42.0",
                "double" => "0.25",
                _ => "{\"items\":[1,true,null,{\"nested\":\"ok\"}]}"
            };

            return Task.FromResult(TestHandler.Json(
                $"{{\"flagKey\":\"{key}\",\"value\":{value},\"reason\":\"SPLIT\",\"variant\":\"treatment\"}}"));
        });

        using var http = new HttpClient(handler);
        var api = OpenFeatureFactory.CreateIsolated();

        await api.SetProviderAsync(
            new FlagshipServerProvider(ClientTests.Options, http),
            TestContext.Current.CancellationToken);

        try
        {
            var client = api.GetClient();
            var context = EvaluationContext.Builder()
                .SetTargetingKey("user-123")
                .Set("plan", "pro")
                .Build();

            var boolean = await client.GetBooleanDetailsAsync(
                "boolean",
                false,
                context,
                cancellationToken: TestContext.Current.CancellationToken);

            Assert.True(boolean.Value);
            Assert.Equal(Reason.Split, boolean.Reason);
            Assert.Equal("treatment", boolean.Variant);
            Assert.Equal(ErrorType.None, boolean.ErrorType);

            var text = await client.GetStringValueAsync(
                "string",
                "fallback",
                context,
                cancellationToken: TestContext.Current.CancellationToken);

            Assert.Equal("hello", text);

            var integer = await client.GetIntegerValueAsync(
                "integer",
                0,
                context,
                cancellationToken: TestContext.Current.CancellationToken);

            Assert.Equal(42, integer);

            var number = await client.GetDoubleValueAsync(
                "double",
                0,
                context,
                cancellationToken: TestContext.Current.CancellationToken);

            Assert.Equal(0.25, number);

            var structure = await client.GetObjectValueAsync(
                "object",
                new Value(),
                context,
                cancellationToken: TestContext.Current.CancellationToken);

            var items = structure.AsStructure!.GetValue("items").AsList!;

            Assert.Equal(1, items[0].AsInteger);
            Assert.True(items[1].AsBoolean);
            Assert.True(items[2].IsNull);
            Assert.Equal("ok", items[3].AsStructure!.GetValue("nested").AsString);
        }
        finally
        {
            await api.ShutdownAsync();
        }

        Assert.False(handler.Disposed);
    }

    [Theory]
    [InlineData("true")]
    [InlineData("\"12\"")]
    [InlineData("1.5")]
    [InlineData("2147483648")]
    [InlineData("null")]
    [InlineData("{}")]
    public async Task IntegerMismatchReturnsDefaultWithoutRounding(string value)
    {
        using var handler = new TestHandler((_, _) =>
            Task.FromResult(TestHandler.Json($"{{\"flagKey\":\"flag\",\"value\":{value}}}")));
        using var http = new HttpClient(handler);
        using var provider = new FlagshipServerProvider(ClientTests.Options, http);

        var result = await provider.ResolveIntegerValueAsync(
            "flag",
            7,
            cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(7, result.Value);
        Assert.Equal(ErrorType.TypeMismatch, result.ErrorType);
        Assert.Equal(Reason.Error, result.Reason);
    }

    [Fact]
    public async Task DisabledFlagUsesCallerDefaultWithoutTypeCheckOrVariant()
    {
        using var handler = new TestHandler((_, _) =>
            Task.FromResult(TestHandler.Json(
                "{\"flagKey\":\"flag\",\"value\":null,\"reason\":\"DISABLED\",\"variant\":\"off\"}")));
        using var http = new HttpClient(handler);
        using var provider = new FlagshipServerProvider(ClientTests.Options, http);

        var result = await provider.ResolveBooleanValueAsync(
            "flag",
            true,
            cancellationToken: TestContext.Current.CancellationToken);

        Assert.True(result.Value);
        Assert.Equal(Reason.Disabled, result.Reason);
        Assert.Equal(ErrorType.None, result.ErrorType);
        Assert.Null(result.Variant);
    }

    [Theory]
    [InlineData(404, "{}", ErrorType.FlagNotFound)]
    [InlineData(400, "{}", ErrorType.General)]
    [InlineData(500, "{}", ErrorType.General)]
    [InlineData(200, "{}", ErrorType.ParseError)]
    public async Task OfficialClientReturnsDefaultWithMappedError(int status, string json, ErrorType error)
    {
        using var handler = new TestHandler((_, _) =>
            Task.FromResult(TestHandler.Json(json, (HttpStatusCode)status)));
        using var http = new HttpClient(handler);
        var api = OpenFeatureFactory.CreateIsolated();

        await api.SetProviderAsync(
            new FlagshipServerProvider(ClientTests.Options, http),
            TestContext.Current.CancellationToken);

        try
        {
            var result = await api.GetClient().GetStringDetailsAsync(
                "flag",
                "fallback",
                cancellationToken: TestContext.Current.CancellationToken);

            Assert.Equal("fallback", result.Value);
            Assert.Equal(error, result.ErrorType);
            Assert.Equal(Reason.Error, result.Reason);
        }
        finally
        {
            await api.ShutdownAsync();
        }
    }

    [Fact]
    public async Task InvalidContextAndShutdownAreReportedWithoutNetworkRequests()
    {
        using var handler = new TestHandler((_, _) =>
            Task.FromResult(TestHandler.Json(ClientTests.Success)));
        using var http = new HttpClient(handler);
        using var provider = new FlagshipServerProvider(ClientTests.Options, http);

        var context = EvaluationContext.Builder()
            .Set("nested", new Value(Structure.Empty))
            .Build();

        var result = await provider.ResolveBooleanValueAsync(
            "flag",
            false,
            context,
            cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(ErrorType.InvalidContext, result.ErrorType);

        await provider.ShutdownAsync(TestContext.Current.CancellationToken);

        var afterShutdown = await provider.ResolveBooleanValueAsync(
            "flag",
            false,
            cancellationToken: TestContext.Current.CancellationToken);

        Assert.Equal(ErrorType.ProviderNotReady, afterShutdown.ErrorType);
        Assert.Equal(0, handler.Calls);
    }

    [Fact]
    public async Task ProviderPreservesCallerCancellation()
    {
        using var cancel = CancellationTokenSource.CreateLinkedTokenSource(TestContext.Current.CancellationToken);
        cancel.Cancel();

        using var provider = new FlagshipServerProvider(ClientTests.Options);

        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => provider.ResolveBooleanValueAsync("flag", false, cancellationToken: cancel.Token));
    }
}
