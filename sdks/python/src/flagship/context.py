import math
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Any, TypeAlias

from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import InvalidContextError

__all__ = ["NormalizedContext", "context_to_query_params", "normalize_context"]

JsonValue: TypeAlias = str | int | float | bool | None | list["JsonValue"] | dict[str, "JsonValue"]


@dataclass(frozen=True)
class NormalizedContext:
    values: dict[str, JsonValue]
    requires_post: bool


def normalize_context(context: EvaluationContext | None) -> NormalizedContext:
    if context is None:
        return NormalizedContext({}, False)

    values: dict[str, JsonValue] = {}
    requires_post = False
    ancestors: set[int] = set()

    if context.targeting_key is not None:
        values["targetingKey"] = str(context.targeting_key)

    for key, value in context.attributes.items():
        normalized, structured = _normalize_value(value, key, ancestors)
        values[key] = normalized
        requires_post = requires_post or structured

    return NormalizedContext(values, requires_post)


def context_to_query_params(context: EvaluationContext | None) -> dict[str, str]:
    normalized = normalize_context(context)
    if normalized.requires_post:
        raise InvalidContextError("Structured evaluation context requires a JSON request body")

    params: dict[str, str] = {}
    for key, value in normalized.values.items():
        if isinstance(value, bool):
            params[key] = "true" if value else "false"
        else:
            params[key] = str(value)
    return params


def _normalize_value(value: Any, path: str, ancestors: set[int]) -> tuple[JsonValue, bool]:
    if value is None:
        return None, True
    if isinstance(value, bool):
        return value, False
    if isinstance(value, str):
        return value, False
    if isinstance(value, int):
        return value, False
    if isinstance(value, float):
        if math.isfinite(value):
            return value, False
        raise _invalid(path, "numbers must be finite")
    if isinstance(value, datetime):
        return value.isoformat(), False

    identity = id(value)
    if identity in ancestors:
        raise _invalid(path, "cyclic values are not supported")

    if isinstance(value, Mapping):
        ancestors.add(identity)
        try:
            result: dict[str, JsonValue] = {}
            for key, item in value.items():
                if not isinstance(key, str):
                    raise _invalid(path, "object keys must be strings")
                result[key], _ = _normalize_value(item, f"{path}.{key}", ancestors)
            return result, True
        finally:
            ancestors.remove(identity)

    if isinstance(value, Sequence) and not isinstance(value, (str, bytes, bytearray)):
        ancestors.add(identity)
        try:
            result: list[JsonValue] = []
            for index, item in enumerate(value):
                normalized, _ = _normalize_value(item, f"{path}[{index}]", ancestors)
                result.append(normalized)
            return result, True
        finally:
            ancestors.remove(identity)

    raise _invalid(path, f"unsupported value type {type(value).__name__}")


def _invalid(path: str, detail: str) -> InvalidContextError:
    return InvalidContextError(f"Evaluation context key {path!r} is invalid: {detail}")
