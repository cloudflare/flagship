//! Resolution of validated Flagship configuration into API endpoint URLs.
//!
//! Configuration validation belongs to [`crate::config`]. This module only
//! constructs evaluation URLs and derives definitions URLs from those validated
//! values.

use crate::config::{FlagshipEndpoint, Url};
use crate::error::ConfigError;

const CLIENT_API_PREFIX: &str = "client/v4/accounts";
const APP_PREFIX: &str = "flagship/apps";
const EVALUATE_PATH: &str = "evaluate";
const DEFINITIONS_PATH: &str = "definitions";
const EVALUATE_SUFFIX: &str = "/evaluate";
const DEFINITIONS_SUFFIX: &str = "/definitions";
const EVALUATION_ACCOUNT_ERROR: &str = "account_id is required to resolve the evaluation endpoint";
const DEFINITIONS_ACCOUNT_ERROR: &str =
    "account_id is required to resolve the definitions endpoint";
const BASE_URL_ERROR: &str = "configured base_url cannot be used to construct an API endpoint";
const DEFINITIONS_DERIVATION_ERROR: &str =
    "explicit endpoint path must end in /evaluate to derive the definitions endpoint";
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Resolves the remote-evaluation URL from validated endpoint configuration.
pub(crate) fn resolve_evaluation_endpoint(endpoint: &FlagshipEndpoint) -> Result<Url, ConfigError> {
    match endpoint {
        FlagshipEndpoint::App(app) => {
            let account_id = endpoint
                .account_id()
                .ok_or_else(|| ConfigError::new(EVALUATION_ACCOUNT_ERROR))?;

            resolve_app_endpoint(app.base_url(), account_id, app.app_id(), EVALUATE_PATH)
        }
        FlagshipEndpoint::Explicit(explicit) => Ok(explicit.endpoint().clone()),
    }
}

/// Resolves the local-evaluation definitions URL from validated configuration.
pub(crate) fn resolve_definitions_endpoint(
    endpoint: &FlagshipEndpoint,
) -> Result<Url, ConfigError> {
    match endpoint {
        FlagshipEndpoint::App(app) => {
            let account_id = endpoint
                .account_id()
                .ok_or_else(|| ConfigError::new(DEFINITIONS_ACCOUNT_ERROR))?;

            resolve_app_endpoint(app.base_url(), account_id, app.app_id(), DEFINITIONS_PATH)
        }
        FlagshipEndpoint::Explicit(explicit) => derive_definitions_endpoint(explicit.endpoint()),
    }
}

fn resolve_app_endpoint(
    base_url: &Url,
    account_id: &str,
    app_id: &str,
    resource: &str,
) -> Result<Url, ConfigError> {
    let mut endpoint = base_url.clone();
    if endpoint.cannot_be_a_base() {
        return Err(ConfigError::new(BASE_URL_ERROR));
    }

    // `set_path` normalizes literal dot segments. Config validation rejects `.`
    // and `..` slash-delimited ID segments before they reach this point, while
    // escaping `/` keeps every validated ID in exactly one URL path segment.
    let base_path = endpoint.path().trim_end_matches('/');
    let account_id = escape_path_segment(account_id);
    let app_id = escape_path_segment(app_id);
    let suffix = format!("{CLIENT_API_PREFIX}/{account_id}/{APP_PREFIX}/{app_id}/{resource}");
    endpoint.set_path(&format!("{base_path}/{suffix}"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);

    Ok(endpoint)
}

/// Derives a definitions URL without decoding already escaped path segments.
///
/// Go's `url.URL.Path` replacement can turn an encoded slash into a path
/// separator. Operating on the serialized path deliberately differs from that
/// buggy behavior and keeps `%2F` inside the original ID segment.
fn derive_definitions_endpoint(evaluation_endpoint: &Url) -> Result<Url, ConfigError> {
    let normalized_path = evaluation_endpoint.path().trim_end_matches('/');
    let Some(prefix) = normalized_path.strip_suffix(EVALUATE_SUFFIX) else {
        return Err(ConfigError::new(DEFINITIONS_DERIVATION_ERROR));
    };

    let mut endpoint = evaluation_endpoint.clone();
    endpoint.set_path(&format!("{prefix}{DEFINITIONS_SUFFIX}"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);

    Ok(endpoint)
}

fn escape_path_segment(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for byte in value.bytes() {
        if is_path_escape_safe(byte) {
            escaped.push(char::from(byte));
        } else {
            escaped.push('%');
            escaped.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            escaped.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
    }

    escaped
}

const fn is_path_escape_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b'~' | b'$' | b'&' | b'+' | b':' | b'=' | b'@'
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FlagshipClientOptions;

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    fn app_endpoint(app_id: &str, account_id: &str, base_url: Option<&str>) -> FlagshipEndpoint {
        let mut builder = FlagshipClientOptions::builder()
            .app_id(app_id)
            .account_id(account_id);

        if let Some(base_url) = base_url {
            builder = builder.base_url(base_url);
        }

        builder
            .build()
            .expect("valid app configuration")
            .endpoint()
            .clone()
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    fn explicit_endpoint(value: &str) -> FlagshipEndpoint {
        FlagshipClientOptions::builder()
            .endpoint(value)
            .build()
            .expect("valid explicit endpoint")
            .endpoint()
            .clone()
    }

    #[cfg(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    ))]
    #[test]
    fn definitions_resolver_compiles_without_constructing_a_client() {
        let _: fn(&FlagshipEndpoint) -> Result<Url, ConfigError> = resolve_definitions_endpoint;
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn resolves_default_app_endpoints() {
        let config = app_endpoint("app-123", "acct-456", None);

        let evaluation = resolve_evaluation_endpoint(&config).expect("evaluation endpoint");
        let definitions = resolve_definitions_endpoint(&config).expect("definitions endpoint");

        assert_eq!(
            evaluation.as_str(),
            "https://api.cloudflare.com/client/v4/accounts/acct-456/flagship/apps/app-123/evaluate"
        );
        assert_eq!(
            definitions.as_str(),
            "https://api.cloudflare.com/client/v4/accounts/acct-456/flagship/apps/app-123/definitions"
        );
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn resolves_custom_base_paths_after_trailing_slash_normalization() {
        let config = app_endpoint(
            "app-1",
            "acct-1",
            Some("https://api.example.com/prefix////"),
        );

        let evaluation = resolve_evaluation_endpoint(&config).expect("evaluation endpoint");

        assert_eq!(
            evaluation.as_str(),
            "https://api.example.com/prefix/client/v4/accounts/acct-1/flagship/apps/app-1/evaluate"
        );
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn encodes_ids_as_single_safe_path_segments() {
        let config = app_endpoint("my app/version%2", "a/b c", None);

        let evaluation = resolve_evaluation_endpoint(&config).expect("evaluation endpoint");

        assert_eq!(
            evaluation.as_str(),
            concat!(
                "https://api.cloudflare.com/client/v4/accounts/a%2Fb%20c/",
                "flagship/apps/my%20app%2Fversion%252/evaluate"
            )
        );
    }

    #[test]
    fn escaping_matches_go_path_escape_for_all_ascii_punctuation() {
        let punctuation = " !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

        assert_eq!(
            escape_path_segment(punctuation),
            "%20%21%22%23$%25&%27%28%29%2A+%2C-.%2F:%3B%3C=%3E%3F@%5B%5C%5D%5E_%60%7B%7C%7D~"
        );
        assert_eq!(escape_path_segment("AZaz09"), "AZaz09");
        assert_eq!(escape_path_segment("café/☃"), "caf%C3%A9%2F%E2%98%83");
        assert_eq!(escape_path_segment("\0\u{7f}"), "%00%7F");
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn resolved_endpoint_retains_go_path_escape_safe_characters() {
        let config = app_endpoint("safe-$&+:=@~. value/part", "account", None);

        let evaluation = resolve_evaluation_endpoint(&config).expect("evaluation endpoint");

        assert!(
            evaluation
                .as_str()
                .ends_with("/apps/safe-$&+:=@~.%20value%2Fpart/evaluate")
        );
    }

    #[test]
    fn rejects_dot_segment_ids_before_url_resolution() {
        for (app_id, account_id, expected) in [
            (".", "account", "app_id must not contain dot path segments"),
            ("app", "..", "account_id must not contain dot path segments"),
        ] {
            let error = FlagshipClientOptions::builder()
                .app_id(app_id)
                .account_id(account_id)
                .build()
                .expect_err("dot segment must be rejected");

            assert_eq!(error.message(), expected);
        }
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn preserves_explicit_evaluation_endpoint_and_custom_path() {
        let config = explicit_endpoint("https://api.example.com/custom/v1/apps/app/evaluate/");

        let evaluation = resolve_evaluation_endpoint(&config).expect("evaluation endpoint");
        let definitions = resolve_definitions_endpoint(&config).expect("definitions endpoint");

        assert_eq!(
            evaluation.as_str(),
            "https://api.example.com/custom/v1/apps/app/evaluate/"
        );
        assert_eq!(
            definitions.as_str(),
            "https://api.example.com/custom/v1/apps/app/definitions"
        );
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn app_endpoint_round_trips_to_explicit_definitions_with_encoded_ids() {
        let app = app_endpoint("my app/version", "account/team", None);
        let evaluation = resolve_evaluation_endpoint(&app).expect("app evaluation endpoint");
        let explicit = explicit_endpoint(evaluation.as_str());

        let app_definitions = resolve_definitions_endpoint(&app).expect("app definitions endpoint");
        let explicit_definitions =
            resolve_definitions_endpoint(&explicit).expect("explicit definitions endpoint");

        assert_eq!(explicit_definitions, app_definitions);
        assert!(explicit_definitions.as_str().contains("account%2Fteam"));
        assert!(explicit_definitions.as_str().contains("my%20app%2Fversion"));
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn definitions_derivation_preserves_encoded_slashes() {
        let config = explicit_endpoint(
            "https://api.example.com/accounts/account%2Fteam/apps/app%2Fversion/evaluate",
        );

        let definitions = resolve_definitions_endpoint(&config).expect("definitions endpoint");

        assert_eq!(
            definitions.as_str(),
            "https://api.example.com/accounts/account%2Fteam/apps/app%2Fversion/definitions"
        );
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn encoded_slash_does_not_satisfy_evaluate_suffix() {
        let config = explicit_endpoint("https://api.example.com/apps/app%2Fevaluate");

        let error = resolve_definitions_endpoint(&config).expect_err("derivation should fail");

        assert_eq!(error.message(), DEFINITIONS_DERIVATION_ERROR);
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn derives_definitions_from_root_evaluate_path() {
        let config = explicit_endpoint("https://api.example.com/evaluate");

        let definitions = resolve_definitions_endpoint(&config).expect("definitions endpoint");

        assert_eq!(definitions.as_str(), "https://api.example.com/definitions");
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn definitions_derivation_requires_trailing_evaluate_segment() {
        let config = explicit_endpoint("https://api.example.com/custom/evaluate-flags");

        let error = resolve_definitions_endpoint(&config).expect_err("derivation should fail");

        assert_eq!(error.message(), DEFINITIONS_DERIVATION_ERROR);
        assert!(!error.to_string().contains("api.example.com"));
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    #[test]
    fn resolved_urls_never_gain_a_query_or_fragment() {
        let app = app_endpoint("app", "account", Some("https://api.example.com/base"));
        let explicit = explicit_endpoint("https://api.example.com/custom/evaluate");

        for endpoint in [
            resolve_evaluation_endpoint(&app).expect("app evaluation endpoint"),
            resolve_definitions_endpoint(&app).expect("app definitions endpoint"),
            resolve_evaluation_endpoint(&explicit).expect("explicit evaluation endpoint"),
            resolve_definitions_endpoint(&explicit).expect("explicit definitions endpoint"),
        ] {
            assert!(endpoint.query().is_none());
            assert!(endpoint.fragment().is_none());
        }

        let error = FlagshipClientOptions::builder()
            .endpoint("https://api.example.com/evaluate?token=secret#fragment")
            .build()
            .expect_err("query and fragment must be rejected");
        assert_eq!(
            error.message(),
            "endpoint must not contain a query or fragment"
        );
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn definitions_derivation_defensively_clears_query_and_fragment() {
        let evaluation = Url::parse(
            "https://api.example.com/custom/evaluate?credential=secret#private-fragment",
        )
        .expect("test URL");

        let definitions = derive_definitions_endpoint(&evaluation).expect("definitions endpoint");

        assert_eq!(
            definitions.as_str(),
            "https://api.example.com/custom/definitions"
        );
        assert!(definitions.query().is_none());
        assert!(definitions.fragment().is_none());
    }
}
