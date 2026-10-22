//! Evaluation-context conversion.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::hash::Hash;
use std::io;
use std::sync::Arc;

use open_feature::{
    EvaluationContext, EvaluationContextFieldValue, StructValue, Value as OpenFeatureValue,
};
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};

use crate::error::{ErrorCode, FlagshipError};

const MAX_CONTEXT_DEPTH: usize = 100;
const MAX_CONTEXT_JSON_BYTES: usize = 1024 * 1024;
const TARGETING_KEY: &str = "targetingKey";
const FLAG_KEY: &str = "flagKey";
const MAX_ESCAPED_PATH_LENGTH: usize = 160;

enum ContextPath<'a> {
    Root(&'a str),
    Field { parent: &'a Self, key: &'a str },
    Index { parent: &'a Self, index: usize },
}

/// The flag value type expected by the caller.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ExpectedFlagType {
    Bool,
    String,
    Int,
    Float,
    Object,
}

impl ExpectedFlagType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::String => "string",
            Self::Int => "int",
            Self::Float => "float",
            Self::Object => "object",
        }
    }
}

/// An owned, deterministic representation of an `OpenFeature` evaluation context.
///
/// Objects are recursively sorted by key. Opaque `Struct` fields are accepted only
/// when their concrete type is [`StructValue`], [`OpenFeatureValue`], or
/// [`JsonValue`]. Other `Any` values are rejected deterministically.
///
/// The canonical JSON is stable SDK serialization, not RFC 8785 JSON Canonicalization.
/// A custom `flagKey` remains context data in JSON POST bodies and canonical context,
/// but [`Self::query_pairs`] omits it so the separately requested flag always wins on
/// GET. `f32` context values have already been widened to `f64` by `open-feature`, so
/// their query text can expose the exact widened value rather than Go's original
/// 32-bit formatting. The serialized context is limited to 1 MiB.
#[derive(Clone, PartialEq)]
pub(crate) struct NormalizedContext {
    values: BTreeMap<String, JsonValue>,
}

impl fmt::Debug for NormalizedContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedContext")
            .field("attribute_count", &self.values.len())
            .field("requires_post", &self.requires_post())
            .finish_non_exhaustive()
    }
}

impl NormalizedContext {
    /// Returns the normalized context values.
    pub(crate) const fn values(&self) -> &BTreeMap<String, JsonValue> {
        &self.values
    }

    /// Returns whether null, array, or object data requires a JSON POST body.
    pub(crate) fn requires_post(&self) -> bool {
        self.values.values().any(is_structured)
    }

    /// Produces sorted query pairs and makes the requested flag key authoritative.
    pub(crate) fn query_pairs(
        &self,
        flag_key: &str,
    ) -> Result<Vec<(String, String)>, FlagshipError> {
        if self.requires_post() {
            return Err(invalid_context(
                None,
                "structured evaluation context requires a JSON request body",
            ));
        }

        let mut pairs = Vec::with_capacity(self.values.len() + 1);

        for (key, value) in &self.values {
            if key != FLAG_KEY {
                let path = ContextPath::Root(key);
                pairs.push((key.clone(), primitive_query_value(value, &path)?));
            }
        }

        pairs.push((FLAG_KEY.to_owned(), flag_key.to_owned()));
        pairs.sort_by(|left, right| left.0.cmp(&right.0));

        Ok(pairs)
    }

    /// Serializes with recursively stable key ordering; this is not RFC 8785.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the forthcoming provider integration")
    )]
    pub(crate) fn canonical_json(&self) -> Result<String, FlagshipError> {
        serialize_json(&self.values)
    }

    /// Builds a cache key separating requested flag, expected type, and context.
    pub(crate) fn cache_key(
        &self,
        flag_key: &str,
        expected_type: ExpectedFlagType,
    ) -> Result<String, FlagshipError> {
        serialize_json(&(flag_key, expected_type.as_str(), &self.values))
    }
}

/// Converts an `OpenFeature` context into an owned deterministic JSON model.
///
/// A nonempty dedicated targeting key is authoritative. In that case a custom
/// `targetingKey` field is skipped before validation, so even an invalid or
/// structured colliding value cannot change the request method or fail conversion.
/// Nesting depth starts at zero for each context attribute: values at depth 100
/// are accepted and values at depth 101 are rejected.
///
/// Datetimes use RFC 3339 with Go-compatible nanosecond trimming. Years must be
/// in `0000..=9999`, offsets must be minute-aligned and less than 24 hours, and
/// UTC is emitted as `Z`. This is intentionally stricter than `time::OffsetDateTime`.
pub(crate) fn normalize_context(
    context: &EvaluationContext,
) -> Result<NormalizedContext, FlagshipError> {
    let mut values = BTreeMap::new();
    let targeting_key = context
        .targeting_key
        .as_deref()
        .filter(|key| !key.is_empty());

    if let Some(targeting_key) = targeting_key {
        values.insert(
            TARGETING_KEY.to_owned(),
            JsonValue::String(targeting_key.to_owned()),
        );
    }

    let mut fields: Vec<_> = context.custom_fields.iter().collect();
    fields.sort_by(|left, right| left.0.cmp(right.0));

    for (key, value) in fields {
        if targeting_key.is_some() && key == TARGETING_KEY {
            continue;
        }

        let path = ContextPath::Root(key);
        let normalized = normalize_field_value(value, &path, 0)?;
        values.insert(key.clone(), normalized);
    }

    validate_context_size(&values)?;

    Ok(NormalizedContext { values })
}

fn normalize_field_value(
    value: &EvaluationContextFieldValue,
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    check_depth(path, depth)?;

    match value {
        EvaluationContextFieldValue::Bool(value) => Ok(JsonValue::Bool(*value)),
        EvaluationContextFieldValue::Int(value) => Ok(JsonValue::Number(JsonNumber::from(*value))),
        EvaluationContextFieldValue::Float(value) => {
            let number = finite_json_number(*value, path)?;

            Ok(JsonValue::Number(number))
        }
        EvaluationContextFieldValue::String(value) => Ok(JsonValue::String(value.clone())),
        EvaluationContextFieldValue::DateTime(value) => {
            let formatted = format_datetime(
                value.year(),
                u8::from(value.month()),
                value.day(),
                value.hour(),
                value.minute(),
                value.second(),
                value.nanosecond(),
                value.offset().whole_seconds(),
                path,
            )?;

            Ok(JsonValue::String(formatted))
        }
        EvaluationContextFieldValue::Struct(value) => normalize_opaque_struct(value, path, depth),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit datetime components keep formatting independent of the upstream time type"
)]
fn format_datetime(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanosecond: u32,
    offset_seconds: i32,
    path: &ContextPath<'_>,
) -> Result<String, FlagshipError> {
    if !(0..=9999).contains(&year) || offset_seconds % 60 != 0 {
        return Err(invalid_context(
            Some(path),
            "contains a datetime outside RFC 3339's representable range",
        ));
    }

    let offset_minutes = offset_seconds / 60;
    if !(-1439..=1439).contains(&offset_minutes) {
        return Err(invalid_context(
            Some(path),
            "contains a datetime outside RFC 3339's representable range",
        ));
    }

    let mut formatted = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");

    if nanosecond != 0 {
        let fraction = format!("{nanosecond:09}");
        formatted.push('.');
        formatted.push_str(fraction.trim_end_matches('0'));
    }

    if offset_minutes == 0 {
        formatted.push('Z');
    } else {
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let absolute_minutes = offset_minutes.unsigned_abs();
        write!(
            formatted,
            "{sign}{:02}:{:02}",
            absolute_minutes / 60,
            absolute_minutes % 60
        )
        .map_err(|_| invalid_context(Some(path), "could not format its datetime"))?;
    }

    Ok(formatted)
}

fn normalize_opaque_struct(
    value: &Arc<dyn std::any::Any + Send + Sync>,
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    if let Some(value) = value.downcast_ref::<StructValue>() {
        let normalized = normalize_struct_value(value, path, depth)?;

        return Ok(normalized);
    }

    if let Some(value) = value.downcast_ref::<OpenFeatureValue>() {
        let normalized = normalize_open_feature_value(value, path, depth)?;

        return Ok(normalized);
    }

    if let Some(value) = value.downcast_ref::<JsonValue>() {
        let normalized = normalize_json_value(value, path, depth)?;

        return Ok(normalized);
    }

    Err(invalid_context(
        Some(path),
        "has an unsupported opaque structured type",
    ))
}

fn normalize_struct_value(
    value: &StructValue,
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    let mut fields: Vec<_> = value.fields.iter().collect();
    fields.sort_by(|left, right| left.0.cmp(right.0));

    let mut object = JsonMap::new();

    for (key, value) in fields {
        let child_path = ContextPath::Field { parent: path, key };
        let normalized = normalize_open_feature_value(value, &child_path, depth + 1)?;
        object.insert(key.clone(), normalized);
    }

    Ok(JsonValue::Object(object))
}

fn normalize_open_feature_value(
    value: &OpenFeatureValue,
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    check_depth(path, depth)?;

    match value {
        OpenFeatureValue::Bool(value) => Ok(JsonValue::Bool(*value)),
        OpenFeatureValue::Int(value) => Ok(JsonValue::Number(JsonNumber::from(*value))),
        OpenFeatureValue::Float(value) => {
            let number = finite_json_number(*value, path)?;

            Ok(JsonValue::Number(number))
        }
        OpenFeatureValue::String(value) => Ok(JsonValue::String(value.clone())),
        OpenFeatureValue::Array(values) => normalize_open_feature_array(values, path, depth),
        OpenFeatureValue::Struct(value) => normalize_struct_value(value, path, depth),
    }
}

fn normalize_open_feature_array(
    values: &[OpenFeatureValue],
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    let mut array = Vec::with_capacity(values.len());

    for (index, value) in values.iter().enumerate() {
        let child_path = ContextPath::Index {
            parent: path,
            index,
        };
        array.push(normalize_open_feature_value(value, &child_path, depth + 1)?);
    }

    Ok(JsonValue::Array(array))
}

fn normalize_json_value(
    value: &JsonValue,
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    check_depth(path, depth)?;

    match value {
        JsonValue::Null => Ok(JsonValue::Null),
        JsonValue::Bool(value) => Ok(JsonValue::Bool(*value)),
        JsonValue::Number(value) => normalize_json_number(value, path),
        JsonValue::String(value) => Ok(JsonValue::String(value.clone())),
        JsonValue::Array(values) => normalize_json_array(values, path, depth),
        JsonValue::Object(values) => normalize_json_object(values, path, depth),
    }
}

fn normalize_json_number(
    value: &JsonNumber,
    path: &ContextPath<'_>,
) -> Result<JsonValue, FlagshipError> {
    if let Some(value) = value.as_i64() {
        return Ok(JsonValue::Number(JsonNumber::from(value)));
    }

    if let Some(value) = value.as_u64() {
        return Ok(JsonValue::Number(JsonNumber::from(value)));
    }

    if value.is_f64()
        && let Some(value) = value.as_f64()
    {
        let number = finite_json_number(value, path)?;

        return Ok(JsonValue::Number(number));
    }

    Err(invalid_context(
        Some(path),
        "contains a JSON number outside the supported numeric range",
    ))
}

fn normalize_json_array(
    values: &[JsonValue],
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    let mut array = Vec::with_capacity(values.len());

    for (index, value) in values.iter().enumerate() {
        let child_path = ContextPath::Index {
            parent: path,
            index,
        };
        array.push(normalize_json_value(value, &child_path, depth + 1)?);
    }

    Ok(JsonValue::Array(array))
}

fn normalize_json_object(
    values: &JsonMap<String, JsonValue>,
    path: &ContextPath<'_>,
    depth: usize,
) -> Result<JsonValue, FlagshipError> {
    let mut fields: Vec<_> = values.iter().collect();
    fields.sort_by(|left, right| left.0.cmp(right.0));

    let mut object = JsonMap::new();

    for (key, value) in fields {
        let child_path = ContextPath::Field { parent: path, key };
        object.insert(
            key.clone(),
            normalize_json_value(value, &child_path, depth + 1)?,
        );
    }

    Ok(JsonValue::Object(object))
}

fn finite_json_number(value: f64, path: &ContextPath<'_>) -> Result<JsonNumber, FlagshipError> {
    JsonNumber::from_f64(value)
        .ok_or_else(|| invalid_context(Some(path), "contains a non-finite number"))
}

fn is_structured(value: &JsonValue) -> bool {
    matches!(
        value,
        JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_)
    )
}

fn primitive_query_value(
    value: &JsonValue,
    path: &ContextPath<'_>,
) -> Result<String, FlagshipError> {
    match value {
        JsonValue::Bool(value) => Ok(value.to_string()),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(value.to_string())
            } else if let Some(value) = value.as_u64() {
                Ok(value.to_string())
            } else if value.is_f64()
                && let Some(value) = value.as_f64()
                && value.is_finite()
            {
                Ok(value.to_string())
            } else {
                Err(invalid_context(
                    Some(path),
                    "contains a JSON number outside the supported numeric range",
                ))
            }
        }
        JsonValue::String(value) => Ok(value.clone()),
        JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_) => {
            Err(invalid_context(Some(path), "requires a JSON request body"))
        }
    }
}

fn serialize_json(value: &impl serde::Serialize) -> Result<String, FlagshipError> {
    serde_json::to_string(value).map_err(|source| {
        FlagshipError::new(
            ErrorCode::InvalidContext,
            "failed to serialize normalized evaluation context",
        )
        .with_source(source)
    })
}

fn validate_context_size(values: &BTreeMap<String, JsonValue>) -> Result<(), FlagshipError> {
    let mut writer = ContextSizeWriter::default();

    match serde_json::to_writer(&mut writer, values) {
        Ok(()) => Ok(()),
        Err(_) if writer.exceeded => Err(invalid_context(
            None,
            "exceeds the maximum serialized size of 1 MiB",
        )),
        Err(source) => Err(FlagshipError::new(
            ErrorCode::InvalidContext,
            "failed to serialize normalized evaluation context",
        )
        .with_source(source)),
    }
}

#[derive(Default)]
struct ContextSizeWriter {
    bytes: usize,
    exceeded: bool,
}

impl io::Write for ContextSizeWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > MAX_CONTEXT_JSON_BYTES.saturating_sub(self.bytes) {
            self.bytes = MAX_CONTEXT_JSON_BYTES;
            self.exceeded = true;

            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "serialized context exceeds its size limit",
            ));
        }

        self.bytes += buffer.len();

        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn check_depth(path: &ContextPath<'_>, depth: usize) -> Result<(), FlagshipError> {
    if depth > MAX_CONTEXT_DEPTH {
        return Err(invalid_context(Some(path), "exceeds maximum nesting depth"));
    }

    Ok(())
}

fn invalid_context(path: Option<&ContextPath<'_>>, problem: &str) -> FlagshipError {
    let message = path.map_or_else(
        || problem.to_owned(),
        |path| format!("context attribute {} {problem}", escaped_path(path)),
    );

    FlagshipError::new(ErrorCode::InvalidContext, message)
}

fn escaped_path(path: &ContextPath<'_>) -> String {
    let mut content = String::with_capacity(MAX_ESCAPED_PATH_LENGTH);
    let mut truncated = false;
    append_escaped_path(path, &mut content, &mut truncated);

    let mut escaped = String::with_capacity(MAX_ESCAPED_PATH_LENGTH + 5);
    escaped.push('"');
    escaped.push_str(&content);

    if truncated {
        escaped.push('…');
    }
    escaped.push('"');

    escaped
}

fn append_escaped_path(path: &ContextPath<'_>, output: &mut String, truncated: &mut bool) {
    if *truncated {
        return;
    }

    match path {
        ContextPath::Root(key) => append_escaped_text(key, output, truncated),
        ContextPath::Field { parent, key } => {
            append_escaped_path(parent, output, truncated);
            append_complete_sequence(".", output, truncated);
            append_escaped_text(key, output, truncated);
        }
        ContextPath::Index { parent, index } => {
            append_escaped_path(parent, output, truncated);
            append_complete_sequence(&format!("[{index}]"), output, truncated);
        }
    }
}

fn append_escaped_text(value: &str, output: &mut String, truncated: &mut bool) {
    for character in value.chars() {
        let sequence = if is_bidi_control(character) {
            character.escape_unicode().to_string()
        } else {
            character.escape_debug().to_string()
        };

        append_complete_sequence(&sequence, output, truncated);
        if *truncated {
            return;
        }
    }
}

fn append_complete_sequence(sequence: &str, output: &mut String, truncated: &mut bool) {
    if *truncated {
        return;
    }

    if output.len() + sequence.len() > MAX_ESCAPED_PATH_LENGTH {
        *truncated = true;
        return;
    }

    output.push_str(sequence);
}

const fn is_bidi_control(character: char) -> bool {
    // Bidi controls are printable, but emitting them verbatim could visually reorder diagnostics.
    matches!(
        character,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use open_feature::{EvaluationContextFieldValue, StructValue, Value as OpenFeatureValue};
    use serde_json::{Value as JsonValue, json};

    use super::*;

    #[derive(Debug)]
    struct Unsupported {
        secret: &'static str,
    }

    #[test]
    fn empty_and_targeting_key_contexts() {
        let empty = normalize_context(&EvaluationContext::default()).unwrap();
        assert!(empty.values().is_empty());
        assert!(!empty.requires_post());

        let blank =
            normalize_context(&EvaluationContext::default().with_targeting_key("")).unwrap();
        assert!(blank.values().is_empty());

        let targeted =
            normalize_context(&EvaluationContext::default().with_targeting_key("user-1")).unwrap();
        assert_eq!(targeted.values()[TARGETING_KEY], json!("user-1"));
    }

    #[test]
    fn primitives_have_stable_go_style_query_values() {
        let context = EvaluationContext::default()
            .with_custom_field("string", "alice")
            .with_custom_field("integer", -8_i64)
            .with_custom_field("float", 1.5)
            .with_custom_field("whole_float", 1.0)
            .with_custom_field("widened_f32", 1.2_f32)
            .with_custom_field("enabled", true);
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(
            normalized.query_pairs("dark-mode").unwrap(),
            vec![
                ("enabled".to_owned(), "true".to_owned()),
                ("flagKey".to_owned(), "dark-mode".to_owned()),
                ("float".to_owned(), "1.5".to_owned()),
                ("integer".to_owned(), "-8".to_owned()),
                ("string".to_owned(), "alice".to_owned()),
                ("whole_float".to_owned(), "1".to_owned()),
                ("widened_f32".to_owned(), f64::from(1.2_f32).to_string()),
            ]
        );
    }

    #[test]
    fn primitive_query_edges_are_explicit_and_lossless() {
        let context = EvaluationContext::default()
            .with_custom_field("empty", "")
            .with_custom_field("i64_min", i64::MIN)
            .with_custom_field("u64_max", Arc::new(json!(u64::MAX)))
            .with_custom_field("negative_zero", -0.0_f64)
            .with_custom_field("false", false);
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(
            normalized.query_pairs("flag").unwrap(),
            vec![
                ("empty".to_owned(), String::new()),
                ("false".to_owned(), "false".to_owned()),
                ("flagKey".to_owned(), "flag".to_owned()),
                ("i64_min".to_owned(), i64::MIN.to_string()),
                ("negative_zero".to_owned(), "-0".to_owned()),
                ("u64_max".to_owned(), u64::MAX.to_string()),
            ]
        );
    }

    #[test]
    fn json_numbers_are_rebuilt_into_supported_numeric_kinds() {
        let context = EvaluationContext::default()
            .with_custom_field("signed", Arc::new(json!(-1)))
            .with_custom_field("unsigned", Arc::new(json!(u64::MAX)))
            .with_custom_field("float", Arc::new(json!(1.25)));
        let normalized = normalize_context(&context).unwrap();

        assert!(normalized.values()["signed"].as_i64().is_some());
        assert!(normalized.values()["unsigned"].as_u64().is_some());
        assert!(normalized.values()["float"].as_f64().is_some());

        // arbitrary_precision preserves this as an integer instead of coercing it to f64.
        if let Ok(value) =
            serde_json::from_str::<JsonValue>("12345678901234567890123456789012345678901234567890")
        {
            let is_arbitrary_precision = value
                .as_number()
                .is_some_and(|number| !number.is_i64() && !number.is_u64() && !number.is_f64());
            let context = EvaluationContext::default().with_custom_field("huge", Arc::new(value));

            if is_arbitrary_precision {
                let error = normalize_context(&context).unwrap_err();
                assert_eq!(error.code(), &ErrorCode::InvalidContext);
                assert_eq!(
                    error.message(),
                    "context attribute \"huge\" contains a JSON number outside the supported numeric range"
                );
            } else {
                assert!(normalize_context(&context).is_ok());
            }
        }
    }

    #[test]
    fn nested_values_and_null_require_post() {
        let context = EvaluationContext::default()
            .with_custom_field("profile", Arc::new(json!({"plan": "pro"})))
            .with_custom_field("missing", Arc::new(JsonValue::Null));
        let normalized = normalize_context(&context).unwrap();

        assert!(normalized.requires_post());
        assert_eq!(normalized.values()["missing"], JsonValue::Null);
        assert_eq!(normalized.values()["profile"], json!({"plan": "pro"}));
        assert_eq!(
            normalized.query_pairs("flag").unwrap_err().code(),
            &ErrorCode::InvalidContext
        );
    }

    #[test]
    fn concrete_struct_primitives_stay_query_compatible() {
        let context = EvaluationContext::default()
            .with_custom_field("json", Arc::new(json!("value")))
            .with_custom_field("open_feature", Arc::new(OpenFeatureValue::Int(42)));
        let normalized = normalize_context(&context).unwrap();

        assert!(!normalized.requires_post());
        assert_eq!(
            normalized.query_pairs("flag").unwrap(),
            vec![
                ("flagKey".to_owned(), "flag".to_owned()),
                ("json".to_owned(), "value".to_owned()),
                ("open_feature".to_owned(), "42".to_owned()),
            ]
        );
    }

    #[test]
    fn empty_structures_and_arrays_force_post() {
        let context = EvaluationContext::default()
            .with_custom_field("struct_value", Arc::new(StructValue::default()))
            .with_custom_field(
                "open_feature_struct",
                Arc::new(OpenFeatureValue::Struct(StructValue::default())),
            )
            .with_custom_field(
                "open_feature_array",
                Arc::new(OpenFeatureValue::Array(Vec::new())),
            )
            .with_custom_field("json_object", Arc::new(json!({})));
        let normalized = normalize_context(&context).unwrap();

        assert!(normalized.requires_post());
        assert!(normalized.query_pairs("flag").is_err());
    }

    #[test]
    fn arrays_preserve_order() {
        let structure = StructValue::default().with_field(
            "items",
            OpenFeatureValue::Array(vec!["first".into(), 2_i64.into(), true.into()]),
        );
        let context = EvaluationContext::default().with_custom_field("data", Arc::new(structure));
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(
            normalized.values()["data"],
            json!({"items": ["first", 2, true]})
        );
    }

    #[test]
    fn canonical_serialization_ignores_source_key_order() {
        let first = StructValue::default()
            .with_field(
                "z",
                StructValue::default().with_field("b", 2).with_field("a", 1),
            )
            .with_field("a", true);
        let second = StructValue::default().with_field("a", true).with_field(
            "z",
            StructValue::default().with_field("a", 1).with_field("b", 2),
        );

        let first = normalize_context(
            &EvaluationContext::default().with_custom_field("object", Arc::new(first)),
        )
        .unwrap();
        let second = normalize_context(
            &EvaluationContext::default().with_custom_field("object", Arc::new(second)),
        )
        .unwrap();

        assert_eq!(
            first.canonical_json().unwrap(),
            second.canonical_json().unwrap()
        );
        assert_eq!(
            first.canonical_json().unwrap(),
            r#"{"object":{"a":true,"z":{"a":1,"b":2}}}"#
        );
        assert_eq!(
            first.cache_key("flag", ExpectedFlagType::Bool).unwrap(),
            second.cache_key("flag", ExpectedFlagType::Bool).unwrap()
        );
        assert_eq!(
            first.cache_key("flag", ExpectedFlagType::Bool).unwrap(),
            r#"["flag","bool",{"object":{"a":true,"z":{"a":1,"b":2}}}]"#
        );
        assert_ne!(
            first.cache_key("flag-a", ExpectedFlagType::Bool).unwrap(),
            first.cache_key("flag-b", ExpectedFlagType::Bool).unwrap()
        );
        assert_ne!(
            first.cache_key("flag", ExpectedFlagType::Bool).unwrap(),
            first.cache_key("flag", ExpectedFlagType::String).unwrap()
        );
    }

    #[test]
    fn dedicated_targeting_key_and_requested_flag_win_control_key_collisions() {
        let context = EvaluationContext::default()
            .with_targeting_key("original")
            .with_custom_field(TARGETING_KEY, Arc::new(JsonValue::Null))
            .with_custom_field(FLAG_KEY, "context-flag");
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(normalized.values()[TARGETING_KEY], json!("original"));
        assert_eq!(normalized.values()[FLAG_KEY], json!("context-flag"));
        assert!(!normalized.requires_post());
        assert_eq!(
            normalized.query_pairs("requested-flag").unwrap(),
            vec![
                (FLAG_KEY.to_owned(), "requested-flag".to_owned()),
                (TARGETING_KEY.to_owned(), "original".to_owned()),
            ]
        );

        let invalid_collision = EvaluationContext::default()
            .with_targeting_key("original")
            .with_custom_field(
                TARGETING_KEY,
                EvaluationContextFieldValue::new_struct(Unsupported {
                    secret: "must-not-leak",
                }),
            );
        assert!(normalize_context(&invalid_collision).is_ok());
    }

    #[test]
    fn empty_dedicated_targeting_key_allows_custom_targeting_key() {
        let context = EvaluationContext::default()
            .with_targeting_key("")
            .with_custom_field(TARGETING_KEY, "custom");
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(normalized.values()[TARGETING_KEY], json!("custom"));
    }

    #[test]
    fn invalid_custom_targeting_key_without_dedicated_key_is_rejected() {
        let context = EvaluationContext::default().with_custom_field(
            TARGETING_KEY,
            EvaluationContextFieldValue::new_struct(Unsupported {
                secret: "must-not-leak",
            }),
        );
        let error = normalize_context(&context).unwrap_err();

        assert_eq!(
            error.message(),
            "context attribute \"targetingKey\" has an unsupported opaque structured type"
        );
        assert!(!error.message().contains("must-not-leak"));
    }

    #[test]
    fn rejects_non_finite_values_with_exact_path() {
        let context = EvaluationContext::default().with_custom_field("score", f64::NAN);
        let error = normalize_context(&context).unwrap_err();

        assert_eq!(error.code(), &ErrorCode::InvalidContext);
        assert_eq!(
            error.message(),
            "context attribute \"score\" contains a non-finite number"
        );

        let nested = StructValue::default().with_field(
            "items",
            OpenFeatureValue::Array(vec![1_i64.into(), OpenFeatureValue::Float(f64::INFINITY)]),
        );
        let context = EvaluationContext::default().with_custom_field("profile", Arc::new(nested));
        let error = normalize_context(&context).unwrap_err();

        assert_eq!(
            error.message(),
            "context attribute \"profile.items[1]\" contains a non-finite number"
        );
    }

    #[test]
    fn datetimes_use_rfc3339_nano_and_trimmed_fractions() {
        let utc =
            match EvaluationContextFieldValue::DateTime(std::time::SystemTime::UNIX_EPOCH.into()) {
                EvaluationContextFieldValue::DateTime(value) => value
                    .replace_year(2024)
                    .unwrap()
                    .replace_day(2)
                    .unwrap()
                    .replace_hour(3)
                    .unwrap()
                    .replace_minute(4)
                    .unwrap()
                    .replace_second(5)
                    .unwrap()
                    .replace_nanosecond(600)
                    .unwrap(),
                _ => unreachable!(),
            };
        let mut context = EvaluationContext::default();
        context
            .custom_fields
            .insert("utc".to_owned(), EvaluationContextFieldValue::DateTime(utc));

        let normalized = normalize_context(&context).unwrap();
        assert_eq!(
            normalized.values()["utc"],
            json!("2024-01-02T03:04:05.0000006Z")
        );

        let epoch = EvaluationContext::default().with_custom_field(
            "epoch",
            EvaluationContextFieldValue::DateTime(std::time::SystemTime::UNIX_EPOCH.into()),
        );
        let normalized = normalize_context(&epoch).unwrap();
        assert_eq!(normalized.values()["epoch"], json!("1970-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_year_and_offset_rules_are_strict() {
        let year_zero = datetime_with_year(0);
        let year_max = datetime_with_year(9999);
        let invalid_year = datetime_with_year(-1);
        let context = EvaluationContext::default()
            .with_custom_field("year_zero", year_zero)
            .with_custom_field("year_max", year_max);
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(
            normalized.values()["year_zero"],
            json!("0000-01-01T00:00:00Z")
        );
        assert_eq!(
            normalized.values()["year_max"],
            json!("9999-01-01T00:00:00Z")
        );

        let invalid = EvaluationContext::default().with_custom_field("when", invalid_year);
        assert_eq!(
            normalize_context(&invalid).unwrap_err().message(),
            "context attribute \"when\" contains a datetime outside RFC 3339's representable range"
        );

        let path = ContextPath::Root("when");
        assert_eq!(
            format_datetime(2024, 1, 2, 3, 4, 5, 123_400_000, 5 * 3_600 + 30 * 60, &path).unwrap(),
            "2024-01-02T03:04:05.1234+05:30"
        );
        assert_eq!(
            format_datetime(2024, 1, 2, 3, 4, 5, 0, -(3 * 3_600 + 30 * 60), &path).unwrap(),
            "2024-01-02T03:04:05-03:30"
        );

        for invalid_offset in [1, 24 * 3_600, -24 * 3_600] {
            assert!(format_datetime(2024, 1, 2, 3, 4, 5, 0, invalid_offset, &path).is_err());
        }
    }

    #[test]
    fn accepts_depth_100_and_rejects_depth_101_with_bounded_path() {
        let at_limit = nested_json_array(100);
        let context = EvaluationContext::default().with_custom_field("deep", Arc::new(at_limit));
        assert!(normalize_context(&context).is_ok());

        let too_deep = nested_json_array(101);
        let context = EvaluationContext::default().with_custom_field("deep", Arc::new(too_deep));
        let error = normalize_context(&context).unwrap_err();
        assert!(error.message().contains("exceeds maximum nesting depth"));
        assert!(error.message().contains('…'));
        assert!(error.message().len() < 240);
    }

    #[test]
    fn serialized_context_size_limit_is_exact_and_value_safe() {
        let empty = EvaluationContext::default().with_custom_field("value", "");
        let empty_length = normalize_context(&empty)
            .unwrap()
            .canonical_json()
            .unwrap()
            .len();
        let value = "x".repeat(MAX_CONTEXT_JSON_BYTES - empty_length);
        let context = EvaluationContext::default().with_custom_field("value", value);
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(
            normalized.canonical_json().unwrap().len(),
            MAX_CONTEXT_JSON_BYTES
        );

        let secret = "sensitive-value";
        let value = format!(
            "{}{}",
            "x".repeat(MAX_CONTEXT_JSON_BYTES - empty_length + 1 - secret.len()),
            secret
        );
        let context = EvaluationContext::default().with_custom_field("value", value);
        let error = normalize_context(&context).unwrap_err();

        assert_eq!(
            error.message(),
            "exceeds the maximum serialized size of 1 MiB"
        );
        assert!(!error.message().contains(secret));
    }

    #[test]
    fn supports_documented_structs_and_rejects_other_any_values() {
        let open_feature = StructValue::default().with_field("answer", 42);
        let context = EvaluationContext::default()
            .with_custom_field("open_feature", Arc::new(open_feature))
            .with_custom_field("json", Arc::new(json!({"ok": true})));
        let normalized = normalize_context(&context).unwrap();

        assert_eq!(normalized.values()["open_feature"], json!({"answer": 42}));
        assert_eq!(normalized.values()["json"], json!({"ok": true}));

        let unsupported = Unsupported {
            secret: "must-not-leak",
        };
        assert_eq!(unsupported.secret, "must-not-leak");
        let context = EvaluationContext::default().with_custom_field(
            "profile",
            EvaluationContextFieldValue::new_struct(unsupported),
        );
        let error = normalize_context(&context).unwrap_err();

        assert_eq!(
            error.message(),
            "context attribute \"profile\" has an unsupported opaque structured type"
        );
        assert!(!error.message().contains("must-not-leak"));
    }

    #[test]
    fn error_paths_are_escaped_bounded_and_do_not_expose_values() {
        let key = format!("line\n\t\0\u{202e}\u{2067}{}", "x".repeat(1_000));
        let context = EvaluationContext::default().with_custom_field(
            key,
            EvaluationContextFieldValue::new_struct(Unsupported {
                secret: "sensitive-value",
            }),
        );
        let error = normalize_context(&context).unwrap_err();

        assert!(error.message().contains("line\\n"));
        assert!(error.message().contains("\\t"));
        assert!(error.message().contains("\\0"));
        assert!(error.message().contains("\\u{202e}"));
        assert!(error.message().contains("\\u{2067}"));
        assert!(!error.message().contains('\n'));
        assert!(error.message().contains('…'));
        assert!(error.message().len() < 240);
        assert!(!error.message().contains("sensitive-value"));
    }

    #[test]
    fn path_truncation_never_splits_an_escape_sequence() {
        let key = format!("{}\u{202e}", "x".repeat(MAX_ESCAPED_PATH_LENGTH - 1));
        let path = ContextPath::Root(&key);
        let escaped = escaped_path(&path);

        assert!(escaped.ends_with("…\""));
        assert!(!escaped.contains("\\u{"));
    }

    #[test]
    fn path_truncation_prevents_later_segments_from_changing_prefix() {
        let root_key = format!("{}\u{202e}", "x".repeat(MAX_ESCAPED_PATH_LENGTH - 1));
        let root = ContextPath::Root(&root_key);
        let field = ContextPath::Field {
            parent: &root,
            key: "must-not-append",
        };
        let index = ContextPath::Index {
            parent: &field,
            index: 42,
        };

        assert_eq!(escaped_path(&index), escaped_path(&root));
        assert!(!escaped_path(&index).contains("must-not-append"));
        assert!(!escaped_path(&index).contains("[42]"));
    }

    #[test]
    fn debug_redacts_context_values() {
        let context = EvaluationContext::default().with_custom_field("secret", "must-not-leak");
        let normalized = normalize_context(&context).unwrap();
        let debug = format!("{normalized:?}");

        assert!(debug.contains("attribute_count"));
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("must-not-leak"));
    }

    #[test]
    fn cache_key_separates_flag_type_and_tuple_boundaries() {
        let context = normalize_context(&EvaluationContext::default()).unwrap();
        let types = [
            ExpectedFlagType::Bool,
            ExpectedFlagType::String,
            ExpectedFlagType::Int,
            ExpectedFlagType::Float,
            ExpectedFlagType::Object,
        ];
        let keys: std::collections::HashSet<_> = types
            .into_iter()
            .map(|expected| context.cache_key("flag", expected).unwrap())
            .collect();

        assert_eq!(keys.len(), types.len());

        let with_context =
            normalize_context(&EvaluationContext::default().with_custom_field("b", "c")).unwrap();
        assert_ne!(
            context.cache_key("a", ExpectedFlagType::Bool).unwrap(),
            with_context.cache_key("a", ExpectedFlagType::Bool).unwrap()
        );
    }

    fn nested_json_array(depth: usize) -> JsonValue {
        let mut value = JsonValue::Bool(true);

        for _ in 0..depth {
            value = JsonValue::Array(vec![value]);
        }

        value
    }

    fn datetime_with_year(year: i32) -> EvaluationContextFieldValue {
        let datetime =
            match EvaluationContextFieldValue::DateTime(std::time::SystemTime::UNIX_EPOCH.into()) {
                EvaluationContextFieldValue::DateTime(value) => value.replace_year(year).unwrap(),
                _ => unreachable!(),
            };

        EvaluationContextFieldValue::DateTime(datetime)
    }
}
