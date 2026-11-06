from datetime import datetime, timezone

import pytest
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import InvalidContextError

from flagship.context import context_to_query_params, normalize_context


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


def test_none_values_require_post_and_are_preserved() -> None:
    ctx = EvaluationContext(targeting_key=None, attributes={"a": None, "b": "x"})
    assert normalize_context(ctx).values == {"a": None, "b": "x"}
    assert normalize_context(ctx).requires_post is True


def test_datetime_is_serialised_as_iso() -> None:
    dt = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    ctx = EvaluationContext(attributes={"signed_up": dt})
    assert context_to_query_params(ctx)["signed_up"] == dt.isoformat()


def test_nested_objects_arrays_and_dates_are_normalised() -> None:
    dt = datetime(2024, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    ctx = EvaluationContext(
        attributes={
            "profile": {"account": {"plan": "enterprise", "created_at": dt}},
            "tags": ["beta", 42, True, None],
        }
    )

    normalized = normalize_context(ctx)

    assert normalized.requires_post is True
    assert normalized.values == {
        "profile": {"account": {"plan": "enterprise", "created_at": dt.isoformat()}},
        "tags": ["beta", 42, True, None],
    }


def test_cyclic_context_raises_invalid_context() -> None:
    profile: dict[str, object] = {}
    profile["self"] = profile
    with pytest.raises(InvalidContextError):
        normalize_context(EvaluationContext(attributes={"profile": profile}))


def test_non_finite_context_number_raises_invalid_context() -> None:
    with pytest.raises(InvalidContextError):
        normalize_context(EvaluationContext(attributes={"score": float("nan")}))


def test_none_context_returns_empty() -> None:
    assert context_to_query_params(None) == {}


def test_zero_and_empty_string() -> None:
    ctx = EvaluationContext(attributes={"n": 0, "s": ""})
    assert context_to_query_params(ctx) == {"n": "0", "s": ""}
