//! Flagship wire-format and domain types.
//!
//! These types model the native Flagship server API and are independent of
//! `OpenFeature`'s evaluation reason and resolution-detail types.

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// The native Flagship reason for selecting an evaluation value.
///
/// The named constants cover reasons currently defined by Flagship. Values
/// introduced by a newer server are preserved verbatim when deserialized, so
/// callers should compare against known constants rather than assume this list
/// is exhaustive. This wire reason is separate from `OpenFeature`'s reason type;
/// providers are responsible for mapping between them.
///
/// Known constants can be used as patterns:
///
/// ```
/// use cloudflare_flagship::types::EvaluationReason;
///
/// let reason = EvaluationReason::from("TARGETING_MATCH");
/// let description = match reason {
///     EvaluationReason::STATIC => "static",
///     EvaluationReason::TARGETING_MATCH => "targeted",
///     EvaluationReason::DEFAULT => "default",
///     EvaluationReason::DISABLED => "disabled",
///     EvaluationReason::SPLIT => "split",
///     _ => "reason introduced by a newer server",
/// };
///
/// assert_eq!(description, "targeted");
/// ```
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct EvaluationReason(EvaluationReasonRepr);

#[derive(Clone, Eq, Hash, PartialEq)]
enum EvaluationReasonRepr {
    Static,
    TargetingMatch,
    Default,
    Disabled,
    Split,
    Unknown(String),
}

impl EvaluationReason {
    /// The enabled flag has no rules and served its default variation.
    pub const STATIC: Self = Self(EvaluationReasonRepr::Static);

    /// A targeting rule without a partial percentage rollout matched.
    pub const TARGETING_MATCH: Self = Self(EvaluationReasonRepr::TargetingMatch);

    /// The flag has rules, but none matched, so its default variation was served.
    pub const DEFAULT: Self = Self(EvaluationReasonRepr::Default);

    /// The flag is disabled and served its default variation.
    pub const DISABLED: Self = Self(EvaluationReasonRepr::Disabled);

    /// A matching rule's partial percentage rollout selected the variation.
    pub const SPLIT: Self = Self(EvaluationReasonRepr::Split);

    /// Returns this reason's exact Flagship wire string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            EvaluationReasonRepr::Static => "STATIC",
            EvaluationReasonRepr::TargetingMatch => "TARGETING_MATCH",
            EvaluationReasonRepr::Default => "DEFAULT",
            EvaluationReasonRepr::Disabled => "DISABLED",
            EvaluationReasonRepr::Split => "SPLIT",
            EvaluationReasonRepr::Unknown(value) => value,
        }
    }

    fn known(value: &str) -> Option<Self> {
        match value {
            "STATIC" => Some(Self::STATIC),
            "TARGETING_MATCH" => Some(Self::TARGETING_MATCH),
            "DEFAULT" => Some(Self::DEFAULT),
            "DISABLED" => Some(Self::DISABLED),
            "SPLIT" => Some(Self::SPLIT),
            _ => None,
        }
    }
}

impl Default for EvaluationReason {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for EvaluationReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.as_str())
    }
}

impl fmt::Debug for EvaluationReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("EvaluationReason")
            .field(&self.as_str())
            .finish()
    }
}

impl FromStr for EvaluationReason {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let reason = Self::from(value);

        Ok(reason)
    }
}

impl From<&str> for EvaluationReason {
    fn from(value: &str) -> Self {
        if let Some(reason) = Self::known(value) {
            return reason;
        }

        Self(EvaluationReasonRepr::Unknown(value.to_owned()))
    }
}

impl From<String> for EvaluationReason {
    fn from(value: String) -> Self {
        if let Some(reason) = Self::known(&value) {
            return reason;
        }

        Self(EvaluationReasonRepr::Unknown(value))
    }
}

impl AsRef<str> for EvaluationReason {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Serialize for EvaluationReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EvaluationReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let reason = Self::from(value);

        Ok(reason)
    }
}

fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let value = Option::<T>::deserialize(deserializer)?;

    Ok(value.unwrap_or_default())
}

/// A successful response from the native Flagship evaluation API.
///
/// `value` retains any JSON value, including `null`. A missing or `null`
/// `variant` is represented as an empty string, matching the string field used
/// by the Go and Python SDKs. Missing or `null` reasons default to
/// [`EvaluationReason::DEFAULT`].
///
/// JSON numbers follow [`serde_json::Number`] limits. Integers from [`i64::MIN`]
/// through [`u64::MAX`] retain an integer representation; integers outside that
/// range are represented as floating-point numbers and may lose precision.
///
/// The type is non-exhaustive so new wire fields can be added compatibly. Use
/// [`Self::new`] and the builder methods rather than a struct literal.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "camelCase")]
pub struct FlagshipEvaluationResponse {
    /// The key of the evaluated flag.
    pub flag_key: String,

    /// The JSON value selected for the flag, subject to [`serde_json::Number`] limits.
    pub value: Value,

    /// The key of the selected variant, or an empty string when missing or `null`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub variant: String,

    /// The reason this value was selected, defaulting to
    /// [`EvaluationReason::DEFAULT`] when missing or `null`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub reason: EvaluationReason,
}

impl FlagshipEvaluationResponse {
    /// Creates an evaluation response with an empty variant and default reason.
    #[must_use]
    pub fn new(flag_key: impl Into<String>, value: Value) -> Self {
        Self {
            flag_key: flag_key.into(),
            value,
            variant: String::new(),
            reason: EvaluationReason::DEFAULT,
        }
    }

    /// Sets the served variant key.
    #[must_use]
    pub fn with_variant(mut self, variant: impl Into<String>) -> Self {
        self.variant = variant.into();

        self
    }

    /// Sets the evaluation reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<EvaluationReason>) -> Self {
        self.reason = reason.into();

        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::str::FromStr;

    use serde_json::{Value, json};

    use super::{EvaluationReason, FlagshipEvaluationResponse};

    fn parse_response_value(raw_value: &str) -> Value {
        let wire = format!(r#"{{"flagKey":"number","value":{raw_value}}}"#);
        let response: FlagshipEvaluationResponse = serde_json::from_str(&wire).unwrap();

        response.value
    }

    #[test]
    fn known_reasons_support_string_apis_and_round_trip() {
        let reasons = [
            (EvaluationReason::STATIC, "STATIC"),
            (EvaluationReason::TARGETING_MATCH, "TARGETING_MATCH"),
            (EvaluationReason::DEFAULT, "DEFAULT"),
            (EvaluationReason::DISABLED, "DISABLED"),
            (EvaluationReason::SPLIT, "SPLIT"),
        ];

        for (reason, expected) in reasons {
            assert_eq!(reason.as_str(), expected);
            assert_eq!(reason.as_ref(), expected);
            assert_eq!(reason.to_string(), expected);
            assert_eq!(EvaluationReason::from_str(expected), Ok(reason.clone()));
            assert_eq!(serde_json::to_value(&reason).unwrap(), json!(expected));
            assert_eq!(
                serde_json::from_value::<EvaluationReason>(json!(expected)).unwrap(),
                reason
            );
        }
    }

    #[test]
    fn default_reason_is_default() {
        assert_eq!(EvaluationReason::default(), EvaluationReason::DEFAULT);
    }

    #[test]
    fn unknown_reasons_are_preserved_and_round_trip() {
        let reason = EvaluationReason::from_str("NEW_REASON").unwrap();

        assert_eq!(reason.as_str(), "NEW_REASON");
        assert_eq!(serde_json::to_value(&reason).unwrap(), json!("NEW_REASON"));
        assert_eq!(
            serde_json::from_value::<EvaluationReason>(json!("NEW_REASON")).unwrap(),
            reason
        );
    }

    #[test]
    fn parsing_normalizes_known_reasons() {
        for expected in [
            EvaluationReason::STATIC,
            EvaluationReason::TARGETING_MATCH,
            EvaluationReason::DEFAULT,
            EvaluationReason::DISABLED,
            EvaluationReason::SPLIT,
        ] {
            let parsed = EvaluationReason::from_str(expected.as_str()).unwrap();
            let deserialized: EvaluationReason =
                serde_json::from_value(json!(expected.as_str())).unwrap();

            assert_eq!(parsed, expected);
            assert_eq!(deserialized, expected);
        }
    }

    #[test]
    fn known_reasons_are_usable_as_patterns() {
        let reason = EvaluationReason::from("STATIC");
        let matched = matches!(reason, EvaluationReason::STATIC);

        assert!(matched);
    }

    #[test]
    fn string_conversions_normalize_known_and_preserve_unknown_reasons() {
        assert_eq!(EvaluationReason::from("DEFAULT"), EvaluationReason::DEFAULT);
        assert_eq!(
            EvaluationReason::from(String::from("SPLIT")),
            EvaluationReason::SPLIT
        );
        assert_eq!(EvaluationReason::from("FUTURE").as_str(), "FUTURE");
        assert_eq!(
            EvaluationReason::from(String::from("FUTURE")).as_str(),
            "FUTURE"
        );
    }

    #[test]
    fn empty_reason_is_preserved_as_unknown() {
        let reason = EvaluationReason::from("");

        assert_eq!(reason.as_str(), "");
        assert_ne!(reason, EvaluationReason::DEFAULT);
        assert_eq!(
            serde_json::from_value::<EvaluationReason>(json!("")).unwrap(),
            reason
        );
    }

    #[test]
    fn reasons_hash_by_normalized_value() {
        let reasons = HashSet::from([
            EvaluationReason::DEFAULT,
            EvaluationReason::from("DEFAULT"),
            EvaluationReason::from(String::from("FUTURE")),
            EvaluationReason::from("FUTURE"),
        ]);

        assert_eq!(reasons.len(), 2);
        assert!(reasons.contains(&EvaluationReason::DEFAULT));
        assert!(reasons.contains(&EvaluationReason::from("FUTURE")));
    }

    #[test]
    fn display_honors_formatter_width_alignment_and_precision() {
        assert_eq!(format!("{:>10}", EvaluationReason::STATIC), "    STATIC");
        assert_eq!(format!("{:<10}", EvaluationReason::STATIC), "STATIC    ");
        assert_eq!(format!("{:.4}", EvaluationReason::STATIC), "STAT");
        assert_eq!(format!("{:^10.4}", EvaluationReason::STATIC), "   STAT   ");
    }

    #[test]
    fn debug_uses_wire_strings_instead_of_private_representation() {
        assert_eq!(
            format!("{:?}", EvaluationReason::TARGETING_MATCH),
            r#"EvaluationReason("TARGETING_MATCH")"#
        );
        assert_eq!(
            format!("{:?}", EvaluationReason::from("FUTURE_REASON")),
            r#"EvaluationReason("FUTURE_REASON")"#
        );
    }

    #[test]
    fn response_accepts_every_json_value_shape() {
        let values = [
            Value::Null,
            json!(true),
            json!(false),
            json!(0),
            json!(-42.5),
            json!("text"),
            json!([]),
            json!([null, true, 3, "four", [], {}]),
            json!({}),
            json!({"nested": {"array": [1, false, null]}}),
        ];

        for value in values {
            let response: FlagshipEvaluationResponse = serde_json::from_value(json!({
                "flagKey": "example",
                "value": value,
                "variant": "on",
                "reason": "STATIC"
            }))
            .unwrap();

            assert_eq!(response.flag_key, "example");
            assert_eq!(response.value, value);
            assert_eq!(response.variant, "on");
            assert_eq!(response.reason, EvaluationReason::STATIC);
        }
    }

    #[test]
    fn response_parses_json_number_shapes_from_wire_text() {
        let integer = parse_response_value("1");
        assert_eq!(integer.as_i64(), Some(1));
        assert_eq!(integer.to_string(), "1");

        let decimal = parse_response_value("1.0");
        assert_eq!(decimal.as_f64(), Some(1.0));
        assert_eq!(decimal.as_i64(), None);
        assert_eq!(decimal.to_string(), "1.0");

        let minimum_i64 = parse_response_value("-9223372036854775808");
        assert_eq!(minimum_i64.as_i64(), Some(i64::MIN));

        let below_minimum_i64 = parse_response_value("-9223372036854775809");
        assert_eq!(below_minimum_i64.as_i64(), None);
        assert_eq!(below_minimum_i64.as_u64(), None);
        assert_eq!(
            below_minimum_i64.as_f64(),
            Some(-9_223_372_036_854_775_808.0)
        );

        let maximum_u64 = parse_response_value("18446744073709551615");
        assert_eq!(maximum_u64.as_u64(), Some(u64::MAX));
        assert_eq!(maximum_u64.as_i64(), None);

        let two_to_the_sixty_four = parse_response_value("18446744073709551616");
        assert_eq!(two_to_the_sixty_four.as_u64(), None);
        assert_eq!(two_to_the_sixty_four.as_i64(), None);
        assert_eq!(
            two_to_the_sixty_four.as_f64(),
            Some(18_446_744_073_709_551_616.0)
        );
    }

    #[test]
    fn response_defaults_missing_optional_wire_fields() {
        let response: FlagshipEvaluationResponse = serde_json::from_value(json!({
            "flagKey": "example",
            "value": true
        }))
        .unwrap();

        assert_eq!(response.variant, "");
        assert_eq!(response.reason, EvaluationReason::DEFAULT);
    }

    #[test]
    fn response_defaults_null_variant_and_reason() {
        let response: FlagshipEvaluationResponse = serde_json::from_value(json!({
            "flagKey": "example",
            "value": null,
            "variant": null,
            "reason": null
        }))
        .unwrap();

        assert_eq!(response.value, Value::Null);
        assert_eq!(response.variant, "");
        assert_eq!(response.reason, EvaluationReason::DEFAULT);
    }

    #[test]
    fn response_ignores_unknown_fields() {
        let response: FlagshipEvaluationResponse = serde_json::from_value(json!({
            "flagKey": "example",
            "value": 1,
            "variant": "one",
            "reason": "SPLIT",
            "futureField": {"anything": true}
        }))
        .unwrap();

        assert_eq!(response.flag_key, "example");
        assert_eq!(response.reason, EvaluationReason::SPLIT);
    }

    #[test]
    fn response_preserves_unknown_reason() {
        let response: FlagshipEvaluationResponse = serde_json::from_value(json!({
            "flagKey": "example",
            "value": "value",
            "reason": "FUTURE_REASON"
        }))
        .unwrap();

        assert_eq!(response.reason.as_str(), "FUTURE_REASON");
    }

    #[test]
    fn response_rejects_missing_required_fields() {
        assert!(
            serde_json::from_value::<FlagshipEvaluationResponse>(json!({"value": true})).is_err()
        );
        assert!(
            serde_json::from_value::<FlagshipEvaluationResponse>(json!({"flagKey": "example"}))
                .is_err()
        );
    }

    #[test]
    fn response_rejects_null_flag_key() {
        let wire = json!({
            "flagKey": null,
            "value": true,
            "variant": null,
            "reason": null
        });

        assert!(serde_json::from_value::<FlagshipEvaluationResponse>(wire).is_err());
    }

    #[test]
    fn response_rejects_invalid_field_types() {
        for (field, invalid_value) in [
            ("flagKey", json!(7)),
            ("variant", json!(false)),
            ("reason", json!(["DEFAULT"])),
        ] {
            let mut wire = json!({
                "flagKey": "example",
                "value": true,
                "variant": "on",
                "reason": "DEFAULT"
            });
            wire[field] = invalid_value;

            assert!(serde_json::from_value::<FlagshipEvaluationResponse>(wire).is_err());
        }

        assert!(serde_json::from_value::<FlagshipEvaluationResponse>(json!([])).is_err());
    }

    #[test]
    fn response_serializes_with_camel_case_wire_names() {
        let response = FlagshipEvaluationResponse::new("example", json!({"enabled": true}))
            .with_variant("on")
            .with_reason(EvaluationReason::TARGETING_MATCH);

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "flagKey": "example",
                "value": {"enabled": true},
                "variant": "on",
                "reason": "TARGETING_MATCH"
            })
        );
    }

    #[test]
    fn response_builders_accept_strings_and_apply_defaults() {
        let default_response = FlagshipEvaluationResponse::new("example", json!(true));

        assert_eq!(default_response.flag_key, "example");
        assert_eq!(default_response.value, json!(true));
        assert_eq!(default_response.variant, "");
        assert_eq!(default_response.reason, EvaluationReason::DEFAULT);

        let configured = FlagshipEvaluationResponse::new(String::from("example"), json!(false))
            .with_variant(String::from("off"))
            .with_reason("FUTURE_REASON");

        assert_eq!(configured.variant, "off");
        assert_eq!(configured.reason.as_str(), "FUTURE_REASON");
    }

    #[test]
    fn response_round_trips_through_serde() {
        let response = FlagshipEvaluationResponse::new("example", json!([1, null, {"on": true}]))
            .with_variant("v2")
            .with_reason("FUTURE_REASON");
        let wire = serde_json::to_string(&response).unwrap();
        let decoded: FlagshipEvaluationResponse = serde_json::from_str(&wire).unwrap();

        assert_eq!(decoded, response);
    }
}
