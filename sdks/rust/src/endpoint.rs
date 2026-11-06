//! Resolution of validated Flagship configuration into API endpoint URLs.

use reqwest::Url;

use crate::config::FlagshipEndpoint;
use crate::error::ConfigError;

const CLIENT_API_PREFIX: &str = "client/v4/accounts";
const APP_PREFIX: &str = "flagship/apps";
const EVALUATE_PATH: &str = "evaluate";
const DEFINITIONS_PATH: &str = "definitions";
const EVALUATE_SUFFIX: &str = "/evaluate";
const DEFINITIONS_SUFFIX: &str = "/definitions";
const DEFINITIONS_DERIVATION_ERROR: &str =
    "explicit endpoint path must end in /evaluate to derive the definitions endpoint";
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

pub(crate) fn resolve_evaluation_endpoint(endpoint: &FlagshipEndpoint) -> Url {
    match endpoint {
        FlagshipEndpoint::App(app) => resolve_app_endpoint(
            app.base_url(),
            app.account_id(),
            app.app_id(),
            EVALUATE_PATH,
        ),
        FlagshipEndpoint::Explicit(explicit) => explicit.endpoint().clone(),
    }
}

pub(crate) fn resolve_definitions_endpoint(
    endpoint: &FlagshipEndpoint,
) -> Result<Url, ConfigError> {
    match endpoint {
        FlagshipEndpoint::App(app) => Ok(resolve_app_endpoint(
            app.base_url(),
            app.account_id(),
            app.app_id(),
            DEFINITIONS_PATH,
        )),
        FlagshipEndpoint::Explicit(explicit) => derive_definitions_endpoint(explicit.endpoint()),
    }
}

fn resolve_app_endpoint(base_url: &Url, account_id: &str, app_id: &str, resource: &str) -> Url {
    let mut endpoint = base_url.clone();

    // Escaping `/` keeps each ID in one path segment; config rejects `.`/`..`.
    let base_path = endpoint.path().trim_end_matches('/');
    let account_id = escape_path_segment(account_id);
    let app_id = escape_path_segment(app_id);
    let suffix = format!("{CLIENT_API_PREFIX}/{account_id}/{APP_PREFIX}/{app_id}/{resource}");
    endpoint.set_path(&format!("{base_path}/{suffix}"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);

    endpoint
}

/// Works on the serialized path so escaped `%2F` stays inside its segment.
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

    fn app_endpoint(app_id: &str, account_id: &str, base_url: Option<&str>) -> FlagshipEndpoint {
        let mut builder = FlagshipClientOptions::builder()
            .app_id(app_id)
            .account_id(account_id);

        if let Some(base_url) = base_url {
            builder = builder.base_url(base_url);
        }

        builder.build_endpoint().expect("valid app configuration")
    }

    fn explicit_endpoint(value: &str) -> FlagshipEndpoint {
        FlagshipClientOptions::builder()
            .endpoint(value)
            .build_endpoint()
            .expect("valid explicit endpoint")
    }

    #[test]
    fn resolves_default_app_endpoints() {
        let config = app_endpoint("app-123", "acct-456", None);

        let evaluation = resolve_evaluation_endpoint(&config);
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

    #[test]
    fn resolves_custom_base_paths_after_trailing_slash_normalization() {
        let config = app_endpoint(
            "app-1",
            "acct-1",
            Some("https://api.example.com/prefix////"),
        );

        let evaluation = resolve_evaluation_endpoint(&config);

        assert_eq!(
            evaluation.as_str(),
            "https://api.example.com/prefix/client/v4/accounts/acct-1/flagship/apps/app-1/evaluate"
        );
    }

    #[test]
    fn encodes_ids_as_single_safe_path_segments() {
        let config = app_endpoint("my app/version%2", "a/b c", None);

        let evaluation = resolve_evaluation_endpoint(&config);

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

    #[test]
    fn resolved_endpoint_retains_go_path_escape_safe_characters() {
        let config = app_endpoint("safe-$&+:=@~. value/part", "account", None);

        let evaluation = resolve_evaluation_endpoint(&config);

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
                .build_endpoint()
                .expect_err("dot segment must be rejected");

            assert_eq!(error.message(), expected);
        }
    }

    #[test]
    fn preserves_explicit_evaluation_endpoint_and_custom_path() {
        let config = explicit_endpoint("https://api.example.com/custom/v1/apps/app/evaluate/");

        let evaluation = resolve_evaluation_endpoint(&config);
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

    #[test]
    fn app_endpoint_round_trips_to_explicit_definitions_with_encoded_ids() {
        let app = app_endpoint("my app/version", "account/team", None);
        let evaluation = resolve_evaluation_endpoint(&app);
        let explicit = explicit_endpoint(evaluation.as_str());

        let app_definitions = resolve_definitions_endpoint(&app).expect("app definitions endpoint");
        let explicit_definitions =
            resolve_definitions_endpoint(&explicit).expect("explicit definitions endpoint");

        assert_eq!(explicit_definitions, app_definitions);
        assert!(explicit_definitions.as_str().contains("account%2Fteam"));
        assert!(explicit_definitions.as_str().contains("my%20app%2Fversion"));
    }

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

    #[test]
    fn encoded_slash_does_not_satisfy_evaluate_suffix() {
        let config = explicit_endpoint("https://api.example.com/apps/app%2Fevaluate");

        let error = resolve_definitions_endpoint(&config).expect_err("derivation should fail");

        assert_eq!(error.message(), DEFINITIONS_DERIVATION_ERROR);
    }

    #[test]
    fn derives_definitions_from_root_evaluate_path() {
        let config = explicit_endpoint("https://api.example.com/evaluate");

        let definitions = resolve_definitions_endpoint(&config).expect("definitions endpoint");

        assert_eq!(definitions.as_str(), "https://api.example.com/definitions");
    }

    #[test]
    fn definitions_derivation_requires_trailing_evaluate_segment() {
        let config = explicit_endpoint("https://api.example.com/custom/evaluate-flags");

        let error = resolve_definitions_endpoint(&config).expect_err("derivation should fail");

        assert_eq!(error.message(), DEFINITIONS_DERIVATION_ERROR);
        assert!(!error.to_string().contains("api.example.com"));
    }

    #[test]
    fn resolved_urls_never_gain_a_query_or_fragment() {
        let app = app_endpoint("app", "account", Some("https://api.example.com/base"));
        let explicit = explicit_endpoint("https://api.example.com/custom/evaluate");

        for endpoint in [
            resolve_evaluation_endpoint(&app),
            resolve_definitions_endpoint(&app).expect("app definitions endpoint"),
            resolve_evaluation_endpoint(&explicit),
            resolve_definitions_endpoint(&explicit).expect("explicit definitions endpoint"),
        ] {
            assert!(endpoint.query().is_none());
            assert!(endpoint.fragment().is_none());
        }

        let error = FlagshipClientOptions::builder()
            .endpoint("https://api.example.com/evaluate?token=secret#fragment")
            .build_endpoint()
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
