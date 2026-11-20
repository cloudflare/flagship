using System.Text.Json;

namespace Cloudflare.Flagship;

/// <summary>An API evaluation. Value remains valid independently of the response lifetime.</summary>
public sealed record EvaluationResponse(string FlagKey, JsonElement Value, string Reason, string? Variant);
