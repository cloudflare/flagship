import time

import httpx
import pytest
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import FlagNotFoundError
from openfeature.flag_evaluation import Reason

from flagship import FlagshipServerProvider

ENDPOINT_REGEX = r".*/evaluate.*"


def _resp(value: object, *, reason: str = "TARGETING_MATCH", variant: str = "on") -> httpx.Response:
    return httpx.Response(200, json={"flagKey": "k", "value": value, "variant": variant, "reason": reason})


def _provider(**kwargs: object) -> FlagshipServerProvider:
    return FlagshipServerProvider(app_id="app-123", account_id="acct-456", **kwargs)  # type: ignore[arg-type]


def _ctx(user: str) -> EvaluationContext:
    return EvaluationContext(targeting_key=user)


@respx.mock
def test_caching_disabled_by_default() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider()
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert route.call_count == 2


@respx.mock
def test_cache_hit_serves_without_request_and_marks_cached() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider(cache_ttl=60)
    first = provider.resolve_boolean_details("k", False, _ctx("u1"))
    second = provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert route.call_count == 1
    assert first.reason == Reason.TARGETING_MATCH
    assert second.value is True
    assert second.variant == "on"
    assert second.reason == Reason.CACHED


@respx.mock
def test_separate_entry_per_context() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider(cache_ttl=60)
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    provider.resolve_boolean_details("k", False, _ctx("u2"))
    assert route.call_count == 2


@respx.mock
def test_cache_expires_after_ttl() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider(cache_ttl=0.05)
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    time.sleep(0.1)
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert route.call_count == 2


@respx.mock
def test_disabled_results_are_not_cached() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True, reason="DISABLED", variant=""))
    provider = _provider(cache_ttl=60)
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert route.call_count == 2


@respx.mock
def test_errors_are_not_cached() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(404))
    provider = _provider(cache_ttl=60)
    for _ in range(2):
        with pytest.raises(FlagNotFoundError):
            provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert route.call_count == 2


@respx.mock
def test_evicts_least_recently_used_beyond_max_size() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider(cache_ttl=60, cache_max_size=1)
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    provider.resolve_boolean_details("k", False, _ctx("u2"))
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert route.call_count == 3


@respx.mock
def test_shutdown_clears_cache() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider(cache_ttl=60)
    provider.resolve_boolean_details("k", False, _ctx("u1"))
    assert provider._cache is not None and len(provider._cache) == 1  # type: ignore[attr-defined]
    provider.shutdown()
    assert provider._cache is not None and len(provider._cache) == 0  # type: ignore[attr-defined]


@respx.mock
async def test_async_cache_hit() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    provider = _provider(cache_ttl=60)
    await provider.resolve_boolean_details_async("k", False, _ctx("u1"))
    second = await provider.resolve_boolean_details_async("k", False, _ctx("u1"))
    assert route.call_count == 1
    assert second.reason == Reason.CACHED
