from collections.abc import Mapping
from datetime import datetime
from typing import Any

from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import InvalidContextError

__all__ = ["context_to_query_params"]


def context_to_query_params(context: EvaluationContext | None) -> dict[str, str]:
    """Serialise an EvaluationContext to URL query parameters.

    Only primitive values (str, int, float, bool, datetime) are supported.
    Complex values (dict, list) raise InvalidContextError.
    """
    if context is None:
        return {}

    params: dict[str, str] = {}

    if context.targeting_key is not None:
        params["targetingKey"] = str(context.targeting_key)

    for key, value in context.attributes.items():
        if value is None:
            continue
        if isinstance(value, datetime):
            params[key] = value.isoformat()
        elif isinstance(value, bool):
            params[key] = "true" if value else "false"
        elif isinstance(value, (int, float, str)):
            params[key] = str(value)
        elif isinstance(value, (Mapping, list, tuple)) or not _is_primitive(value):
            raise InvalidContextError(
                f"Context attribute {key!r} has unsupported type {type(value).__name__}; "
                f"use str, int, float, bool, or datetime"
            )

    return params


def _is_primitive(value: Any) -> bool:
    return isinstance(value, (str, int, float, bool, datetime))
