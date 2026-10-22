//! Internal value coercion for local evaluation.
//!
//! Scalar number and collection coercion follows ECMAScript semantics used by
//! Flagship. Date ordering follows the canonical Go evaluator, including its
//! RFC3339 parser's accepted syntax and strict calendar validation. Extremely
//! large arbitrary-precision JSON numbers map to signed infinity here; the
//! current Go implementation instead treats `json.Number.Float64` range errors
//! as failed numeric coercion (or preserves their text during string coercion).

use std::{borrow::Cow, cmp::Ordering, collections::HashMap};

use serde_json::{Number, Value};

/// Caches pure coercions for immutable context values during one evaluation.
///
/// The cache keys are value addresses rather than serialized values so a large
/// context string is parsed at most once for each coercion kind even when many
/// rules reference it. Callers must keep the referenced values alive and
/// unchanged for the cache's lifetime; the evaluator does both.
#[derive(Default)]
pub(crate) struct CoercionCache {
    numbers: HashMap<*const Value, f64>,
    dates: HashMap<*const Value, Option<i64>>,
}

impl CoercionCache {
    fn number(&mut self, value: &Value) -> f64 {
        let key = std::ptr::from_ref(value);
        if let Some(&number) = self.numbers.get(&key) {
            return number;
        }

        let number = js_number(value);
        self.numbers.insert(key, number);

        number
    }

    fn date(&mut self, value: &Value) -> Option<i64> {
        let key = std::ptr::from_ref(value);
        if let Some(&date) = self.dates.get(&key) {
            return date;
        }

        let date = match value {
            Value::String(value) => parse_iso_millis(value),
            _ => None,
        };
        self.dates.insert(key, date);

        date
    }

    pub(crate) fn compare_values(&mut self, actual: &Value, expected: &Value) -> Option<Ordering> {
        if !is_scalar(actual) || !is_scalar(expected) {
            return None;
        }

        if let (Value::String(_), Value::String(expected)) = (actual, expected)
            && is_iso_target(expected)
            && let Some(actual_millis) = self.date(actual)
        {
            return Some(actual_millis.cmp(&parse_iso_millis(expected)?));
        }

        self.number(actual).partial_cmp(&js_number(expected))
    }
}

/// Whether a JSON value corresponds to a JavaScript scalar accepted by the
/// evaluator. JSON null is deliberately not a scalar.
pub(crate) fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::Bool(_) | Value::Number(_) | Value::String(_))
}

/// Whether a JSON value corresponds to a JavaScript primitive used by the
/// evaluator. Missing attributes are represented by the caller, not as null.
pub(crate) fn is_primitive(value: &Value) -> bool {
    value.is_null() || is_scalar(value)
}

/// Format an IEEE-754 number exactly like ECMAScript `String(number)`.
pub(crate) fn js_string_f64(number: f64) -> String {
    let mut buffer = ryu_js::Buffer::new();

    buffer.format(number).to_owned()
}

fn js_string_number(number: &Number) -> String {
    js_string_f64(number_to_f64(number))
}

fn number_to_f64(number: &Number) -> f64 {
    if let Some(number) = number.as_f64() {
        return number;
    }

    // With serde_json's arbitrary_precision feature, valid JSON such as 1e400
    // remains reachable even though it is outside f64. ECMAScript converts it
    // to signed infinity. The current Go evaluator diverges because it treats
    // strconv range errors as failed conversion despite receiving +/-Inf.
    if number.to_string().starts_with('-') {
        f64::NEG_INFINITY
    } else {
        f64::INFINITY
    }
}

fn js_string_array(values: &[Value]) -> String {
    let mut result = String::new();

    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            result.push(',');
        }

        // Array.prototype.toString treats null array elements as empty slots.
        if !value.is_null() {
            result.push_str(js_string(value).as_ref());
        }
    }

    result
}

/// Convert a JSON value using the JavaScript `String` rules relevant to local
/// evaluation, including array element conversion used by collection checks.
pub(crate) fn js_string(value: &Value) -> Cow<'_, str> {
    match value {
        Value::Null => Cow::Borrowed("null"),
        Value::Bool(true) => Cow::Borrowed("true"),
        Value::Bool(false) => Cow::Borrowed("false"),
        Value::Number(number) => Cow::Owned(js_string_number(number)),
        Value::String(string) => Cow::Borrowed(string),
        Value::Array(values) => Cow::Owned(js_string_array(values)),
        Value::Object(_) => Cow::Borrowed("[object Object]"),
    }
}

fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}' | '\u{000B}' | '\u{000C}' | '\u{0020}' | '\u{00A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
                | '\n'
                | '\r'
    )
}

fn trim_js_whitespace(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

fn is_decimal_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut integer_digits = 0;

    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        integer_digits += 1;
        index += 1;
    }

    let mut fractional_digits = 0;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            fractional_digits += 1;
            index += 1;
        }
    }

    if integer_digits + fractional_digits == 0 {
        return false;
    }

    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }

        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }

    index == bytes.len()
}

fn radix_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the caller proves the integer has at most 53 significant bits"
)]
fn exact_significand_to_f64(significand: u64) -> f64 {
    significand as f64
}

fn parse_radix_number(digits: &[u8], radix: u8) -> Option<f64> {
    if digits.is_empty() {
        return None;
    }

    let bits_per_digit = match radix {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => return None,
    };
    let mut first_nonzero = None;
    for (index, &byte) in digits.iter().enumerate() {
        let digit = radix_digit(byte)?;
        if digit >= radix {
            return None;
        }

        if digit != 0 && first_nonzero.is_none() {
            first_nonzero = Some((index, digit));
        }
    }

    let Some((first_nonzero, first_digit)) = first_nonzero else {
        return Some(0.0);
    };

    let first_digit_bits = u8::BITS - first_digit.leading_zeros();
    let remaining_digits = digits.len() - first_nonzero - 1;
    let bit_length = remaining_digits
        .saturating_mul(bits_per_digit)
        .saturating_add(first_digit_bits as usize);

    let mut significand = 0_u64;
    let mut significant_bits = 0_usize;
    let mut halfway = false;
    let mut sticky = false;
    let mut seen_first_bit = false;

    for &byte in &digits[first_nonzero..] {
        let digit = radix_digit(byte)?;

        for bit_index in (0..bits_per_digit).rev() {
            let bit = (digit >> bit_index) & 1;
            if !seen_first_bit {
                if bit == 0 {
                    continue;
                }
                seen_first_bit = true;
            }

            match significant_bits.cmp(&53) {
                Ordering::Less => {
                    significand = (significand << 1) | u64::from(bit);
                    significant_bits += 1;
                }
                Ordering::Equal => {
                    halfway = bit != 0;
                    significant_bits += 1;
                }
                Ordering::Greater => sticky |= bit != 0,
            }
        }
    }

    if bit_length <= 53 {
        return Some(exact_significand_to_f64(significand));
    }

    let round_up = halfway && (sticky || significand & 1 != 0);
    if round_up {
        significand += 1;
    }

    let rounded_bit_length = bit_length.saturating_add(usize::from(significand == 1 << 53));
    if significand == 1 << 53 {
        significand >>= 1;
    }
    if rounded_bit_length > 1_024 {
        return Some(f64::INFINITY);
    }

    let exponent = i32::try_from(rounded_bit_length - 53).ok()?;

    Some(exact_significand_to_f64(significand) * 2.0_f64.powi(exponent))
}

enum PrefixedNumber {
    NotPrefixed,
    Invalid,
    Value(f64),
}

fn parse_prefixed_number(value: &str) -> PrefixedNumber {
    let bytes = value.as_bytes();
    let radix = match bytes.get(1) {
        Some(b'x' | b'X') if bytes.first() == Some(&b'0') => 16,
        Some(b'o' | b'O') if bytes.first() == Some(&b'0') => 8,
        Some(b'b' | b'B') if bytes.first() == Some(&b'0') => 2,
        _ => return PrefixedNumber::NotPrefixed,
    };

    parse_radix_number(&bytes[2..], radix).map_or(PrefixedNumber::Invalid, PrefixedNumber::Value)
}

/// Parse a string using JavaScript `Number` syntax, except that the Flagship
/// evaluator intentionally maps an empty or whitespace-only string to NaN.
pub(crate) fn js_number_string(value: &str) -> f64 {
    let value = trim_js_whitespace(value);
    if value.is_empty() {
        return f64::NAN;
    }

    match value {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }

    match parse_prefixed_number(value) {
        PrefixedNumber::Value(number) => return number,
        PrefixedNumber::Invalid => return f64::NAN,
        PrefixedNumber::NotPrefixed => {}
    }
    if !is_decimal_number(value) {
        return f64::NAN;
    }

    value.parse().unwrap_or(f64::NAN)
}

/// Apply Flagship's `to_number` coercion to a JSON value.
///
/// This follows ECMAScript number coercion for supported scalars, except
/// Flagship deliberately maps blank strings, null, arrays, and objects to NaN.
/// Arbitrary-precision JSON numbers outside the f64 range map to signed
/// infinity; see the module-level note about the current Go divergence.
pub(crate) fn js_number(value: &Value) -> f64 {
    match value {
        Value::Bool(true) => 1.0,
        Value::Bool(false) => 0.0,
        Value::Number(number) => number_to_f64(number),
        Value::String(string) => js_number_string(string),
        Value::Null | Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

/// Whether a target selects the evaluator's strict date-ordering mode.
///
/// This checks only the RFC3339-like lexical shape. Calendar validity is
/// checked by [`parse_iso_millis`].
pub(crate) fn is_iso_target(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }

    let fixed_digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if fixed_digits
        .iter()
        .any(|&index| !bytes[index].is_ascii_digit())
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return false;
    }

    let mut suffix = 19;
    if bytes.get(suffix) == Some(&b'.') {
        suffix += 1;
        let fraction_start = suffix;
        while bytes.get(suffix).is_some_and(u8::is_ascii_digit) {
            suffix += 1;
        }
        if suffix == fraction_start {
            return false;
        }
    }

    bytes.get(suffix..) == Some(b"Z")
        || (bytes.len() == suffix + 6
            && matches!(bytes[suffix], b'+' | b'-')
            && bytes[suffix + 1..suffix + 3].iter().all(u8::is_ascii_digit)
            && bytes[suffix + 3] == b':'
            && bytes[suffix + 4..suffix + 6].iter().all(u8::is_ascii_digit))
}

fn parse_two_digits(bytes: &[u8], index: usize) -> i64 {
    i64::from(bytes[index] - b'0') * 10 + i64::from(bytes[index + 1] - b'0')
}

fn has_two_digits(bytes: &[u8], index: usize) -> bool {
    bytes
        .get(index..index.saturating_add(2))
        .is_some_and(|digits| digits.iter().all(u8::is_ascii_digit))
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> Option<i64> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if is_leap_year(year) => Some(29),
        2 => Some(28),
        _ => None,
    }
}

// Howard Hinnant's civil-date conversion, shifted to the Unix epoch.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

/// Parse a Go-compatible RFC3339 timestamp as UTC epoch milliseconds.
///
/// Fractional seconds are truncated to milliseconds. Following Go's
/// `time.Parse(time.RFC3339Nano, ...)`, attributes may use a one-digit hour or
/// comma fractional separator, and offsets permit hour 24 or minute 60. The
/// calendar date and clock hour remain strict. This deliberately differs from
/// TypeScript `Date.parse`, which can accept or roll over other invalid dates.
pub(crate) fn parse_iso_millis(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || bytes[4] != b'-'
        || !has_two_digits(bytes, 5)
        || bytes[7] != b'-'
        || !has_two_digits(bytes, 8)
        || bytes[10] != b'T'
        || !bytes.get(11).is_some_and(u8::is_ascii_digit)
    {
        return None;
    }

    let year = bytes[..4]
        .iter()
        .fold(0_i64, |year, digit| year * 10 + i64::from(digit - b'0'));
    let month = parse_two_digits(bytes, 5);
    let day = parse_two_digits(bytes, 8);

    let (hour, hour_separator) = if bytes.get(12) == Some(&b':') {
        (i64::from(bytes[11] - b'0'), 12)
    } else if has_two_digits(bytes, 11) && bytes.get(13) == Some(&b':') {
        (parse_two_digits(bytes, 11), 13)
    } else {
        return None;
    };

    let minute_index = hour_separator + 1;
    let second_index = minute_index + 3;
    if !has_two_digits(bytes, minute_index)
        || bytes.get(minute_index + 2) != Some(&b':')
        || !has_two_digits(bytes, second_index)
    {
        return None;
    }

    let minute = parse_two_digits(bytes, minute_index);
    let second = parse_two_digits(bytes, second_index);

    if day == 0 || day > days_in_month(year, month)? || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let mut suffix = second_index + 2;
    let mut milliseconds = 0;
    if matches!(bytes.get(suffix), Some(b'.' | b',')) {
        suffix += 1;
        let fraction_start = suffix;
        for factor in [100, 10, 1] {
            let Some(digit) = bytes.get(suffix).filter(|digit| digit.is_ascii_digit()) else {
                break;
            };
            milliseconds += i64::from(digit - b'0') * factor;
            suffix += 1;
        }
        while bytes.get(suffix).is_some_and(u8::is_ascii_digit) {
            suffix += 1;
        }
        if suffix == fraction_start {
            return None;
        }
    }

    let offset_seconds = if bytes.get(suffix..) == Some(b"Z") {
        0
    } else {
        if bytes.len() != suffix + 6
            || !matches!(bytes.get(suffix), Some(b'+' | b'-'))
            || !has_two_digits(bytes, suffix + 1)
            || bytes.get(suffix + 3) != Some(&b':')
            || !has_two_digits(bytes, suffix + 4)
        {
            return None;
        }

        let offset_hour = parse_two_digits(bytes, suffix + 1);
        let offset_minute = parse_two_digits(bytes, suffix + 4);
        if offset_hour > 24 || offset_minute > 60 {
            return None;
        }

        let magnitude = offset_hour * 3_600 + offset_minute * 60;
        if bytes[suffix] == b'+' {
            magnitude
        } else {
            -magnitude
        }
    };

    let local_seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;

    Some((local_seconds - offset_seconds) * 1_000 + milliseconds)
}

/// Compare scalar values using Flagship numeric and strict date coercion.
///
/// A lexically date-shaped target selects date mode only when the attribute is
/// also accepted by Go's RFC3339 parser. Invalid attributes follow Go's numeric
/// fallback; unlike TypeScript, permissive `Date.parse` rollover is not used.
#[cfg(test)]
fn compare_values(actual: &Value, expected: &Value) -> Option<Ordering> {
    let mut cache = CoercionCache::default();

    cache.compare_values(actual, expected)
}

/// Whether an array has a non-null element whose JavaScript string form equals
/// the target's string form. Used by `in` and `not_in`.
pub(crate) fn contains_value(values: &[Value], target: &Value) -> bool {
    let expected = js_string(target);

    values
        .iter()
        .any(|value| !value.is_null() && js_string(value) == expected)
}

/// Whether an array has a scalar element whose JavaScript string form equals
/// the target's string form. Used by `has` and `not_has`.
pub(crate) fn contains_primitive_value(values: &[Value], target: &Value) -> bool {
    let expected = js_string(target);

    values
        .iter()
        .any(|value| is_scalar(value) && js_string(value) == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identifies_scalar_and_primitive_values() {
        let cases = [
            (json!(null), false, true),
            (json!(true), true, true),
            (json!(1), true, true),
            (json!("x"), true, true),
            (json!([]), false, false),
            (json!({}), false, false),
        ];

        for (value, scalar, primitive) in cases {
            assert_eq!(is_scalar(&value), scalar, "scalar: {value}");
            assert_eq!(is_primitive(&value), primitive, "primitive: {value}");
        }
    }

    #[test]
    fn formats_numbers_like_javascript() {
        let cases = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (1e-7, "1e-7"),
            (1e-6, "0.000001"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
        ];

        for (value, expected) in cases {
            assert_eq!(js_string_f64(value), expected, "value: {value}");
        }
        assert_eq!(js_string_f64(f64::NAN), "NaN");
    }

    #[test]
    fn stringifies_json_values_like_javascript() {
        let cases = [
            (json!(null), "null"),
            (json!(true), "true"),
            (json!(false), "false"),
            (json!(1.5), "1.5"),
            (json!(9_007_199_254_740_993_u64), "9007199254740992"),
            (json!("hello"), "hello"),
            (json!(["a", null, 2, [true, false]]), "a,,2,true,false"),
            (json!({"key": "value"}), "[object Object]"),
        ];

        for (value, expected) in cases {
            assert_eq!(js_string(&value), expected, "value: {value}");
        }

        let string = json!("borrowed");
        assert!(matches!(js_string(&string), Cow::Borrowed("borrowed")));
        assert!(matches!(js_string(&Value::Null), Cow::Borrowed("null")));
    }

    #[test]
    fn parses_javascript_numbers() {
        let cases = [
            ("42", 42.0),
            ("+42", 42.0),
            (".5", 0.5),
            ("1.", 1.0),
            ("1.25e2", 125.0),
            ("0x10", 16.0),
            ("0o10", 8.0),
            ("0b10", 2.0),
            ("Infinity", f64::INFINITY),
            ("+Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("\u{FEFF}\u{2007}42\u{3000}", 42.0),
        ];

        for (value, expected) in cases {
            assert_eq!(js_number_string(value), expected, "value: {value:?}");
        }
    }

    #[test]
    fn parses_prefixed_integers_with_one_correct_rounding_step() {
        let cases = [
            ("0x20000000000001", 9_007_199_254_740_992.0),
            ("0x20000000000003", 9_007_199_254_740_996.0),
            ("\u{FEFF}0X20000000000001\u{3000}", 9_007_199_254_740_992.0),
            ("0O20", 16.0),
            ("0B10", 2.0),
        ];

        for (value, expected) in cases {
            assert_eq!(js_number_string(value), expected, "value: {value:?}");
        }

        let octal = format!("0o4{}1", "0".repeat(16));
        let binary = format!("0b1{}1", "0".repeat(52));
        let overflow = format!("0x1{}", "0".repeat(256));
        assert_eq!(js_number_string(&octal), 9_007_199_254_740_992.0);
        assert_eq!(js_number_string(&binary), 9_007_199_254_740_992.0);
        assert_eq!(js_number_string(&overflow), f64::INFINITY);
    }

    #[test]
    fn radix_rounding_uses_discarded_sticky_bits_and_handles_carry() {
        // Repeated float multiply/add rounds this value twice and produces the
        // adjacent lower float. One exact integer conversion must retain the
        // low sticky bit before rounding.
        assert_eq!(
            js_number_string("0x1000000000000081").to_bits(),
            0x43b0_0000_0000_0001
        );

        // Rounding the all-ones significand carries into the next exponent.
        assert_eq!(
            js_number_string("0x1fffffffffffff800").to_bits(),
            0x4400_0000_0000_0000
        );
    }

    #[test]
    fn preserves_decimal_zero_sign_and_extreme_exponents() {
        assert_eq!(js_number_string("-0").to_bits(), (-0.0_f64).to_bits());
        assert_eq!(js_number_string("-0e100").to_bits(), (-0.0_f64).to_bits());
        assert_eq!(js_number_string("1e309"), f64::INFINITY);
        assert_eq!(js_number_string("-1e309"), f64::NEG_INFINITY);
        assert_eq!(js_number_string("1e-400").to_bits(), 0.0_f64.to_bits());
        assert_eq!(js_number_string("-1e-400").to_bits(), (-0.0_f64).to_bits());
        assert_eq!(
            js_number_string("9007199254740993"),
            9_007_199_254_740_992.0
        );
        assert_eq!(js_number_string("1.7976931348623157e308"), f64::MAX);
        assert_eq!(js_number_string("1.7976931348623159e308"), f64::INFINITY);
        assert_eq!(
            js_number_string("-1.7976931348623159e308"),
            f64::NEG_INFINITY
        );
    }

    #[test]
    fn rejects_non_javascript_numbers_and_blank_strings() {
        let cases = [
            "",
            " \t\n",
            "inf",
            "nan",
            "infinity",
            "1_000",
            "0x",
            "-0x10",
            "0b2",
            "1e",
            ".",
            "1 2",
            "\u{0085}42",
            "４２",
        ];

        for value in cases {
            assert!(js_number_string(value).is_nan(), "value: {value:?}");
        }
    }

    #[test]
    fn parses_iso_timestamps_to_truncated_epoch_milliseconds() {
        let cases = [
            ("0000-01-01T00:00:00Z", -62_167_219_200_000),
            ("1970-01-01T00:00:00Z", 0),
            ("1970-01-01T00:00:00.1Z", 100),
            ("1970-01-01T00:00:00.123999Z", 123),
            ("1969-12-31T23:59:59.999999Z", -1),
            ("1970-01-01T01:00:00+01:00", 0),
            ("1969-12-31T23:00:00-01:00", 0),
            ("1970-01-01T00:00:00+23:59", -86_340_000),
            ("1970-01-01T00:00:00-23:59", 86_340_000),
            ("1970-01-01T00:00:00+24:60", -90_000_000),
            ("1970-01-01T00:00:00-24:60", 90_000_000),
            ("1970-01-01T0:00:00Z", 0),
            ("1970-01-01T0:00:00,1239Z", 123),
            ("2000-02-29T00:00:00Z", 951_782_400_000),
            ("2400-02-29T00:00:00Z", 13_574_563_200_000),
        ];

        for (value, expected) in cases {
            assert_eq!(parse_iso_millis(value), Some(expected), "value: {value}");
        }
    }

    #[test]
    fn date_mode_target_shape_stays_narrower_than_go_attribute_parsing() {
        assert!(is_iso_target("2023-01-01T00:00:00+24:60"));
        assert!(is_iso_target("2023-01-01T00:00:00.123Z"));
        assert!(!is_iso_target("2023-01-01T0:00:00Z"));
        assert!(!is_iso_target("2023-01-01T00:00:00,123Z"));

        assert!(parse_iso_millis("2023-01-01T0:00:00Z").is_some());
        assert!(parse_iso_millis("2023-01-01T00:00:00,123Z").is_some());
    }

    #[test]
    fn rejects_invalid_iso_timestamps() {
        let cases = [
            "2023-01-01",
            "2023-01-01t00:00:00Z",
            "2023-01-01T00:00:00",
            "2023-01-01T00:00:00.Z",
            "2023-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2023-01-01T24:00:00Z",
            "2023-01-01T00:00:00+25:00",
            "2023-01-01T00:00:00-25:00",
            "2023-01-01T00:00:00+24:61",
            "2023-01-01T00:00:00+00:61",
        ];

        for value in cases {
            assert_eq!(parse_iso_millis(value), None, "value: {value}");
        }
    }

    #[test]
    fn compares_numeric_and_iso_values() {
        let cases = [
            (json!(2), json!("10"), Some(Ordering::Less)),
            (json!(true), json!(1), Some(Ordering::Equal)),
            (json!("Infinity"), json!(10), Some(Ordering::Greater)),
            (json!("-Infinity"), json!(-10), Some(Ordering::Less)),
            (json!("-0"), json!(0), Some(Ordering::Equal)),
            (
                json!("2023-01-01T00:00:00.123999Z"),
                json!("2023-01-01T00:00:00.123Z"),
                Some(Ordering::Equal),
            ),
            (
                json!("2023-01-01T1:02:03Z"),
                json!("2023-01-01T01:02:03Z"),
                Some(Ordering::Equal),
            ),
            (
                json!("2023-01-01T01:02:03,1239+24:60"),
                json!("2023-01-01T01:02:03.123+24:60"),
                Some(Ordering::Equal),
            ),
            (
                json!("2023-02-30T00:00:00Z"),
                json!("2023-03-01T00:00:00Z"),
                None,
            ),
            (json!("10"), json!("2023-01-01T00:00:00Z"), None),
            (json!("not-a-number"), json!(1), None),
            (json!(null), json!(1), None),
        ];

        for (actual, expected, ordering) in cases {
            assert_eq!(compare_values(&actual, &expected), ordering);
        }
    }

    #[test]
    fn caches_repeated_actual_numeric_and_date_coercions() {
        let numeric = Value::String(format!("0x{}", "f".repeat(2_048)));
        let numeric_target = json!(0);
        let date = Value::String(format!("1970-01-01T00:00:00.{}Z", "1".repeat(2_048)));
        let date_target = json!("1970-01-01T00:00:00Z");
        let mut cache = CoercionCache::default();

        for _ in 0..100 {
            assert_eq!(
                cache.compare_values(&numeric, &numeric_target),
                Some(Ordering::Greater)
            );
            assert_eq!(
                cache.compare_values(&date, &date_target),
                Some(Ordering::Greater)
            );
        }

        assert_eq!(cache.numbers.len(), 1);
        assert_eq!(cache.dates.len(), 1);
    }

    #[test]
    fn maps_out_of_range_arbitrary_precision_json_numbers_to_infinity() {
        let positive = serde_json::from_str::<Value>("1e400");
        let negative = serde_json::from_str::<Value>("-1e400");
        if positive.is_err() || negative.is_err() {
            // The default serde_json configuration rejects these at the wire
            // boundary. The assertions below run when arbitrary_precision is
            // enabled explicitly for the dependency.
            assert!(positive.is_err());
            assert!(negative.is_err());
            return;
        }

        let positive = positive.unwrap_or(Value::Null);
        let negative = negative.unwrap_or(Value::Null);
        assert_eq!(js_string(&positive), "Infinity");
        assert_eq!(js_string(&negative), "-Infinity");
        assert_eq!(js_number(&positive), f64::INFINITY);
        assert_eq!(js_number(&negative), f64::NEG_INFINITY);
    }

    #[test]
    fn applies_collection_membership_stringification_rules() {
        let values = json!(["US", 1, true, null, ["a", "b"], {"x": 1}]);
        let values = values.as_array().unwrap_or_else(|| unreachable!());

        assert!(contains_value(values, &json!("1")));
        assert!(contains_value(values, &json!("a,b")));
        assert!(contains_value(values, &json!("[object Object]")));
        assert!(!contains_value(values, &Value::Null));
        assert!(contains_primitive_value(values, &json!("true")));
        assert!(!contains_primitive_value(values, &json!("a,b")));
    }
}
