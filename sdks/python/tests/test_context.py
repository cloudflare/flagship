from datetime import datetime, timezone

import pytest
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import InvalidContextError

from flagship.context import context_to_query_params


def test_primitives_are_serialised() -> None:
    ctx = EvaluationContext(
        targeting_key="u1",
        attributes={
            "name": "alice",
            "age": 30,
            "score": 1.5,
            "premium": True,
            "trial": False,
        },
    )
    params = context_to_query_params(ctx)
    assert params == {
        "targetingKey": "u1",
        "name": "alice",
        "age": "30",
        "score": "1.5",
        "premium": "true",
        "trial": "false",
    }


def test_none_values_are_skipped() -> None:
    ctx = EvaluationContext(targeting_key=None, attributes={"a": None, "b": "x"})
    assert context_to_query_params(ctx) == {"b": "x"}


def test_datetime_is_serialised_as_iso() -> None:
    dt = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    ctx = EvaluationContext(attributes={"signed_up": dt})
    assert context_to_query_params(ctx)["signed_up"] == dt.isoformat()


def test_complex_value_raises_invalid_context() -> None:
    ctx = EvaluationContext(attributes={"obj": {"nested": 1}})
    with pytest.raises(InvalidContextError):
        context_to_query_params(ctx)


def test_list_raises_invalid_context() -> None:
    ctx = EvaluationContext(attributes={"arr": [1, 2, 3]})
    with pytest.raises(InvalidContextError):
        context_to_query_params(ctx)


def test_none_context_returns_empty() -> None:
    assert context_to_query_params(None) == {}


def test_zero_and_empty_string() -> None:
    ctx = EvaluationContext(attributes={"n": 0, "s": ""})
    assert context_to_query_params(ctx) == {"n": "0", "s": ""}
