using System.Text.Json;
using OpenFeature;
using OpenFeature.Constant;
using OpenFeature.Model;

namespace Cloudflare.Flagship.OpenFeature;

/// <summary>OpenFeature server provider using per-evaluation HTTP requests.</summary>
public sealed class FlagshipServerProvider : FeatureProvider, IDisposable
{
    private readonly FlagshipClient _client;

    public FlagshipServerProvider(FlagshipOptions options, HttpClient? httpClient = null)
    {
        _client = new FlagshipClient(options, httpClient);
    }

    public override Metadata GetMetadata() => new("flagship");

    public override Task<ResolutionDetails<bool>> ResolveBooleanValueAsync(
        string flagKey,
        bool defaultValue,
        EvaluationContext? context = null,
        CancellationToken cancellationToken = default) =>
        ResolveAsync(flagKey, defaultValue, context, value => value.GetBoolean(), cancellationToken);

    public override Task<ResolutionDetails<string>> ResolveStringValueAsync(
        string flagKey,
        string defaultValue,
        EvaluationContext? context = null,
        CancellationToken cancellationToken = default) =>
        ResolveAsync(
            flagKey,
            defaultValue,
            context,
            value => value.ValueKind == JsonValueKind.String
                ? value.GetString()!
                : throw new InvalidOperationException(),
            cancellationToken);

    public override Task<ResolutionDetails<int>> ResolveIntegerValueAsync(
        string flagKey,
        int defaultValue,
        EvaluationContext? context = null,
        CancellationToken cancellationToken = default) =>
        ResolveAsync(flagKey, defaultValue, context, ToInteger, cancellationToken);

    public override Task<ResolutionDetails<double>> ResolveDoubleValueAsync(
        string flagKey,
        double defaultValue,
        EvaluationContext? context = null,
        CancellationToken cancellationToken = default) =>
        ResolveAsync(flagKey, defaultValue, context, ToDouble, cancellationToken);

    public override Task<ResolutionDetails<Value>> ResolveStructureValueAsync(
        string flagKey,
        Value defaultValue,
        EvaluationContext? context = null,
        CancellationToken cancellationToken = default) =>
        ResolveAsync(
            flagKey,
            defaultValue,
            context,
            value => value.ValueKind is JsonValueKind.Object or JsonValueKind.Array
                ? ToValue(value)
                : throw new InvalidOperationException(),
            cancellationToken);

    private async Task<ResolutionDetails<T>> ResolveAsync<T>(
        string flagKey,
        T defaultValue,
        EvaluationContext? context,
        Func<JsonElement, T> convert,
        CancellationToken cancellationToken)
    {
        try
        {
            var attributes = context?.AsDictionary()
                .ToDictionary(pair => pair.Key, pair => pair.Value.AsObject);

            var response = await _client
                .EvaluateAsync(flagKey, attributes, cancellationToken)
                .ConfigureAwait(false);

            if (response.Reason == Reason.Disabled)
            {
                return new ResolutionDetails<T>(flagKey, defaultValue, reason: Reason.Disabled);
            }

            T value;

            try
            {
                value = convert(response.Value);
            }
            catch (Exception ex) when (ex is InvalidOperationException or FormatException or OverflowException)
            {
                return new ResolutionDetails<T>(
                    flagKey,
                    defaultValue,
                    ErrorType.TypeMismatch,
                    Reason.Error,
                    errorMessage: "Flag value does not match the requested type.");
            }

            return new ResolutionDetails<T>(flagKey, value, reason: response.Reason, variant: response.Variant);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            throw;
        }
        catch (FlagshipException ex)
        {
            var error = ex.Code switch
            {
                FlagshipErrorCode.FlagNotFound => ErrorType.FlagNotFound,
                FlagshipErrorCode.InvalidContext => ErrorType.InvalidContext,
                FlagshipErrorCode.ParseError => ErrorType.ParseError,
                _ => ErrorType.General
            };

            return new ResolutionDetails<T>(flagKey, defaultValue, error, Reason.Error, errorMessage: ex.Message);
        }
        catch (ObjectDisposedException)
        {
            return new ResolutionDetails<T>(
                flagKey,
                defaultValue,
                ErrorType.ProviderNotReady,
                Reason.Error,
                errorMessage: "Flagship provider has been shut down.");
        }
    }

    private static int ToInteger(JsonElement value)
    {
        // Accept JSON numbers such as 2.0, but never round fractional values.
        var number = value.GetDecimal();

        if (number != decimal.Truncate(number))
        {
            throw new FormatException();
        }

        return checked((int)number);
    }

    private static double ToDouble(JsonElement value)
    {
        var number = value.GetDouble();

        return double.IsFinite(number) ? number : throw new FormatException();
    }

    private static Value ToValue(JsonElement value) => value.ValueKind switch
    {
        JsonValueKind.Object => new Value(new Structure(
            value.EnumerateObject().ToDictionary(p => p.Name, p => ToValue(p.Value)))),
        JsonValueKind.Array => new Value(value.EnumerateArray().Select(ToValue).ToList()),
        JsonValueKind.String => new Value(value.GetString()!),
        JsonValueKind.Number => new Value(ToDouble(value)),
        JsonValueKind.True => new Value(true),
        JsonValueKind.False => new Value(false),
        JsonValueKind.Null => new Value(),
        _ => throw new FormatException()
    };

    public override Task ShutdownAsync(CancellationToken cancellationToken = default)
    {
        Dispose();

        return Task.CompletedTask;
    }

    public void Dispose() => _client.Dispose();
}
