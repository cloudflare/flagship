"""In-process flag evaluation engine.

Port of the Flagship data-plane ``evaluateFlag``. Private to the package —
not part of the public API. Every result must be bit-identical to the
server for the same definitions and context.
"""

from __future__ import annotations

import math
import random
import re
from datetime import datetime, timezone
from typing import Any, Literal, TypedDict

from .context import JsonValue

__all__ = ["FlagConfigError", "EvaluationResult", "evaluate_flag", "js_number", "js_str", "murmurhash3"]

EvaluationReason = Literal["STATIC", "TARGETING_MATCH", "DEFAULT", "DISABLED", "SPLIT"]


class EvaluationResult(TypedDict):
    value: Any
    variant: str
    reason: EvaluationReason


_ISO_8601_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$")
_SCI_RE = re.compile(r"^(-?)(\d+)(?:\.(\d+))?e([+-]?)(\d+)$", re.IGNORECASE)
_JS_WHITESPACE_RE = re.compile(r"^[\t\v\f \u00a0\ufeff\n\r\u2028\u2029]+|[\t\v\f \u00a0\ufeff\n\r\u2028\u2029]+$")
_MAX_PATH_DEPTH = 5
_HASH_QUOTIENT_RANGE = math.ceil((2**32) / 100)
_SURROGATE_RE = re.compile(r"[\ud800-\udfff]")


class FlagConfigError(Exception):
    """Raised when a flag definition is internally inconsistent."""

    def __init__(self, flag_key: str, message: str) -> None:
        super().__init__(f"Flag '{flag_key}' {message}")
        self.flag_key = flag_key


def js_str(value: Any) -> str:
    """Match JavaScript ``String(value)`` for scalars used by the engine."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, str):
        return value
    if isinstance(value, int) and not isinstance(value, bool):
        return str(value)
    if isinstance(value, float):
        return _js_str_float(value)
    if value is None:
        return "null"
    return str(value)


def _js_str_float(n: float) -> str:
    if math.isnan(n):
        return "NaN"
    if n == 0.0:
        return "0"
    if math.isinf(n):
        return "Infinity" if n > 0 else "-Infinity"

    abs_n = abs(n)
    r = repr(n)
    m = _SCI_RE.match(r)
    if m:
        sign, intp, frac, exp_sign, exp_digits = m.groups()
        exp = int(exp_digits)
        if exp_sign == "-":
            exp = -exp
        all_digits = intp + (frac or "")
        sig = all_digits.rstrip("0") or "0"
        first_exp = exp + (len(intp) - 1)
        use_sci = abs_n < 1e-6 or abs_n >= 1e21
        if use_sci:
            mant = sig if len(sig) == 1 else f"{sig[0]}.{sig[1:]}"
            exp_str = f"+{first_exp}" if first_exp >= 0 else str(first_exp)
            return f"{sign}{mant}e{exp_str}"
        if first_exp >= 0:
            if len(sig) <= first_exp + 1:
                body = sig + "0" * (first_exp + 1 - len(sig))
            else:
                body = sig[: first_exp + 1] + "." + sig[first_exp + 1 :]
        else:
            body = "0." + ("0" * (-first_exp - 1)) + sig
        return f"{sign}{body}"

    if r.endswith(".0"):
        return r[:-2]
    return r


def js_number(value: Any) -> float:
    """Match JavaScript ``Number(value)`` for scalars, with empty-string → NaN."""
    if isinstance(value, bool):
        return 1.0 if value else 0.0
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return float(value)
    if not isinstance(value, str):
        return float("nan")

    # JS trims whitespace then parses. Whitespace-only → 0 in JS Number(), but
    # the engine's toNumber special-cases empty/whitespace-only as NaN.
    trimmed = _JS_WHITESPACE_RE.sub("", value)
    if trimmed == "":
        return float("nan")

    lower = trimmed.lower()
    if lower == "infinity" or lower == "+infinity":
        return float("inf")
    if lower == "-infinity":
        return float("-inf")

    # Reject Python-only syntax that JS Number does not accept.
    if "_" in trimmed:
        return float("nan")
    if lower in ("inf", "+inf", "-inf", "nan"):
        return float("nan")

    # Hex / octal / binary — JS Number accepts these prefixes.
    try:
        if len(trimmed) >= 2 and trimmed[0] == "0" and trimmed[1] in "xXoObB":
            return float(int(trimmed, 0))
        return float(trimmed)
    except ValueError:
        return float("nan")


def murmurhash3(s: str, seed: int) -> int:
    """MurmurHash3 x86_32 over UTF-8 bytes. Returns an unsigned 32-bit int."""
    # Match TextEncoder: lone surrogates become U+FFFD (EF BF BD).
    encoded = _SURROGATE_RE.sub("\ufffd", s).encode("utf-8")
    data = encoded
    n = len(data)
    h = seed & 0xFFFFFFFF
    i = 0
    while i + 4 <= n:
        k = data[i] | (data[i + 1] << 8) | (data[i + 2] << 16) | (data[i + 3] << 24)
        k = (k * 0xCC9E2D51) & 0xFFFFFFFF
        k = ((k << 15) | (k >> 17)) & 0xFFFFFFFF
        k = (k * 0x1B873593) & 0xFFFFFFFF
        h ^= k
        h = ((h << 13) | (h >> 19)) & 0xFFFFFFFF
        h = (h * 5 + 0xE6546B64) & 0xFFFFFFFF
        i += 4

    k = 0
    remaining = n - i
    if remaining >= 3:
        k ^= data[i + 2] << 16
    if remaining >= 2:
        k ^= data[i + 1] << 8
    if remaining >= 1:
        k ^= data[i]
        k = (k * 0xCC9E2D51) & 0xFFFFFFFF
        k = ((k << 15) | (k >> 17)) & 0xFFFFFFFF
        k = (k * 0x1B873593) & 0xFFFFFFFF
        h ^= k

    h ^= n
    h ^= h >> 16
    h = (h * 0x85EBCA6B) & 0xFFFFFFFF
    h ^= h >> 13
    h = (h * 0xC2B2AE35) & 0xFFFFFFFF
    h ^= h >> 16
    return h & 0xFFFFFFFF


def is_scalar(value: Any) -> bool:
    """True for JS-scalar equivalents: string, number, boolean (not null/list/dict)."""
    if isinstance(value, bool):
        return True
    if isinstance(value, (str, int, float)):
        return True
    return False


def _is_primitive(value: Any) -> bool:
    """JS ``isPrimitive``: null or non-object/non-function. None counts as primitive."""
    return value is None or is_scalar(value)


def _get_context_value(context: dict[str, JsonValue], attribute: str) -> Any:
    if attribute in context:
        return context[attribute]
    if "." not in attribute:
        return _MISSING

    path = attribute.split(".")
    if len(path) > _MAX_PATH_DEPTH:
        return _MISSING

    value: Any = context
    for segment in path:
        if _is_primitive(value) or not isinstance(value, dict) or segment not in value:
            return _MISSING
        value = value[segment]
    return value


_MISSING = object()


def _evaluate_string_operator(operator: str, attr_value: Any, target: Any) -> bool:
    if attr_value is None:
        return operator == "not_equals"
    if not is_scalar(attr_value) or not _is_primitive(target):
        return False
    actual = js_str(attr_value)
    expected = js_str(target)

    if operator == "equals":
        return actual == expected
    if operator == "not_equals":
        return actual != expected
    if operator == "contains":
        return expected in actual
    if operator == "starts_with":
        return actual.startswith(expected)
    if operator == "ends_with":
        return actual.endswith(expected)
    return False


def _parse_iso_ms(value: str) -> float:
    """Parse an ISO-8601 string to epoch milliseconds, matching ``Date.parse``."""
    try:
        # Python 3.10 needs Z → +00:00; 3.11+ accepts Z natively.
        normalized = value.replace("Z", "+00:00") if value.endswith("Z") else value
        dt = datetime.fromisoformat(normalized)
        if dt.tzinfo is None:
            # Naive strings shouldn't reach here (regex requires offset), but
            # treat as UTC to stay defensive.
            dt = dt.replace(tzinfo=timezone.utc)
        return dt.timestamp() * 1000.0
    except ValueError:
        return float("nan")


def _compare_values(attr_value: Any, target: Any) -> int | None:
    if not is_scalar(attr_value) or not is_scalar(target):
        return None

    if isinstance(target, str) and _ISO_8601_RE.match(target) is not None and isinstance(attr_value, str):
        actual = _parse_iso_ms(attr_value)
        if math.isnan(actual):
            actual = js_number(attr_value)
            expected = js_number(target)
        else:
            expected = _parse_iso_ms(target)
    else:
        actual = js_number(attr_value)
        expected = js_number(target)

    if math.isnan(actual) or math.isnan(expected):
        return None
    if actual == expected:
        return 0
    return -1 if actual < expected else 1


def _evaluate_ordering_operator(operator: str, attr_value: Any, target: Any) -> bool:
    comparison = _compare_values(attr_value, target)
    if comparison is None:
        return False
    if operator == "greater_than":
        return comparison > 0
    if operator == "less_than":
        return comparison < 0
    if operator == "greater_than_or_equals":
        return comparison >= 0
    if operator == "less_than_or_equals":
        return comparison <= 0
    return False


def _contains_value(values: list[Any], target: Any) -> bool:
    expected = js_str(target)
    for value in values:
        if value is not None and js_str(value) == expected:
            return True
    return False


def _contains_primitive_value(values: list[Any], target: Any) -> bool:
    expected = js_str(target)
    for value in values:
        if is_scalar(value) and js_str(value) == expected:
            return True
    return False


def _evaluate_condition(condition: Any, context: dict[str, JsonValue]) -> bool:
    if not isinstance(condition, dict):
        return False

    if "logical_operator" in condition:
        logical_operator = condition.get("logical_operator")
        clauses = condition.get("clauses")
        if not isinstance(clauses, list):
            return False
        if logical_operator == "AND":
            return all(_evaluate_condition(clause, context) for clause in clauses)
        if logical_operator == "OR":
            return any(_evaluate_condition(clause, context) for clause in clauses)
        return False

    attribute = condition.get("attribute")
    operator = condition.get("operator")
    target = condition.get("value")
    if not isinstance(attribute, str) or not isinstance(operator, str):
        return False

    attr_value = _get_context_value(context, attribute)
    if attr_value is _MISSING:
        return False

    if operator in ("equals", "not_equals", "contains", "starts_with", "ends_with"):
        return _evaluate_string_operator(operator, attr_value, target)
    if operator in ("greater_than", "less_than", "greater_than_or_equals", "less_than_or_equals"):
        return _evaluate_ordering_operator(operator, attr_value, target)
    if operator in ("in", "not_in"):
        if not isinstance(target, list):
            return False
        if attr_value is None:
            return operator == "not_in"
        if not is_scalar(attr_value):
            return False
        contained = _contains_value(target, attr_value)
        return contained if operator == "in" else not contained
    if operator in ("has", "not_has"):
        if not isinstance(attr_value, list) or not _is_primitive(target):
            return False
        contained = _contains_primitive_value(attr_value, target)
        return contained if operator == "has" else not contained
    return False


def evaluate_flag(
    flag_def: dict[str, Any],
    context: dict[str, JsonValue],
    account_id: str,
) -> EvaluationResult:
    """Evaluate a single flag definition against the provided context.

    Pure function — no I/O. Raises :class:`FlagConfigError` when a referenced
    variation is missing from ``variations``.
    """
    key = flag_def.get("key", "")
    variations = flag_def.get("variations")
    if not isinstance(variations, dict):
        variations = {}

    def serve(variant: str, reason: EvaluationReason) -> EvaluationResult:
        if variant not in variations:
            raise FlagConfigError(str(key), f"variation '{variant}' is not defined")
        return {"value": variations[variant], "variant": variant, "reason": reason}

    if not flag_def.get("enabled", False):
        return serve(str(flag_def.get("default_variation", "")), "DISABLED")

    rules = flag_def.get("rules")
    if not isinstance(rules, list) or len(rules) == 0:
        return serve(str(flag_def.get("default_variation", "")), "STATIC")

    seed: int | None = None

    for rule in rules:
        if not isinstance(rule, dict):
            continue
        conditions = rule.get("conditions")
        if not isinstance(conditions, list):
            conditions = []

        rule_matches = True
        for condition in conditions:
            if not _evaluate_condition(condition, context):
                rule_matches = False
                break

        rollout = rule.get("rollout")
        is_split = isinstance(rollout, dict) and float(rollout.get("percentage", 100)) < 100
        if rule_matches and isinstance(rollout, dict):
            attr_name = rollout.get("attribute") or "targetingKey"
            if not isinstance(attr_name, str):
                attr_name = "targetingKey"
            attr = _get_context_value(context, attr_name)
            if attr is not None and attr is not _MISSING and not is_scalar(attr):
                rule_matches = False
            elif is_split:
                percentage = float(rollout.get("percentage", 0))
                if seed is None:
                    seed = murmurhash3(f"{account_id}:{key}", 0) % 100
                if is_scalar(attr) and attr is not _MISSING:
                    h = murmurhash3(js_str(attr), seed)
                    bucket = (h % 100) + math.floor(h / 100) / _HASH_QUOTIENT_RANGE
                else:
                    # Missing or null → random bucket.
                    bucket = random.random() * 100
                if bucket >= percentage:
                    rule_matches = False

        if rule_matches:
            serve_variation = str(rule.get("serve_variation", ""))
            return serve(serve_variation, "SPLIT" if is_split else "TARGETING_MATCH")

    return serve(str(flag_def.get("default_variation", "")), "DEFAULT")
