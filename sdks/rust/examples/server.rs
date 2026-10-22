//! Remote Flagship evaluation through `OpenFeature` and the direct client.
//!
//! Set `FLAGSHIP_APP_ID`, `FLAGSHIP_ACCOUNT_ID`, and `FLAGSHIP_AUTH_TOKEN`
//! before running this example. Primitive context values are sent in the GET
//! request URL, so do not put sensitive data in them.

use std::time::Duration;

use cloudflare_flagship::open_feature::{
    EvaluationContext, EvaluationErrorCode, OpenFeature, StructValue,
};
use cloudflare_flagship::{
    FlagshipClient, FlagshipClientOptions, FlagshipServerProvider, LoggingHook, TelemetryHook,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app_id = std::env::var("FLAGSHIP_APP_ID")?;
    let account_id = std::env::var("FLAGSHIP_ACCOUNT_ID")?;
    let auth_token = std::env::var("FLAGSHIP_AUTH_TOKEN")?;
    let options = FlagshipClientOptions::builder()
        .app_id(app_id)
        .account_id(account_id)
        .auth_token(auth_token)
        .build()?;

    let logging = LoggingHook::new(|event| {
        eprintln!("{:?}: {}", event.stage(), event.flag_key());
    });
    let telemetry = TelemetryHook::new(|event| {
        // Telemetry may contain customer context and resolved values.
        let _ = event;
    });
    let provider = FlagshipServerProvider::builder(options.clone())
        .cache_ttl(Duration::from_secs(30))
        .cache_max_entries(1_000)
        .hook(logging)
        .hook(telemetry)
        .build()?;

    let mut api = OpenFeature::default();
    api.set_provider(provider).await;
    let client = api.create_named_client("server-example");
    let context = EvaluationContext::default()
        .with_targeting_key("user-123")
        .with_custom_field("plan", "premium")
        .with_custom_field("country", "US")
        .with_custom_field("age", 25);

    let dark_mode = client
        .get_bool_value("dark-mode", Some(&context), None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let welcome_message = client
        .get_string_value("welcome-message", Some(&context), None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let max_uploads = client
        .get_int_value("max-uploads", Some(&context), None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let sample_rate = client
        .get_float_value("sample-rate", Some(&context), None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let theme_config = client
        .get_struct_value::<StructValue>("theme-config", Some(&context), None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let details = client
        .get_bool_details("premium-features", Some(&context), None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;

    println!("dark-mode: {dark_mode}");
    println!("welcome-message: {welcome_message:?}");
    println!("max-uploads: {max_uploads}");
    println!("sample-rate: {sample_rate}");
    println!("theme-config: {theme_config:?}");
    println!(
        "premium-features: value={} reason={:?} variant={:?}",
        details.value, details.reason, details.variant
    );

    // OpenFeature 0.3 for Rust returns errors instead of accepting a caller
    // default, so apply the missing-flag fallback explicitly without hiding
    // transport, authentication, or other evaluation failures.
    let unknown_flag = match client
        .get_bool_value("non-existent-flag", Some(&context), None)
        .await
    {
        Ok(value) => value,
        Err(error) if error.code == EvaluationErrorCode::FlagNotFound => false,
        Err(error) => return Err(std::io::Error::other(format!("{error:?}")).into()),
    };
    println!("non-existent-flag: {unknown_flag}");

    let beta_access = client
        .get_bool_value("beta-access", None, None)
        .await
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    println!("beta-access: {beta_access}");

    let direct_client = FlagshipClient::new(options)?;
    let response = direct_client.evaluate("welcome-message", &context).await?;
    println!(
        "direct welcome-message: {:?} ({:?})",
        response.value, response.reason
    );

    Ok(())
}
