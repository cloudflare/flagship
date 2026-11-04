//! In-process evaluation with periodically refreshed definitions.
//!
//! Requires `FLAGSHIP_APP_ID`, `FLAGSHIP_ACCOUNT_ID`, and `FLAGSHIP_READ_TOKEN`
//! (a token with app read permission).

use std::time::Duration;

use cloudflare_flagship::open_feature::{EvaluationContext, EvaluationError, OpenFeature};
use cloudflare_flagship::{FlagshipClientOptions, FlagshipServerProvider, LoggingHook};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app_id = std::env::var("FLAGSHIP_APP_ID")?;
    let account_id = std::env::var("FLAGSHIP_ACCOUNT_ID")?;
    let read_token = std::env::var("FLAGSHIP_READ_TOKEN")?;
    let options = FlagshipClientOptions::builder()
        .app_id(app_id)
        .account_id(account_id)
        .auth_token(read_token)
        .build()?;
    let provider = FlagshipServerProvider::builder(options)
        .local_evaluation(true)
        .refresh_interval(Duration::from_secs(30))
        .hook(LoggingHook::new(|event| {
            eprintln!("{:?}: {:?}", event.kind, event.flag_key);
        }))
        .build()?;

    // OpenFeature 0.3 has no provider init or shutdown callbacks.
    provider.initialize_provider().await?;

    let mut api = OpenFeature::default();
    api.set_provider(provider.clone()).await;
    let client = api.create_named_client("local-example");
    let context = EvaluationContext::default()
        .with_targeting_key("user-123")
        .with_custom_field("plan", "premium");
    let dark_mode = client
        .get_bool_value("dark-mode", Some(&context), None)
        .await
        .map_err(eval_error)?;
    let details = client
        .get_bool_details("premium-features", Some(&context), None)
        .await
        .map_err(eval_error)?;

    println!("dark-mode: {dark_mode}");
    println!(
        "premium-features: value={} reason={:?} variant={:?}",
        details.value, details.reason, details.variant
    );

    provider.shutdown_provider().await;

    Ok(())
}

fn eval_error(error: EvaluationError) -> std::io::Error {
    let message = error.message.unwrap_or_default();

    std::io::Error::other(format!("{:?}: {message}", error.code))
}
