"""Local evaluation engine — shared vectors and Python/JS parity helpers."""

from __future__ import annotations

import json
import math
from pathlib import Path

import pytest

from flagship._evaluate import (
    FlagConfigError,
    _parse_iso_ms,
    evaluate_flag,
    js_number,
    js_str,
    murmurhash3,
)

_CASES_PATH = Path(__file__).resolve().parents[3] / "spec" / "local-evaluation" / "cases.json"
_SUITE = json.loads(_CASES_PATH.read_text(encoding="utf-8"))


# --- js_str / js_number parity ----------------------------------------------


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (True, "true"),
        (False, "false"),
        (1, "1"),
        (0, "0"),
        (-0.0, "0"),
        (0.0, "0"),
        (1.0, "1"),
        (1.5, "1.5"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (1e-6, "0.000001"),
        (1e20, "100000000000000000000"),
        (float("inf"), "Infinity"),
        (float("-inf"), "-Infinity"),
        (float("nan"), "NaN"),
        ("hello", "hello"),
    ],
)
def test_js_str(value: object, expected: str) -> None:
    assert js_str(value) == expected


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (True, 1.0),
        (False, 0.0),
        (42, 42.0),
        (1.5, 1.5),
        ("", float("nan")),
        ("   ", float("nan")),
        ("\t\n", float("nan")),
        ("0x10", 16.0),
        ("0o10", 8.0),
        ("0b10", 2.0),
        ("Infinity", float("inf")),
        ("-Infinity", float("-inf")),
        ("inf", float("nan")),
        ("nan", float("nan")),
        ("1_000", float("nan")),
        ("  42  ", 42.0),
        ("not-a-number", float("nan")),
    ],
)
def test_js_number(value: object, expected: float) -> None:
    got = js_number(value)
    if math.isnan(expected):
        assert math.isnan(got)
    else:
        assert got == expected


def test_js_str_bool_not_int() -> None:
    """bool is a subclass of int — must not stringify as '1'/'0'."""
    assert js_str(True) == "true"
    assert js_str(False) == "false"
    # equals via js_str forms: True != 1
    assert js_str(True) != js_str(1)


def test_murmur_lone_surrogate() -> None:
    """Lone surrogates must hash as U+FFFD bytes (EF BF BD), matching TextEncoder."""
    lone = "\ud800"
    # TextEncoder encodes lone surrogates as the replacement character.
    expected = murmurhash3("\ufffd", 0)
    assert murmurhash3(lone, 0) == expected


# --- shared vectors ---------------------------------------------------------


@pytest.mark.parametrize(
    "case",
    _SUITE["cases"],
    ids=[c["name"] for c in _SUITE["cases"]],
)
def test_shared_vector(case: dict) -> None:  # type: ignore[type-arg]
    flag_def = case["definitions"]["flags"][case["flagKey"]]
    expected = case["expected"]

    if "error" in expected:
        with pytest.raises(FlagConfigError):
            evaluate_flag(flag_def, case["context"], case["accountId"])
        return

    if "reasonOneOf" in expected:
        result = evaluate_flag(flag_def, case["context"], case["accountId"])
        assert result["reason"] in expected["reasonOneOf"]
        return

    result = evaluate_flag(flag_def, case["context"], case["accountId"])
    assert result["value"] == expected["value"]
    assert result["variant"] == expected["variant"]
    assert result["reason"] == expected["reason"]


@pytest.mark.parametrize(
    "case",
    _SUITE["murmur"],
    ids=[m["name"] for m in _SUITE["murmur"]],
)
def test_murmur_vectors(case: dict) -> None:  # type: ignore[type-arg]
    assert murmurhash3(case["input"], case["seed"]) == case["expected"]


def test_orders_infinity_via_js_number() -> None:
    flag_def = {
        "key": "inf",
        "enabled": True,
        "default_variation": "none",
        "variations": {"none": "none", "hit": "hit"},
        "rules": [
            {
                "conditions": [{"attribute": "v", "operator": "greater_than", "value": 1}],
                "serve_variation": "hit",
            }
        ],
    }
    result = evaluate_flag(flag_def, {"v": float("inf")}, "acct")
    assert result["reason"] == "TARGETING_MATCH"
    assert result["value"] == "hit"


def test_iso_date_truncates_fractional_seconds_like_date_parse() -> None:
    """Date.parse keeps only 3 fractional digits (truncate, not round).

    Without flooring, Python's microsecond-preserving parse would treat
    ``.1236789Z`` as greater than ``.123Z``, diverging from the server.
    """
    assert _parse_iso_ms("2023-01-01T00:00:00.1236789Z") == _parse_iso_ms("2023-01-01T00:00:00.123Z")
    assert _parse_iso_ms("2023-01-01T00:00:00.123999Z") == _parse_iso_ms("2023-01-01T00:00:00.123Z")
    assert _parse_iso_ms("2023-01-01T00:00:00.1234Z") == _parse_iso_ms("2023-01-01T00:00:00.123Z")
    # Fewer than 3 digits still pad as Date.parse does (via the parsed value).
    assert _parse_iso_ms("2023-01-01T00:00:00.12Z") == 1672531200120.0

    flag_def = {
        "key": "iso",
        "enabled": True,
        "default_variation": "none",
        "variations": {"none": "none", "hit": "hit"},
        "rules": [
            {
                "conditions": [
                    {
                        "attribute": "t",
                        "operator": "greater_than",
                        "value": "2023-01-01T00:00:00.123Z",
                    }
                ],
                "serve_variation": "hit",
            }
        ],
    }
    # Sub-ms extra digits must NOT make the attribute greater than the target.
    result = evaluate_flag(flag_def, {"t": "2023-01-01T00:00:00.1236789Z"}, "acct")
    assert result["reason"] == "DEFAULT"
    assert result["value"] == "none"

    # A true later millisecond still matches.
    result = evaluate_flag(flag_def, {"t": "2023-01-01T00:00:00.124Z"}, "acct")
    assert result["reason"] == "TARGETING_MATCH"
    assert result["value"] == "hit"
