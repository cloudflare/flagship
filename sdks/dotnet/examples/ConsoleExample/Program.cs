using Cloudflare.Flagship;
using Cloudflare.Flagship.OpenFeature;
using OpenFeature;
using OpenFeature.Model;

var provider = new FlagshipServerProvider(new FlagshipOptions
{
    AppId = Environment.GetEnvironmentVariable("FLAGSHIP_APP_ID"),
    AccountId = Environment.GetEnvironmentVariable("CLOUDFLARE_ACCOUNT_ID"),
    AuthToken = Environment.GetEnvironmentVariable("CLOUDFLARE_API_TOKEN")
});

await Api.Instance.SetProviderAsync(provider);

try
{
    var context = EvaluationContext.Builder().SetTargetingKey("user-123").Set("plan", "premium").Build();

    var enabled = await Api.Instance.GetClient().GetBooleanValueAsync("dark-mode", false, context);

    Console.WriteLine($"Dark mode: {enabled}");
}
finally
{
    await Api.Instance.ShutdownAsync();
}
