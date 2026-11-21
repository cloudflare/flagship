import json
import threading
import time
from collections.abc import Callable
from typing import Any

import httpx
import pytest
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.event import ProviderEvent
from openfeature.exception import (
    ErrorCode,
    FlagNotFoundError,
    InvalidContextError,
    ParseError,
    ProviderNotReadyError,
    TypeMismatchError,
)
from openfeature.flag_evaluation import Reason

from flagship import FlagshipClientProvider

ENDPOINT = "https://api.example.com/v1/acct/apps/app/evaluate"
BULK = "https://api.example.com/v1/acct/apps/app/ofrep/v1/evaluate/flags"


def flag(key: str, value: Any, variant: str = "on", reason: str = "TARGETING_MATCH") -> dict[str, Any]:
    return {"key": key, "value": value, "variant": variant, "reason": reason}


def bulk(
    flags: list[dict[str, Any]], version: str = "v1", etag: str = '"e1"', streams: list[Any] | None = None
) -> httpx.Response:
    body: dict[str, Any] = {"flags": flags, "metadata": {"version": version}}
    if streams is not None:
        body["eventStreams"] = streams
    return httpx.Response(200, json=body, headers={"etag": etag})


def sse(*events: dict[str, Any]) -> httpx.Response:
    payload = "".join(f"data: {json.dumps(event)}\n\n" for event in events)
    return httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=httpx.ByteStream(payload.encode()))


def wait_for(predicate: Callable[[], bool], timeout: float = 3.0) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() > deadline:
            raise AssertionError("condition not met")
        time.sleep(0.01)


def events_of(provider: FlagshipClientProvider) -> list[tuple[ProviderEvent, Any]]:
    seen: list[tuple[ProviderEvent, Any]] = []
    provider.attach(lambda _provider, event, details: seen.append((event, details)))
    return seen


@respx.mock
def test_initialize_caches_one_bulk_evaluation_and_resolves_without_requests() -> None:
    route = respx.post(BULK).mock(
        return_value=bulk(
            [
                flag("bool", True),
                flag("str", "hi", "greet", "DEFAULT"),
                flag("num", 3),
                {"key": "off", "reason": "DISABLED", "variant": "off"},
                {"key": "broken", "errorCode": "PARSE_ERROR", "errorDetails": "bad config"},
            ]
        )
    )
    provider = FlagshipClientProvider(endpoint=ENDPOINT, auth_token="token", change_detection="none")
    provider.initialize(EvaluationContext(targeting_key="user-1", attributes={"plan": "pro"}))

    assert json.loads(route.calls[0].request.content) == {"context": {"targetingKey": "user-1", "plan": "pro"}}
    assert route.calls[0].request.headers["authorization"] == "Bearer token"
    details = provider.resolve_boolean_details("bool", False)
    assert (details.value, details.variant, details.reason) == (True, "on", Reason.TARGETING_MATCH)
    assert provider.resolve_string_details("str", "x").value == "hi"
    assert provider.resolve_float_details("num", 0.0).value == 3.0
    disabled = provider.resolve_boolean_details("off", True)
    assert (disabled.value, disabled.reason) == (True, Reason.DISABLED)
    with pytest.raises(ParseError, match="bad config"):
        provider.resolve_boolean_details("broken", False)
    with pytest.raises(TypeMismatchError):
        provider.resolve_boolean_details("str", False)
    with pytest.raises(FlagNotFoundError):
        provider.resolve_boolean_details("missing", False)
    assert route.call_count == 1
    provider.shutdown()


def test_resolution_before_initialize_is_not_ready() -> None:
    provider = FlagshipClientProvider(endpoint=ENDPOINT)
    with pytest.raises(ProviderNotReadyError):
        provider.resolve_boolean_details("f", False)


@respx.mock
def test_generic_ofrep_base_url_headers_and_default_timeout() -> None:
    route = respx.post("https://flags.example.com/api/ofrep/v1/evaluate/flags").mock(return_value=bulk([]))
    provider = FlagshipClientProvider(
        ofrep_base_url="https://flags.example.com/api/", headers={"X-API-Key": "k"}, change_detection="none"
    )
    provider.initialize(EvaluationContext())

    assert route.calls[0].request.headers["x-api-key"] == "k"
    assert provider._client.timeout == 10.0
    with pytest.raises(ValueError, match="only one of"):
        FlagshipClientProvider(ofrep_base_url="https://flags.example.com", endpoint=ENDPOINT)
    with pytest.raises(ValueError, match="http or https"):
        FlagshipClientProvider(ofrep_base_url="ftp://flags.example.com")


@respx.mock
def test_mismatched_context_fails_closed_until_identify() -> None:
    respx.post(BULK).mock(
        side_effect=lambda request: bulk(
            [flag("f", json.loads(request.content)["context"].get("targetingKey") == "b")], etag='"x"'
        )
    )
    provider = FlagshipClientProvider(endpoint=ENDPOINT, change_detection="none")
    seen = events_of(provider)
    provider.initialize(EvaluationContext(targeting_key="a"))

    assert provider.resolve_boolean_details("f", True, EvaluationContext()).value is False
    assert provider.resolve_boolean_details("f", True, EvaluationContext(targeting_key="a")).value is False
    with pytest.raises(InvalidContextError):
        provider.resolve_boolean_details("f", True, EvaluationContext(targeting_key="b"))

    provider.identify(EvaluationContext(targeting_key="b"))

    assert provider.resolve_boolean_details("f", False, EvaluationContext(targeting_key="b")).value is True
    assert [event for event, _ in seen] == [ProviderEvent.PROVIDER_CONFIGURATION_CHANGED]
    assert seen[0][1].flags_changed == ["f"]


@respx.mock
def test_identify_is_unconditional_and_clears_the_cache_on_failure() -> None:
    route = respx.post(BULK).mock(side_effect=[bulk([flag("f", True)]), httpx.Response(500)])
    provider = FlagshipClientProvider(endpoint=ENDPOINT, change_detection="none", retries=0)
    provider.initialize(EvaluationContext(targeting_key="a"))

    with pytest.raises(Exception):
        provider.identify(EvaluationContext(targeting_key="b"))

    assert "if-none-match" not in route.calls[1].request.headers
    with pytest.raises(ProviderNotReadyError):
        provider.resolve_boolean_details("f", False)


@respx.mock
def test_polling_is_opt_in_and_revalidates_with_the_response_etag() -> None:
    route = respx.post(BULK).mock(
        side_effect=[bulk([flag("f", True)]), httpx.Response(304), bulk([flag("f", False)], etag='"e2"')]
    )
    provider = FlagshipClientProvider(endpoint=ENDPOINT, change_detection="polling", poll_interval=0.05)
    seen = events_of(provider)
    provider.initialize(EvaluationContext())
    try:
        wait_for(lambda: route.call_count >= 3)
        assert route.calls[1].request.headers["if-none-match"] == '"e1"'
        wait_for(lambda: any(event == ProviderEvent.PROVIDER_CONFIGURATION_CHANGED for event, _ in seen))
        assert provider.resolve_boolean_details("f", True).value is False
        changes = [details for event, details in seen if event == ProviderEvent.PROVIDER_CONFIGURATION_CHANGED]
        assert len(changes) == 1 and changes[0].flags_changed == ["f"]
    finally:
        provider.shutdown()

    idle = respx.post(BULK).mock(return_value=bulk([]))
    before = idle.call_count
    quiet = FlagshipClientProvider(endpoint=ENDPOINT)
    quiet.initialize(EvaluationContext())
    time.sleep(0.2)
    assert idle.call_count == before + 1
    quiet.shutdown()


@respx.mock
def test_401_during_refresh_is_fatal_and_stops_polling() -> None:
    route = respx.post(BULK).mock(side_effect=[bulk([flag("f", True)]), httpx.Response(401)])
    provider = FlagshipClientProvider(endpoint=ENDPOINT, change_detection="polling", poll_interval=0.05)
    seen = events_of(provider)
    provider.initialize(EvaluationContext())
    try:
        wait_for(lambda: any(event == ProviderEvent.PROVIDER_ERROR for event, _ in seen))
        error = next(details for event, details in seen if event == ProviderEvent.PROVIDER_ERROR)
        assert error.error_code == ErrorCode.PROVIDER_FATAL
        time.sleep(0.2)
        assert route.call_count == 2
    finally:
        provider.shutdown()


@respx.mock
def test_429_retry_after_blocks_requests() -> None:
    route = respx.post(BULK).mock(
        side_effect=[bulk([flag("f", True)]), httpx.Response(429, headers={"Retry-After": "1"}), httpx.Response(304)]
    )
    provider = FlagshipClientProvider(endpoint=ENDPOINT, change_detection="polling", poll_interval=0.05, retries=3)
    seen = events_of(provider)
    provider.initialize(EvaluationContext())
    try:
        wait_for(lambda: route.call_count == 2)
        time.sleep(0.5)
        assert route.call_count == 2
        assert any(event == ProviderEvent.PROVIDER_STALE for event, _ in seen)
        wait_for(lambda: route.call_count == 3, timeout=3)
        wait_for(lambda: any(event == ProviderEvent.PROVIDER_READY for event, _ in seen))
    finally:
        provider.shutdown()


@respx.mock
def test_sse_hint_forwards_metadata_and_coalesces_streams() -> None:
    streams = [
        {"type": "sse", "url": "https://events.example.com/a?ticket=1"},
        {"type": "websocket", "url": "wss://events.example.com/ignored"},
        {"type": "sse", "endpoint": {"requestUri": "/user-stream?ticket=2"}},
    ]
    hint = {"type": "refetchEvaluation", "etag": "v2", "lastModified": 1771622898}
    calls: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(request)
        if request.url.params.get("flagConfigEtag") == "v2":
            return bulk([flag("f", True)], version="v2", etag='"e2"')
        return bulk([flag("f", False)], streams=streams)

    respx.post(BULK).mock(side_effect=handler)
    first = respx.get("https://events.example.com/a?ticket=1").mock(return_value=sse({"type": "unknown"}, hint))
    second = respx.get("https://api.example.com/user-stream?ticket=2").mock(return_value=sse(hint))
    provider = FlagshipClientProvider(endpoint=ENDPOINT)
    observed: list[bool] = []
    provider.attach(
        lambda _p, event, _d: (
            observed.append(provider.resolve_boolean_details("f", False).value)
            if event == ProviderEvent.PROVIDER_CONFIGURATION_CHANGED
            else None
        )
    )
    try:
        provider.initialize(EvaluationContext())
        wait_for(lambda: bool(observed))
        assert observed[0] is True
        assert first.call_count >= 1 and second.call_count >= 1
        assert calls[0].headers["flagship-event-stream"] == "sse"
        hinted = [request for request in calls if request.url.params.get("flagConfigEtag") == "v2"]
        assert hinted[0].url.params["flagConfigLastModified"] == "1771622898"
        assert hinted[0].headers["if-none-match"] == '"e1"'
        assert "authorization" not in first.calls[0].request.headers
    finally:
        provider.shutdown()


@respx.mock
def test_dropped_stream_rediscovers_a_fresh_ticket_unconditionally() -> None:
    tickets = iter(range(1, 100))
    requests: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        return bulk([], streams=[{"type": "sse", "url": f"https://events.example.com/s?ticket={next(tickets)}"}])

    respx.post(BULK).mock(side_effect=handler)
    stream = respx.get(url__regex=r"https://events\.example\.com/s\?ticket=\d+").mock(side_effect=lambda request: sse())
    provider = FlagshipClientProvider(endpoint=ENDPOINT)
    try:
        provider.initialize(EvaluationContext())
        wait_for(lambda: stream.call_count >= 2)
        urls = [str(call.request.url) for call in stream.calls]
        assert urls[0].endswith("ticket=1") and urls[1].endswith("ticket=2")
        assert "if-none-match" not in requests[1].headers
        assert requests[1].headers["flagship-event-stream"] == "sse"
    finally:
        provider.shutdown()


@respx.mock
def test_pause_closes_idle_streams_and_resume_refetches_unconditionally() -> None:
    opened = threading.Event()
    requests: list[httpx.Request] = []

    class Held(httpx.SyncByteStream):
        def __init__(self) -> None:
            self.closed = threading.Event()

        def __iter__(self):
            opened.set()
            yield b": connected\n\n"
            self.closed.wait(5)

        def close(self) -> None:
            self.closed.set()

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        return bulk(
            [], streams=[{"type": "sse", "url": "https://events.example.com/s?ticket=t", "inactivityDelaySec": 1}]
        )

    respx.post(BULK).mock(side_effect=handler)
    respx.get("https://events.example.com/s?ticket=t").mock(
        side_effect=lambda request: httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=Held())
    )
    provider = FlagshipClientProvider(endpoint=ENDPOINT, inactivity_delay=0.1)
    seen = events_of(provider)
    try:
        provider.initialize(EvaluationContext())
        wait_for(opened.is_set)
        provider.pause()
        time.sleep(0.6)
        assert len(requests) == 1
        provider.resume()
        wait_for(lambda: len(requests) == 2)
        assert "if-none-match" not in requests[1].headers
        assert not any(event == ProviderEvent.PROVIDER_STALE for event, _ in seen)
    finally:
        provider.shutdown()


@respx.mock
def test_resume_without_stream_revalidates_unless_disabled() -> None:
    route = respx.post(BULK).mock(side_effect=[bulk([]), httpx.Response(304), bulk([])])
    provider = FlagshipClientProvider(endpoint=ENDPOINT)
    provider.initialize(EvaluationContext())
    provider.resume()
    assert route.calls[1].request.headers["if-none-match"] == '"e1"'

    disabled = FlagshipClientProvider(endpoint=ENDPOINT, change_detection="none")
    disabled.initialize(EvaluationContext())
    disabled.resume()
    assert route.call_count == 3
