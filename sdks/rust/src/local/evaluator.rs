//! Internal orchestration for local flag evaluation.
//!
//! Control flow is Go-first: tolerant field defaults, rule skipping and order,
//! missing-versus-null lookup, rollout decisions, and variation validation match
//! the Go SDK. Value coercion deliberately follows the ECMAScript/data-plane
//! helpers in [`super::coercion`]. Compared with the current Go evaluator this
//! means lowercase `infinity` is rejected, the full ECMAScript Unicode
//! whitespace set is trimmed, arbitrarily large radix integers are accepted and
//! rounded as JavaScript numbers, decimal overflow strings such as `"1e400"`
//! become positive or negative infinity instead of the current Go evaluator's
//! NaN, and structured collection members and malformed non-string
//! `serve_variation` values use JavaScript stringification.
//!
//! A definitions snapshot must retain flags as
//! `BTreeMap<String, Box<serde_json::value::RawValue>>`. Each requested flag is
//! parsed into a [`serde_json::Value`] and evaluated independently, and its parse
//! result or error is cached independently. One malformed flag therefore cannot
//! poison sibling flags or the snapshot. Malformed top-level flag values and
//! undefined variations become [`FlagConfigError`], which the local provider must
//! map to `PARSE_ERROR`. Rollout seeds use the definition's `key`, not the
//! requested lookup key. Context values are already-normalized
//! [`std::collections::BTreeMap`] entries; absence remains distinct from an
//! explicit JSON `null`.
//!
//! `cases.json` is packaged beside this module. The generator and CI still need
//! outside-file integration to copy `spec/local-evaluation/cases.json` here; the
//! source-tree drift test below enforces that contract when the spec is present,
//! and treats its absence as an error under `CI`.

use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap, error::Error, fmt};

use serde_json::{Map, Value};

use super::coercion::{
    CoercionCache, contains_primitive_value, contains_value, is_primitive, is_scalar, js_number,
    js_string,
};
use super::murmur::{rollout_bucket, rollout_seed};
use crate::types::EvaluationReason;

const DEFAULT_ROLLOUT_ATTRIBUTE: &str = "targetingKey";
const MAX_PATH_SEGMENTS: usize = 5;
const MAX_CONDITION_DEPTH: usize = 100;
const MAX_ERROR_FIELD_LENGTH: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EvaluationResult {
    pub(crate) value: Value,
    pub(crate) variant: String,
    pub(crate) reason: EvaluationReason,
}

/// An internally inconsistent flag definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FlagConfigError {
    kind: FlagConfigErrorKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FlagConfigErrorKind {
    MalformedDefinition,
    UndefinedVariation { flag_key: String, variant: String },
}

impl FlagConfigError {
    fn malformed_definition() -> Self {
        Self {
            kind: FlagConfigErrorKind::MalformedDefinition,
        }
    }

    fn undefined_variation(flag_key: &str, variant: &str) -> Self {
        Self {
            kind: FlagConfigErrorKind::UndefinedVariation {
                flag_key: bounded_error_field(flag_key),
                variant: bounded_error_field(variant),
            },
        }
    }
}

impl fmt::Display for FlagConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            FlagConfigErrorKind::MalformedDefinition => {
                formatter.write_str("flag definition must be a JSON object")
            }
            FlagConfigErrorKind::UndefinedVariation { flag_key, variant } => write!(
                formatter,
                "Flag '{flag_key}' variation '{variant}' is not defined"
            ),
        }
    }
}

impl Error for FlagConfigError {}

fn bounded_error_field(value: &str) -> String {
    const TRUNCATION_MARKER: &str = "…";

    let mut escaped = String::new();
    let mut sequence_lengths = Vec::new();
    for character in value.chars() {
        let sequence: String = character.escape_default().collect();
        if escaped.len() + sequence.len() > MAX_ERROR_FIELD_LENGTH {
            while escaped.len() + TRUNCATION_MARKER.len() > MAX_ERROR_FIELD_LENGTH {
                let Some(sequence_length) = sequence_lengths.pop() else {
                    return TRUNCATION_MARKER.to_owned();
                };
                escaped.truncate(escaped.len() - sequence_length);
            }
            escaped.push_str(TRUNCATION_MARKER);
            break;
        }
        sequence_lengths.push(sequence.len());
        escaped.push_str(&sequence);
    }

    escaped
}

/// Supplies a rollout bucket in the half-open production range `[0, 100)`.
///
/// Tests may inject a closure to exercise exact percentage boundaries.
pub(crate) trait BucketRandom {
    fn bucket(&mut self) -> f64;
}

impl<F> BucketRandom for F
where
    F: FnMut() -> f64,
{
    fn bucket(&mut self) -> f64 {
        self()
    }
}

struct ProductionBucketRandom;

impl BucketRandom for ProductionBucketRandom {
    fn bucket(&mut self) -> f64 {
        // rand's convenient thread-local API is infallible at the type level and
        // may panic if its OS-backed generator cannot initialize. Propagating
        // that exceptional process condition would require changing this API.
        rand::random::<f64>() * 100.0
    }
}

#[derive(Clone, Copy)]
enum ContextValue<'a> {
    Missing,
    Present(&'a Value),
}

#[derive(Clone, Copy)]
struct Rollout<'a> {
    percentage: f64,
    attribute: &'a str,
}

/// Evaluates one raw flag definition without applying provider-level defaults.
pub(crate) fn evaluate_flag(
    definition: &Value,
    context: &BTreeMap<String, Value>,
    account_id: &str,
) -> Result<EvaluationResult, FlagConfigError> {
    let mut random = ProductionBucketRandom;

    evaluate_flag_with_random(definition, context, account_id, &mut random)
}

/// Evaluates one raw flag definition with an injectable random bucket source.
pub(crate) fn evaluate_flag_with_random<R: BucketRandom + ?Sized>(
    definition: &Value,
    context: &BTreeMap<String, Value>,
    account_id: &str,
    random: &mut R,
) -> Result<EvaluationResult, FlagConfigError> {
    let Some(flag) = definition.as_object() else {
        return Err(FlagConfigError::malformed_definition());
    };
    let flag_key = string_field(flag, "key");
    let default_variation = string_field(flag, "default_variation");
    let variations = flag.get("variations").and_then(Value::as_object);
    let enabled = flag
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !enabled {
        return serve_variation(
            variations,
            flag_key,
            default_variation,
            EvaluationReason::DISABLED,
        );
    }
    let rules = flag.get("rules").and_then(Value::as_array);
    if !rules.is_some_and(|rules| rules.iter().any(Value::is_object)) {
        return serve_variation(
            variations,
            flag_key,
            default_variation,
            EvaluationReason::STATIC,
        );
    }

    let mut seed = None;
    let mut coercions = CoercionCache::default();
    for rule in rules.into_iter().flatten().filter_map(Value::as_object) {
        let conditions = rule
            .get("conditions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut matches = conditions
            .iter()
            .all(|condition| evaluate_condition_with_cache(condition, context, &mut coercions));
        let rollout = parse_rollout(rule.get("rollout"));
        let is_split = rollout.is_some_and(|rollout| rollout.percentage < 100.0);

        if matches && let Some(rollout) = rollout {
            matches = rollout_matches(rollout, context, account_id, flag_key, &mut seed, random);
        }

        if matches {
            let variant = variation_field(rule, "serve_variation");
            let reason = if is_split {
                EvaluationReason::SPLIT
            } else {
                EvaluationReason::TARGETING_MATCH
            };

            return serve_variation(variations, flag_key, variant.as_ref(), reason);
        }
    }

    serve_variation(
        variations,
        flag_key,
        default_variation,
        EvaluationReason::DEFAULT,
    )
}

fn string_field<'a>(object: &'a Map<String, Value>, field: &str) -> &'a str {
    object
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn variation_field<'a>(object: &'a Map<String, Value>, field: &str) -> Cow<'a, str> {
    let Some(value) = object.get(field) else {
        return Cow::Borrowed("");
    };

    value
        .as_str()
        .map_or_else(|| js_string(value), Cow::Borrowed)
}

fn serve_variation(
    variations: Option<&Map<String, Value>>,
    flag_key: &str,
    variant: &str,
    reason: EvaluationReason,
) -> Result<EvaluationResult, FlagConfigError> {
    let Some(value) = variations.and_then(|variations| variations.get(variant)) else {
        return Err(FlagConfigError::undefined_variation(flag_key, variant));
    };

    Ok(EvaluationResult {
        value: value.clone(),
        variant: variant.to_owned(),
        reason,
    })
}

fn parse_rollout(value: Option<&Value>) -> Option<Rollout<'_>> {
    let rollout = value?.as_object()?;
    let percentage = rollout.get("percentage").map_or(100.0, js_number);
    let attribute = string_field(rollout, "attribute");
    let attribute = if attribute.is_empty() {
        DEFAULT_ROLLOUT_ATTRIBUTE
    } else {
        attribute
    };

    Some(Rollout {
        percentage,
        attribute,
    })
}

fn rollout_matches<R: BucketRandom + ?Sized>(
    rollout: Rollout<'_>,
    context: &BTreeMap<String, Value>,
    account_id: &str,
    flag_key: &str,
    seed: &mut Option<u32>,
    random: &mut R,
) -> bool {
    let attribute = context_value(context, rollout.attribute);
    if matches!(attribute, ContextValue::Present(value) if !value.is_null() && !is_scalar(value)) {
        return false;
    }
    if rollout.percentage >= 100.0 || rollout.percentage.is_nan() {
        return true;
    }

    let seed = *seed.get_or_insert_with(|| rollout_seed(account_id, flag_key));
    let bucket = match attribute {
        ContextValue::Present(value) if is_scalar(value) => {
            rollout_bucket(js_string(value).as_ref(), seed)
        }
        ContextValue::Missing | ContextValue::Present(_) => random.bucket(),
    };

    bucket < rollout.percentage
}

fn context_value<'a>(context: &'a BTreeMap<String, Value>, attribute: &str) -> ContextValue<'a> {
    if let Some(value) = context.get(attribute) {
        return ContextValue::Present(value);
    }
    if !attribute.contains('.') {
        return ContextValue::Missing;
    }

    let mut segments = attribute.split('.');
    if segments.clone().count() > MAX_PATH_SEGMENTS {
        return ContextValue::Missing;
    }
    let Some(first) = segments.next() else {
        return ContextValue::Missing;
    };

    let Some(mut value) = context.get(first) else {
        return ContextValue::Missing;
    };
    for segment in segments {
        let Some(next) = value.as_object().and_then(|object| object.get(segment)) else {
            return ContextValue::Missing;
        };
        value = next;
    }

    ContextValue::Present(value)
}

#[cfg(test)]
fn evaluate_condition(condition: &Value, context: &BTreeMap<String, Value>) -> bool {
    let mut coercions = CoercionCache::default();

    evaluate_condition_with_cache(condition, context, &mut coercions)
}

fn evaluate_condition_with_cache(
    condition: &Value,
    context: &BTreeMap<String, Value>,
    coercions: &mut CoercionCache,
) -> bool {
    evaluate_condition_at_depth(condition, context, 0, coercions)
}

fn evaluate_condition_at_depth(
    condition: &Value,
    context: &BTreeMap<String, Value>,
    depth: usize,
    coercions: &mut CoercionCache,
) -> bool {
    if depth > MAX_CONDITION_DEPTH {
        return false;
    }

    let Some(condition) = condition.as_object() else {
        return false;
    };

    if let Some(logical_operator) = condition.get("logical_operator") {
        let clauses = condition
            .get("clauses")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();

        return match logical_operator.as_str() {
            Some("AND") => clauses
                .iter()
                .all(|clause| evaluate_condition_at_depth(clause, context, depth + 1, coercions)),
            Some("OR") => clauses
                .iter()
                .any(|clause| evaluate_condition_at_depth(clause, context, depth + 1, coercions)),
            _ => false,
        };
    }

    let attribute = string_field(condition, "attribute");
    let operator = string_field(condition, "operator");
    if attribute.is_empty() || operator.is_empty() {
        return false;
    }

    let ContextValue::Present(actual) = context_value(context, attribute) else {
        return false;
    };
    let target = condition.get("value").unwrap_or(&Value::Null);

    match operator {
        "equals" | "not_equals" | "contains" | "starts_with" | "ends_with" => {
            evaluate_string_operator(operator, actual, target)
        }
        "greater_than" | "less_than" | "greater_than_or_equals" | "less_than_or_equals" => {
            evaluate_ordering_operator(operator, actual, target, coercions)
        }
        "in" | "not_in" => evaluate_in_operator(operator, actual, target),
        "has" | "not_has" => evaluate_has_operator(operator, actual, target),
        _ => false,
    }
}

fn evaluate_string_operator(operator: &str, actual: &Value, target: &Value) -> bool {
    if actual.is_null() {
        return operator == "not_equals";
    }
    if !is_scalar(actual) || !is_primitive(target) {
        return false;
    }

    let actual = js_string(actual);
    let target = js_string(target);

    match operator {
        "equals" => actual == target,
        "not_equals" => actual != target,
        "contains" => actual.contains(target.as_ref()),
        "starts_with" => actual.starts_with(target.as_ref()),
        "ends_with" => actual.ends_with(target.as_ref()),
        _ => false,
    }
}

fn evaluate_ordering_operator(
    operator: &str,
    actual: &Value,
    target: &Value,
    coercions: &mut CoercionCache,
) -> bool {
    let Some(ordering) = coercions.compare_values(actual, target) else {
        return false;
    };

    match operator {
        "greater_than" => ordering == Ordering::Greater,
        "less_than" => ordering == Ordering::Less,
        "greater_than_or_equals" => ordering != Ordering::Less,
        "less_than_or_equals" => ordering != Ordering::Greater,
        _ => false,
    }
}

fn evaluate_in_operator(operator: &str, actual: &Value, target: &Value) -> bool {
    let Some(target) = target.as_array() else {
        return false;
    };
    if actual.is_null() {
        return operator == "not_in";
    }
    if !is_scalar(actual) {
        return false;
    }

    let contained = contains_value(target, actual);

    if operator == "in" {
        contained
    } else {
        !contained
    }
}

fn evaluate_has_operator(operator: &str, actual: &Value, target: &Value) -> bool {
    let Some(actual) = actual.as_array() else {
        return false;
    };
    if !is_primitive(target) {
        return false;
    }

    let contained = contains_primitive_value(actual, target);

    if operator == "has" {
        contained
    } else {
        !contained
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::BTreeMap, path::Path};

    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    const FIXTURE: &str = include_str!("cases.json");

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureCase {
        name: String,
        definitions: Value,
        account_id: String,
        flag_key: String,
        context: BTreeMap<String, Value>,
        expected: Value,
    }

    #[derive(Deserialize)]
    struct FixtureSuite {
        version: u64,
        #[serde(default)]
        cases: Vec<FixtureCase>,
        #[serde(default)]
        murmur: Vec<MurmurFixture>,
    }

    #[derive(Deserialize)]
    struct MurmurFixture {
        name: String,
        input: String,
        seed: u32,
        expected: u32,
    }

    fn fixture_suite() -> FixtureSuite {
        serde_json::from_str(FIXTURE).expect("packaged local-evaluation fixture must be valid")
    }

    fn fixture_flag(case: &FixtureCase) -> &Value {
        case.definitions
            .get("flags")
            .and_then(Value::as_object)
            .and_then(|flags| flags.get(&case.flag_key))
            .expect("fixture flag must exist")
    }

    fn flag_with_rule(rule: &Value) -> Value {
        json!({
            "key": "test",
            "enabled": true,
            "default_variation": "off",
            "variations": {"off": false, "on": true},
            "rules": [rule]
        })
    }

    fn context(value: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(value).expect("test context must be an object")
    }

    #[test]
    fn packaged_fixture_matches_source_spec_when_available() {
        let spec =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/local-evaluation/cases.json");
        let source = match std::fs::read(&spec) {
            Ok(source) => source,
            Err(error) if std::env::var_os("CI").is_none() => {
                eprintln!("skipping fixture drift check; {}: {error}", spec.display());
                return;
            }
            Err(error) => panic!("CI must provide {}: {error}", spec.display()),
        };

        assert_eq!(
            FIXTURE.as_bytes(),
            source,
            "regenerate src/local/cases.json"
        );
    }

    #[test]
    fn executes_all_shared_evaluation_cases() {
        let suite = fixture_suite();

        assert_eq!(suite.version, 1);
        assert_eq!(suite.cases.len(), 80);
        assert_eq!(suite.murmur.len(), 7, "hash corpus schema changed");
        for vector in &suite.murmur {
            assert_ne!(vector.name, "");
            let _ = (&vector.input, vector.seed, vector.expected);
        }

        for case in &suite.cases {
            if case.expected.get("reasonOneOf").is_some() {
                for (bucket, expected_value, expected_variant, expected_reason) in [
                    (0.0, json!(true), "on", EvaluationReason::SPLIT),
                    (99.999, json!(false), "off", EvaluationReason::DEFAULT),
                ] {
                    let calls = Cell::new(0);
                    let mut random = || {
                        calls.set(calls.get() + 1);
                        bucket
                    };
                    let result = evaluate_flag_with_random(
                        fixture_flag(case),
                        &case.context,
                        &case.account_id,
                        &mut random,
                    )
                    .unwrap_or_else(|error| panic!("{}: {error}", case.name));

                    assert_eq!(result.value, expected_value, "{}: value", case.name);
                    assert_eq!(result.variant, expected_variant, "{}: variant", case.name);
                    assert_eq!(result.reason, expected_reason, "{}: reason", case.name);
                    assert_eq!(calls.get(), 1, "{}: random calls", case.name);
                }
                continue;
            }

            let mut random = || panic!("{} must not use random bucketing", case.name);
            let result = evaluate_flag_with_random(
                fixture_flag(case),
                &case.context,
                &case.account_id,
                &mut random,
            );
            let expected = case
                .expected
                .as_object()
                .expect("fixture expected result must be an object");

            if expected.contains_key("error") {
                assert!(
                    matches!(result, Err(FlagConfigError { .. })),
                    "{}: expected config error, got {result:?}",
                    case.name
                );
                continue;
            }

            let result = result.unwrap_or_else(|error| panic!("{}: {error}", case.name));
            assert_eq!(result.value, expected["value"], "{}: value", case.name);
            assert_eq!(
                result.variant, expected["variant"],
                "{}: variant",
                case.name
            );
            assert_eq!(
                result.reason.as_str(),
                expected["reason"],
                "{}: reason",
                case.name
            );
        }
    }

    #[test]
    fn defaults_malformed_definition_fields_like_go() {
        let context = BTreeMap::new();
        let mut never = || panic!("RNG must not be called");
        let malformed = json!({
            "key": 7,
            "enabled": "yes",
            "default_variation": 9,
            "variations": {"": "fallback"},
            "rules": [null, "skip"]
        });

        let result =
            evaluate_flag_with_random(&malformed, &context, "account", &mut never).unwrap();

        assert_eq!(result.value, json!("fallback"));
        assert_eq!(result.variant, "");
        assert_eq!(result.reason, EvaluationReason::DISABLED);
    }

    #[test]
    fn malformed_rules_are_skipped_and_malformed_conditions_do_not_match() {
        let definition = json!({
            "key": "test",
            "enabled": true,
            "default_variation": "off",
            "variations": {"off": false, "on": true},
            "rules": [null, {"conditions": [null], "serve_variation": "on"}]
        });
        let mut never = || panic!("RNG must not be called");

        let result =
            evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut never)
                .unwrap();

        assert_eq!(result.reason, EvaluationReason::DEFAULT);
    }

    #[test]
    fn all_invalid_rules_make_an_enabled_flag_static() {
        let definition = json!({
            "key": "test",
            "enabled": true,
            "default_variation": "off",
            "variations": {"off": false},
            "rules": [null, 1, "bad", []]
        });
        let mut never = || panic!("static flag must not call RNG");

        let result =
            evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut never)
                .unwrap();

        assert_eq!(result.reason, EvaluationReason::STATIC);
    }

    #[test]
    fn scalar_and_array_definitions_are_clear_parse_errors() {
        let mut never = || panic!("malformed definitions must not call RNG");

        for definition in [json!(null), json!(1), json!("flag"), json!([])] {
            let error =
                evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut never)
                    .unwrap_err();

            assert_eq!(error.to_string(), "flag definition must be a JSON object");
        }
    }

    #[test]
    fn logical_defaults_match_go() {
        let cases = [
            (json!({"logical_operator": "AND"}), true),
            (json!({"logical_operator": "OR"}), false),
            (json!({"logical_operator": "AND", "clauses": "bad"}), true),
            (json!({"logical_operator": "X", "clauses": []}), false),
        ];

        for (condition, expected) in cases {
            assert_eq!(evaluate_condition(&condition, &BTreeMap::new()), expected);
        }
    }

    #[test]
    fn logical_condition_depth_is_capped_at_one_hundred() {
        let context = context(json!({"x": "yes"}));
        let leaf = json!({"attribute": "x", "operator": "equals", "value": "yes"});

        let mut at_limit = leaf.clone();
        for _ in 0..MAX_CONDITION_DEPTH {
            at_limit = json!({"logical_operator": "AND", "clauses": [at_limit]});
        }

        let beyond_limit = json!({"logical_operator": "AND", "clauses": [at_limit.clone()]});

        assert!(evaluate_condition(&at_limit, &context));
        assert!(!evaluate_condition(&beyond_limit, &context));
    }

    #[test]
    fn exact_keys_paths_and_depth_boundaries_are_preserved() {
        let context = context(json!({
            "a.b": "exact",
            "a": {"b": "nested", "c": {"d": {"e": {"f": "five", "g": "six"}}}}
        }));

        assert!(
            matches!(context_value(&context, "a.b"), ContextValue::Present(value) if value == "exact")
        );
        assert!(
            matches!(context_value(&context, "a.c.d.e.f"), ContextValue::Present(value) if value == "five")
        );
        assert!(matches!(
            context_value(&context, "a.c.d.e.f.g"),
            ContextValue::Missing
        ));
        assert!(matches!(
            context_value(&context, "a.c.missing"),
            ContextValue::Missing
        ));
    }

    #[test]
    fn dotted_lookup_rejects_arrays_but_preserves_explicit_null() {
        let context = context(json!({
            "array": [{"value": "hidden"}],
            "object": {"value": null}
        }));

        assert!(matches!(
            context_value(&context, "array.0.value"),
            ContextValue::Missing
        ));
        assert!(matches!(
            context_value(&context, "object.value"),
            ContextValue::Present(Value::Null)
        ));
    }

    #[test]
    fn unknown_and_malformed_operators_do_not_match() {
        let context = context(json!({"x": "secret"}));
        let conditions = [
            json!({"attribute": "x", "operator": "unknown", "value": "secret"}),
            json!({"attribute": 1, "operator": "equals", "value": "secret"}),
            json!({"attribute": "x", "operator": 1, "value": "secret"}),
            json!({"attribute": "missing", "operator": "not_equals", "value": "secret"}),
        ];

        for condition in conditions {
            assert!(!evaluate_condition(&condition, &context));
        }

        assert!(!evaluate_condition(
            &json!({
                "logical_operator": "and",
                "clauses": [
                    {"attribute": "x", "operator": "equals", "value": "secret"}
                ]
            }),
            &context
        ));
    }

    #[test]
    fn has_operators_handle_null_structured_and_malformed_targets() {
        let null_element = context(json!({"tags": [null]}));
        assert!(!evaluate_condition(
            &json!({"attribute": "tags", "operator": "has", "value": null}),
            &null_element
        ));
        assert!(evaluate_condition(
            &json!({"attribute": "tags", "operator": "not_has", "value": null}),
            &null_element
        ));

        let string_element = context(json!({"tags": ["null"]}));
        assert!(evaluate_condition(
            &json!({"attribute": "tags", "operator": "has", "value": null}),
            &string_element
        ));
        assert!(!evaluate_condition(
            &json!({"attribute": "tags", "operator": "not_has", "value": null}),
            &string_element
        ));

        for target in [json!({"nested": true}), json!(["nested"])] {
            for operator in ["has", "not_has"] {
                assert!(!evaluate_condition(
                    &json!({
                        "attribute": "tags",
                        "operator": operator,
                        "value": target.clone()
                    }),
                    &string_element
                ));
            }
        }

        assert!(!evaluate_condition(
            &json!({"attribute": "tags", "operator": "has"}),
            &null_element
        ));
        assert!(evaluate_condition(
            &json!({"attribute": "tags", "operator": "not_has"}),
            &null_element
        ));
    }

    #[test]
    fn nonscalar_in_behavior_uses_data_plane_stringification() {
        let scalar_context = context(json!({"x": "[object Object]"}));
        let object_context = context(json!({"x": {"private": true}}));
        let structured_target = json!({
            "attribute": "x",
            "operator": "in",
            "value": [{"different": "shape"}]
        });
        let scalar_target = json!({
            "attribute": "x",
            "operator": "in",
            "value": ["[object Object]"]
        });

        assert!(evaluate_condition(&structured_target, &scalar_context));
        assert!(!evaluate_condition(&scalar_target, &object_context));
    }

    #[test]
    fn evaluator_pins_ecmascript_numeric_coercion_differences() {
        let cases = [
            ("infinity", false),
            ("\u{2007}42\u{2007}", true),
            ("0x100000000000000000000000000000000", true),
        ];

        for (actual, expected) in cases {
            let context = context(json!({"x": actual}));
            let condition = json!({
                "attribute": "x",
                "operator": "greater_than",
                "value": 40
            });

            assert_eq!(
                evaluate_condition(&condition, &context),
                expected,
                "{actual}"
            );
        }
    }

    #[test]
    fn nonstring_serve_variations_use_canonical_javascript_keys() {
        let cases = [
            (json!(1.0), "1", json!("numeric")),
            (json!({"private": true}), "[object Object]", json!("object")),
            (json!([1, 2]), "1,2", json!("array")),
        ];

        for (serve_variation, expected_variant, expected_value) in cases {
            let definition = json!({
                "key": "test",
                "enabled": true,
                "default_variation": "off",
                "variations": {
                    "off": "default",
                    "1": "numeric",
                    "1.0": "wrong numeric key",
                    "[object Object]": "object",
                    "1,2": "array"
                },
                "rules": [{"conditions": [], "serve_variation": serve_variation}]
            });
            let mut never = || panic!("targeting rule must not call RNG");

            let result =
                evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut never)
                    .unwrap();

            assert_eq!(result.variant, expected_variant);
            assert_eq!(result.value, expected_value);
        }
    }

    #[test]
    fn rollout_boundaries_and_random_call_counts_match_go() {
        let cases = [
            (-1.0, 0.0, EvaluationReason::DEFAULT),
            (0.0, 0.0, EvaluationReason::DEFAULT),
            (0.5, 0.499, EvaluationReason::SPLIT),
            (0.5, 0.5, EvaluationReason::DEFAULT),
            (100.0, 100.0, EvaluationReason::TARGETING_MATCH),
            (101.0, 100.0, EvaluationReason::TARGETING_MATCH),
        ];

        for (percentage, bucket, expected_reason) in cases {
            let definition = flag_with_rule(&json!({
                "conditions": [],
                "serve_variation": "on",
                "rollout": {"percentage": percentage}
            }));
            let calls = Cell::new(0);
            let mut random = || {
                calls.set(calls.get() + 1);
                bucket
            };

            let result =
                evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut random)
                    .unwrap();

            assert_eq!(result.reason, expected_reason, "percentage {percentage}");
            assert_eq!(calls.get(), usize::from(percentage < 100.0));
        }
    }

    #[test]
    fn missing_and_null_rollout_attributes_each_call_random_once() {
        let definition = flag_with_rule(&json!({
            "conditions": [],
            "serve_variation": "on",
            "rollout": {"percentage": 50}
        }));

        for context in [BTreeMap::new(), context(json!({"targetingKey": null}))] {
            let calls = Cell::new(0);
            let mut random = || {
                calls.set(calls.get() + 1);
                50.0
            };

            let result =
                evaluate_flag_with_random(&definition, &context, "account", &mut random).unwrap();

            assert_eq!(result.reason, EvaluationReason::DEFAULT);
            assert_eq!(result.value, json!(false));
            assert_eq!(result.variant, "off");
            assert_eq!(calls.get(), 1);
        }
    }

    #[test]
    fn null_percentage_is_nan_equivalent_and_does_not_call_rng() {
        let definition = flag_with_rule(&json!({
            "conditions": [],
            "serve_variation": "on",
            "rollout": {"percentage": null}
        }));
        let mut never = || panic!("NaN-equivalent percentage is not a split");

        let result =
            evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut never)
                .unwrap();

        assert_eq!(result.reason, EvaluationReason::TARGETING_MATCH);
    }

    #[test]
    fn scalar_rollouts_are_deterministic_and_never_call_rng() {
        let definition = flag_with_rule(&json!({
            "conditions": [],
            "serve_variation": "on",
            "rollout": {"percentage": 50, "attribute": "id"}
        }));
        let context = context(json!({"id": "0"}));
        let mut never = || panic!("scalar rollout must use Murmur hashing");

        let result = evaluate_flag_with_random(
            &definition,
            &context,
            "aaaabbbbccccdddd1111222233334444",
            &mut never,
        )
        .unwrap();

        assert_eq!(result.reason, EvaluationReason::SPLIT);
    }

    #[test]
    fn scalar_rollout_hashes_canonical_ecmascript_strings() {
        let account_id = "aaaabbbbccccdddd1111222233334444";
        let flag_key = "scalar_string_regression";
        let seed = rollout_seed(account_id, flag_key);
        let cases = [
            (json!(1), "1"),
            (json!(-0.0), "0"),
            (json!(1e21), "1e+21"),
            (json!(1e-7), "1e-7"),
            (json!(9_007_199_254_740_991_u64), "9007199254740991"),
            (json!(true), "true"),
        ];

        for (attribute, canonical) in cases {
            assert_eq!(js_string(&attribute), canonical);

            let threshold = rollout_bucket(canonical, seed);
            let next_threshold = f64::from_bits(threshold.to_bits() + 1);
            let context = context(json!({"id": attribute}));

            for (percentage, expected_reason) in [
                (threshold, EvaluationReason::DEFAULT),
                (next_threshold, EvaluationReason::SPLIT),
            ] {
                let definition = json!({
                    "key": flag_key,
                    "enabled": true,
                    "default_variation": "off",
                    "variations": {"off": false, "on": true},
                    "rules": [{
                        "conditions": [],
                        "serve_variation": "on",
                        "rollout": {"percentage": percentage, "attribute": "id"}
                    }]
                });
                let mut never = || panic!("scalar rollout must not call RNG");

                let result =
                    evaluate_flag_with_random(&definition, &context, account_id, &mut never)
                        .unwrap();

                assert_eq!(result.reason, expected_reason, "attribute {canonical}");
            }
        }
    }

    #[test]
    fn nonscalar_rollout_attributes_reject_even_full_rollouts() {
        let definition = flag_with_rule(&json!({
            "conditions": [],
            "serve_variation": "on",
            "rollout": {"percentage": 100, "attribute": "id"}
        }));
        let context = context(json!({"id": ["not", "scalar"]}));
        let mut never = || panic!("full rollout must not call RNG");

        let result =
            evaluate_flag_with_random(&definition, &context, "account", &mut never).unwrap();

        assert_eq!(result.reason, EvaluationReason::DEFAULT);
    }

    #[test]
    fn config_errors_are_cloneable_and_do_not_leak_context_values() {
        let definition = flag_with_rule(&json!({
            "conditions": [{"attribute": "token", "operator": "equals", "value": "private"}],
            "serve_variation": "missing"
        }));
        let context = context(json!({"token": "private"}));
        let mut never = || panic!("RNG must not be called");

        let error =
            evaluate_flag_with_random(&definition, &context, "account", &mut never).unwrap_err();
        let display = error.to_string();

        assert_eq!(error, error.clone());
        assert!(display.contains("test"));
        assert!(display.contains("missing"));
        assert!(!display.contains("private"));
    }

    #[test]
    fn config_error_fields_are_escaped_and_bounded() {
        let flag_key = format!("key\n\0'{}", "k".repeat(1_000));
        let variant = format!("variant\r{}", "v".repeat(1_000));
        let definition = json!({
            "key": flag_key,
            "enabled": true,
            "default_variation": variant,
            "variations": {},
            "rules": []
        });
        let mut never = || panic!("static error must not call RNG");

        let display =
            evaluate_flag_with_random(&definition, &BTreeMap::new(), "account", &mut never)
                .unwrap_err()
                .to_string();

        assert!(display.len() <= MAX_ERROR_FIELD_LENGTH * 2 + 64);
        assert!(display.contains("\\n"));
        assert!(display.contains("\\u{0}"));
        assert!(display.contains("\\'"));
        assert!(display.contains("\\r"));
        assert!(!display.contains('\n'));
        assert!(!display.contains('\r'));
        assert!(!display.contains('\0'));
        assert!(display.matches('…').count() >= 2);
    }

    #[test]
    fn error_fields_keep_all_content_that_fits_the_limit() {
        let value = "x".repeat(MAX_ERROR_FIELD_LENGTH);

        assert_eq!(bounded_error_field(&value), value);
        assert_eq!(
            bounded_error_field(&format!("{value}x")),
            format!("{}…", "x".repeat(MAX_ERROR_FIELD_LENGTH - "…".len()))
        );
    }

    #[test]
    fn production_random_buckets_stay_in_range() {
        let mut random = ProductionBucketRandom;

        for _ in 0..10_000 {
            let bucket = random.bucket();

            assert!((0.0..100.0).contains(&bucket), "bucket {bucket}");
        }
    }

    #[test]
    fn production_entrypoint_evaluates_without_random_for_static_flags() {
        let definition = json!({
            "key": "static",
            "enabled": true,
            "default_variation": "off",
            "variations": {"off": false},
            "rules": []
        });

        let result = evaluate_flag(&definition, &BTreeMap::new(), "account").unwrap();

        assert_eq!(result.reason, EvaluationReason::STATIC);
    }
}
