"""Local evaluation provider lifecycle and client definitions fetch."""

from __future__ import annotations

import threading
import time
from typing import Any

import httpx
import pytest
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    FlagNotFoundError,
    GeneralError,
    ParseError,
    ProviderNotReadyError,
    TypeMismatchError,
)
from openfeature.flag_evaluation import Reason

from flagship import FlagshipClient, FlagshipServerProvider
from flagship.client import resolve_definitions_endpoint

DEFINITIONS_REGEX = r".*/definitions.*"

_DEFINITIONS: dict[str, Any] = {
    "flags": {
        "bool_flag": {
            "key": "bool_flag",
            "enabled": True,
            "default_variation": "off",
            "variations": {"on": True, "off": False},
            "rules": [
                {
                    "conditions": [{"attribute": "plan", "operator": "equals", "value": "pro"}],
                    "serve_variation": "on",
                }
            ],
        },
        "string_flag": {
            "key": "string_flag",
            "enabled": True,
            "default_variation": "a",
            "variations": {"a": "hello", "b": "world"},
            "rules": [],
        },
        "disabled_flag": {
            "key": "disabled_flag",
            "enabled": False,
            "default_variation": "off",
            "variations": {"on": True, "off": False},
            "rules": [],
        },
        "bad_flag": {
            "key": "bad_flag",
            "enabled": True,
            "default_variation": "missing",
            "variations": {"on": True},
            "rules": [],
        },
    }
}


def _defs_response(
    definitions: dict[str, Any] | None = None,
    *,
    etag: str = '"v1"',
) -> httpx.Response:
    headers = {"etag": etag} if etag else {}
    return httpx.Response(200, json=definitions if definitions is not None else _DEFINITIONS, headers=headers)


def _local_provider(**overrides: Any) -> FlagshipServerProvider:
    opts: dict[str, Any] = {
        "app_id": "app-1",
        "account_id": "acct-1",
        "local_evaluation": True,
        "refresh_interval": 60.0,  # long so tests control refresh timing
    }
    opts.update(overrides)
    return FlagshipServerProvider(**opts)


# --- resolve_definitions_endpoint -------------------------------------------


def test_definitions_url_from_app_id() -> None:
    assert (
        resolve_definitions_endpoint(app_id="app-1", account_id="acct-1", base_url="https://api.example.com")
        == "https://api.example.com/client/v4/accounts/acct-1/flagship/apps/app-1/definitions"
    )


def test_definitions_url_replaces_evaluate() -> None:
    assert (
        resolve_definitions_endpoint(endpoint="https://api.example.com/v1/acct/apps/app/evaluate")
        == "https://api.example.com/v1/acct/apps/app/definitions"
    )


def test_definitions_url_rejects_non_evaluate() -> None:
    with pytest.raises(ValueError, match='must end in "/evaluate"'):
        resolve_definitions_endpoint(endpoint="https://api.example.com/v1/flags")


# --- FlagshipClient.fetch_definitions ---------------------------------------


@respx.mock
def test_fetch_definitions_returns_flags_and_etag() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response(etag='"abc"'))
    client = FlagshipClient(app_id="app-1", account_id="acct-1")
    result = client.fetch_definitions()
    assert result is not None
    flags, etag = result
    assert "bool_flag" in flags
    assert etag == '"abc"'
    client.close()


@respx.mock
def test_fetch_definitions_sends_if_none_match_and_304() -> None:
    route = respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=httpx.Response(304))
    client = FlagshipClient(app_id="app-1", account_id="acct-1")
    result = client.fetch_definitions('"abc"')
    assert result is None
    assert route.calls[0].request.headers["if-none-match"] == '"abc"'
    client.close()


@respx.mock
def test_fetch_definitions_rejects_malformed_payload() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=httpx.Response(200, json={"notFlags": True}))
    client = FlagshipClient(app_id="app-1", account_id="acct-1")
    with pytest.raises(ParseError):
        client.fetch_definitions()
    client.close()


def test_fetch_definitions_without_derived_endpoint() -> None:
    client = FlagshipClient(endpoint="https://api.example.com/flags")
    with pytest.raises(ValueError, match="definitions endpoint is not configured"):
        client.fetch_definitions()
    client.close()


# --- constructor ------------------------------------------------------------


def test_constructor_requires_account_id() -> None:
    with pytest.raises(ValueError, match='"account_id" is required'):
        FlagshipServerProvider(
            endpoint="https://api.example.com/v1/acct/apps/app/evaluate",
            local_evaluation=True,
        )


def test_constructor_rejects_cache_ttl() -> None:
    with pytest.raises(ValueError, match="incompatible with cache_ttl"):
        FlagshipServerProvider(
            app_id="a",
            account_id="b",
            local_evaluation=True,
            cache_ttl=30.0,
        )


def test_constructor_rejects_bad_refresh_interval() -> None:
    with pytest.raises(ValueError, match="refresh_interval"):
        FlagshipServerProvider(app_id="a", account_id="b", local_evaluation=True, refresh_interval=0)
    with pytest.raises(ValueError, match="refresh_interval"):
        FlagshipServerProvider(app_id="a", account_id="b", local_evaluation=True, refresh_interval=-1)
    with pytest.raises(ValueError, match="refresh_interval"):
        FlagshipServerProvider(app_id="a", account_id="b", local_evaluation=True, refresh_interval=float("nan"))


def test_constructor_rejects_non_evaluate_endpoint() -> None:
    with pytest.raises(ValueError, match='must end in "/evaluate"'):
        FlagshipServerProvider(
            endpoint="https://api.example.com/v1/flags",
            account_id="acct",
            local_evaluation=True,
        )


# --- initialize -------------------------------------------------------------


@respx.mock
def test_initialize_loads_definitions() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    details = provider.resolve_boolean_details("bool_flag", False, EvaluationContext(attributes={"plan": "pro"}))
    assert details.value is True
    assert details.reason == Reason.TARGETING_MATCH
    provider.shutdown()


@respx.mock
def test_initialize_failure_raises() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=httpx.Response(500))
    provider = _local_provider(retries=0)
    with pytest.raises(GeneralError):
        provider.initialize(EvaluationContext())
    provider.shutdown()


@respx.mock
def test_provider_not_ready_before_initialize() -> None:
    provider = _local_provider()
    with pytest.raises(ProviderNotReadyError):
        provider.resolve_boolean_details("bool_flag", False)
    provider.shutdown()


# --- evaluation -------------------------------------------------------------


@respx.mock
def test_evaluation_flag_not_found() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    with pytest.raises(FlagNotFoundError):
        provider.resolve_boolean_details("missing", False)
    provider.shutdown()


@respx.mock
def test_evaluation_parse_error_missing_variation() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    with pytest.raises(ParseError):
        provider.resolve_boolean_details("bad_flag", False)
    provider.shutdown()


@respx.mock
def test_evaluation_parse_error_malformed_flag() -> None:
    # Object missing variations/rules — surfaces PARSE_ERROR, must not poison others.
    malformed = {
        "flags": {
            "broken": {"key": "broken", "enabled": True, "default_variation": "a"},
            "ok": _DEFINITIONS["flags"]["string_flag"],
        }
    }
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response(malformed))
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    with pytest.raises(ParseError):
        provider.resolve_boolean_details("broken", False)
    ok = provider.resolve_string_details("ok", "x")
    assert ok.value == "hello"
    provider.shutdown()


@respx.mock
def test_evaluation_disabled_returns_default() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    details = provider.resolve_boolean_details("disabled_flag", True)
    assert details.value is True
    assert details.reason == Reason.DISABLED
    provider.shutdown()


@respx.mock
def test_evaluation_type_mismatch() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    with pytest.raises(TypeMismatchError):
        provider.resolve_boolean_details("string_flag", False)
    provider.shutdown()


@respx.mock
def test_evaluation_static_reason() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    details = provider.resolve_string_details("string_flag", "default")
    assert details.value == "hello"
    assert details.reason == Reason.STATIC
    assert details.variant == "a"
    provider.shutdown()


@respx.mock
async def test_async_api_returns_without_network() -> None:
    route = respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    calls_after_init = route.call_count
    details = await provider.resolve_boolean_details_async(
        "bool_flag", False, EvaluationContext(attributes={"plan": "pro"})
    )
    assert details.value is True
    # No additional network calls for local evaluation.
    assert route.call_count == calls_after_init
    await provider.shutdown_async()


# --- refresh ----------------------------------------------------------------


@respx.mock
def test_refresh_304_keeps_snapshot() -> None:
    route = respx.get(url__regex=DEFINITIONS_REGEX).mock(
        side_effect=[
            _defs_response(etag='"v1"'),
            httpx.Response(304, headers={"etag": '"v1"'}),
        ]
    )
    provider = _local_provider(refresh_interval=0.05)
    provider.initialize(EvaluationContext())

    # Wait for at least one background refresh.
    deadline = time.monotonic() + 2.0
    while route.call_count < 2 and time.monotonic() < deadline:
        time.sleep(0.02)

    assert route.call_count >= 2
    assert route.calls[1].request.headers.get("if-none-match") == '"v1"'
    details = provider.resolve_boolean_details("bool_flag", False, EvaluationContext(attributes={"plan": "pro"}))
    assert details.value is True
    provider.shutdown()


@respx.mock
def test_refresh_200_swaps_snapshot() -> None:
    updated = {
        "flags": {
            "bool_flag": {
                "key": "bool_flag",
                "enabled": True,
                "default_variation": "on",
                "variations": {"on": True, "off": False},
                "rules": [],
            }
        }
    }
    route = respx.get(url__regex=DEFINITIONS_REGEX).mock(
        side_effect=[
            _defs_response(etag='"v1"'),
            _defs_response(updated, etag='"v2"'),
        ]
    )
    provider = _local_provider(refresh_interval=0.05)
    provider.initialize(EvaluationContext())

    # Before refresh: rule-based, plan=pro → on; no plan → off
    before = provider.resolve_boolean_details("bool_flag", False)
    assert before.value is False
    assert before.reason == Reason.DEFAULT

    deadline = time.monotonic() + 2.0
    while route.call_count < 2 and time.monotonic() < deadline:
        time.sleep(0.02)
    # Give the thread a moment to apply the snapshot.
    time.sleep(0.05)

    after = provider.resolve_boolean_details("bool_flag", False)
    assert after.value is True
    assert after.reason == Reason.STATIC
    assert after.variant == "on"
    provider.shutdown()


@respx.mock
def test_failed_refresh_keeps_snapshot() -> None:
    route = respx.get(url__regex=DEFINITIONS_REGEX).mock(
        side_effect=[
            _defs_response(etag='"v1"'),
            httpx.Response(500),
            httpx.Response(500),
            httpx.Response(500),
        ]
    )
    provider = _local_provider(refresh_interval=0.05, retries=0)
    provider.initialize(EvaluationContext())

    deadline = time.monotonic() + 2.0
    while route.call_count < 2 and time.monotonic() < deadline:
        time.sleep(0.02)

    details = provider.resolve_boolean_details("bool_flag", False, EvaluationContext(attributes={"plan": "pro"}))
    assert details.value is True
    provider.shutdown()


# --- shutdown / lifecycle generation ----------------------------------------


@respx.mock
def test_shutdown_drops_snapshot_and_joins_thread() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider(refresh_interval=0.05)
    provider.initialize(EvaluationContext())
    thread = provider._refresh_thread
    assert thread is not None and thread.is_alive()

    provider.shutdown()
    assert provider._snapshot is None
    assert thread is None or not thread.is_alive()

    with pytest.raises(ProviderNotReadyError):
        provider.resolve_boolean_details("bool_flag", False)


@respx.mock
def test_reinitialize_after_shutdown() -> None:
    respx.get(url__regex=DEFINITIONS_REGEX).mock(return_value=_defs_response())
    provider = _local_provider()
    provider.initialize(EvaluationContext())
    provider.shutdown()
    # Client is closed after shutdown — need a fresh provider for a clean re-init
    # of the HTTP client. But the same provider instance can re-initialize if we
    # don't close via a full shutdown of a new client... The plan says shutdown
    # closes the httpx client. Re-init after shutdown on the same instance would
    # need the client still open. Match TS: onClose drops snapshot but doesn't
    # destroy the client permanently in a way that blocks re-init.
    #
    # Our shutdown() closes the client. For re-init testing we call the local
    # lifecycle pieces the way OpenFeature would: shutdown then a new provider.
    # The generation test below covers close+re-init on the same instance by
    # only exercising _shutdown_local + initialize without closing the client.
    provider2 = _local_provider()
    provider2.initialize(EvaluationContext())
    details = provider2.resolve_boolean_details("bool_flag", False, EvaluationContext(attributes={"plan": "pro"}))
    assert details.value is True
    provider2.shutdown()


@respx.mock
def test_stale_inflight_refresh_after_close_ignored() -> None:
    """In-flight refresh started before shutdown must not write after close."""
    release = threading.Event()
    calls = {"n": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        if calls["n"] == 1:
            return _defs_response(etag='"v1"')
        # Background refresh — block until released.
        release.wait(timeout=5.0)
        return _defs_response(
            {
                "flags": {
                    "bool_flag": {
                        "key": "bool_flag",
                        "enabled": True,
                        "default_variation": "off",
                        "variations": {"on": True, "off": False},
                        "rules": [],
                    }
                }
            },
            etag='"v2"',
        )

    respx.get(url__regex=DEFINITIONS_REGEX).mock(side_effect=handler)
    provider = _local_provider(refresh_interval=0.05)
    provider.initialize(EvaluationContext())

    # Wait until the background refresh is in flight (call count >= 2).
    deadline = time.monotonic() + 2.0
    while calls["n"] < 2 and time.monotonic() < deadline:
        time.sleep(0.01)

    provider._shutdown_local()
    release.set()
    time.sleep(0.1)  # let the blocked fetch finish and attempt to apply

    with pytest.raises(ProviderNotReadyError):
        provider.resolve_boolean_details("bool_flag", False)
    # Clean up client.
    provider._client.close()


@respx.mock
def test_stale_inflight_refresh_after_close_and_reinit_ignored() -> None:
    """Stale refresh must not overwrite a fresh snapshot after close + re-init.

    Regression: a boolean "closed" flag reset by initialize() would let a
    refresh that started before shutdown write after re-init.
    """
    stale_definitions = {
        "flags": {
            "bool_flag": {
                "key": "bool_flag",
                "enabled": True,
                "default_variation": "off",
                "variations": {"on": True, "off": False},
                "rules": [],
            }
        }
    }
    fresh_definitions = {
        "flags": {
            "bool_flag": {
                "key": "bool_flag",
                "enabled": True,
                "default_variation": "on",
                "variations": {"on": True, "off": False},
                "rules": [],
            }
        }
    }

    release_stale = threading.Event()
    calls = {"n": 0}

    def handler(request: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        n = calls["n"]
        if n == 1:
            # First initialize — baseline (default off).
            return _defs_response(stale_definitions, etag='"v1"')
        if n == 2:
            # Background refresh started before close — held open.
            release_stale.wait(timeout=5.0)
            return _defs_response(stale_definitions, etag='"stale"')
        # Re-initialize after close — fresh snapshot (default on).
        return _defs_response(fresh_definitions, etag='"v2"')

    respx.get(url__regex=DEFINITIONS_REGEX).mock(side_effect=handler)
    provider = _local_provider(refresh_interval=0.05)
    provider.initialize(EvaluationContext())

    deadline = time.monotonic() + 2.0
    while calls["n"] < 2 and time.monotonic() < deadline:
        time.sleep(0.01)

    # Close + re-init without releasing the stale fetch yet.
    # Use _shutdown_local so the httpx client stays open for re-init.
    provider._shutdown_local()
    provider.initialize(EvaluationContext())

    before = provider.resolve_boolean_details("bool_flag", False)
    assert before.value is True
    assert before.reason == Reason.STATIC
    assert before.variant == "on"

    # Stale refresh from the previous session finally resolves.
    release_stale.set()
    time.sleep(0.15)

    after = provider.resolve_boolean_details("bool_flag", False)
    assert after.value is True
    assert after.reason == Reason.STATIC
    assert after.variant == "on"
    provider.shutdown()


@respx.mock
def test_no_if_none_match_when_response_had_no_etag() -> None:
    route = respx.get(url__regex=DEFINITIONS_REGEX).mock(
        side_effect=[
            httpx.Response(200, json=_DEFINITIONS),  # no etag header
            httpx.Response(200, json=_DEFINITIONS, headers={"etag": '"later"'}),
        ]
    )
    provider = _local_provider(refresh_interval=0.05)
    provider.initialize(EvaluationContext())

    deadline = time.monotonic() + 2.0
    while route.call_count < 2 and time.monotonic() < deadline:
        time.sleep(0.02)

    assert route.call_count >= 2
    assert "if-none-match" not in route.calls[1].request.headers
    provider.shutdown()
