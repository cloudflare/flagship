import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

import httpx
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.event import ProviderEvent

from flagship import FlagshipServerProvider
from flagship.sse import parse_sse_lines, stream_url


def test_sse_parser_ignores_unknown_events_and_comments() -> None:
    lines = [
        ": keepalive",
        "",
        "event: other",
        'data: {"type":"refetchEvaluation","etag":"ignored"}',
        "",
        'data: {"type":"refetchEvaluation",',
        'data: "etag":"v2"}',
        "",
    ]
    assert list(parse_sse_lines(lines)) == [{"type": "refetchEvaluation", "etag": "v2"}]


def test_stream_url_rejects_insecure_descriptors() -> None:
    assert (
        stream_url(
            [
                {"type": "other", "url": "https://example.com/no"},
                {"type": "sse", "url": "https://events.example.com/sse?ticket=t"},
            ],
            "https://api.example.com/definitions",
        )
        == "https://events.example.com/sse?ticket=t"
    )
    assert stream_url([{"type": "sse", "url": "http://example.com/stream"}], "https://api.example.com") is None


@respx.mock
def test_local_sse_refreshes_before_configuration_event() -> None:
    v1, v2 = "a" * 64, "b" * 64
    snapshots: dict[str, dict[str, Any]] = {
        v1: {
            "flags": {
                "switch": {
                    "key": "switch",
                    "enabled": True,
                    "default_variation": "on",
                    "variations": {"on": False},
                    "rules": [],
                }
            }
        },
        v2: {
            "flags": {
                "switch": {
                    "key": "switch",
                    "enabled": True,
                    "default_variation": "on",
                    "variations": {"on": True},
                    "rules": [],
                }
            }
        },
    }
    discovery = respx.get(url__regex=r".*/definitions.*")

    def definitions(request: httpx.Request) -> httpx.Response:
        version = v2 if request.url.params.get("flagConfigEtag") == v2 else v1
        body = {**snapshots[version], "metadata": {"version": version}}
        if request.headers.get("flagship-event-stream") == "sse":
            body["eventStreams"] = [{"type": "sse", "url": "https://events.example.com/stream?ticket=one"}]
        return httpx.Response(200, json=body, headers={"etag": version})

    discovery.mock(side_effect=definitions)
    stream = respx.get("https://events.example.com/stream?ticket=one").mock(
        return_value=httpx.Response(
            200,
            headers={"content-type": "text/event-stream"},
            stream=httpx.ByteStream(f': connected\n\ndata: {{"type":"refetchEvaluation","etag":"{v2}"}}\n\n'.encode()),
        )
    )
    provider = FlagshipServerProvider(
        endpoint="https://api.example.com/v1/acct/apps/app/evaluate",
        account_id="acct",
        auth_token="test-token",
        local_evaluation=True,
        configuration_events=True,
        refresh_interval=60,
        retries=0,
    )
    changes: list[bool] = []
    provider.attach(
        lambda _provider, event, _details: (
            changes.append(provider.resolve_boolean_details("switch", False).value)
            if event == ProviderEvent.PROVIDER_CONFIGURATION_CHANGED
            else None
        )
    )
    try:
        provider.initialize(EvaluationContext())
        deadline = time.monotonic() + 3
        while not changes and time.monotonic() < deadline:
            time.sleep(0.01)
        assert changes == [True]
        assert stream.call_count >= 1
        assert "authorization" not in stream.calls[0].request.headers
        assert any(call.request.url.params.get("flagConfigEtag") == v2 for call in discovery.calls)
    finally:
        provider.shutdown()


@respx.mock
def test_remote_sse_invalidates_cache_and_reconnects_with_fresh_ticket() -> None:
    v1, v2 = "c" * 64, "d" * 64
    current = 1
    discovery_count = 0
    publish = threading.Event()
    bulk = respx.post(url__regex=r".*/ofrep/v1/evaluate/flags.*")

    def discover(request: httpx.Request) -> httpx.Response:
        nonlocal discovery_count
        if request.headers.get("flagship-event-stream") == "sse":
            discovery_count += 1
            return httpx.Response(
                200,
                json={
                    "metadata": {"version": v1 if discovery_count == 1 else v2},
                    "eventStreams": [
                        {"type": "sse", "url": f"https://events.example.com/stream?ticket={discovery_count}"}
                    ],
                },
            )
        return httpx.Response(200, json={"metadata": {"version": v2 if current == 2 else v1}, "flags": []})

    bulk.mock(side_effect=discover)
    evaluation = respx.get(url__regex=r".*/evaluate.*")
    evaluation.mock(
        side_effect=lambda _request: httpx.Response(
            200, json={"flagKey": "switch", "value": current == 2, "reason": "DEFAULT"}
        )
    )

    def stream_response(request: httpx.Request) -> httpx.Response:
        class EventStream(httpx.SyncByteStream):
            def __iter__(self):
                yield b": connected\n\n"
                if request.url.params.get("ticket") == "1" and publish.wait(3):
                    yield f'data: {{"type":"refetchEvaluation","etag":"{v2}"}}\n\n'.encode()

        return httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=EventStream())

    stream = respx.get(url__regex=r".*/stream.*").mock(side_effect=stream_response)
    provider = FlagshipServerProvider(
        endpoint="https://api.example.com/v1/acct/apps/app/evaluate",
        auth_token="test-token",
        cache_ttl=60,
        configuration_events=True,
        retries=0,
    )
    refreshed: list[bool] = []
    provider.attach(
        lambda _provider, event, _details: (
            refreshed.append(provider.resolve_boolean_details("switch", False).value)
            if event == ProviderEvent.PROVIDER_CONFIGURATION_CHANGED
            else None
        )
    )
    try:
        provider.initialize(EvaluationContext())
        deadline = time.monotonic() + 3
        while stream.call_count < 1 and time.monotonic() < deadline:
            time.sleep(0.01)
        assert not provider.resolve_boolean_details("switch", False).value
        assert not provider.resolve_boolean_details("switch", False).value
        assert evaluation.call_count == 1
        current = 2
        publish.set()
        deadline = time.monotonic() + 3
        while not refreshed and time.monotonic() < deadline:
            time.sleep(0.01)
        assert refreshed == [True]
        assert evaluation.call_count == 2
        assert any(call.request.url.params.get("flagConfigEtag") == v2 for call in bulk.calls)
        while stream.call_count < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        assert stream.call_count >= 2
        assert discovery_count >= 2
        assert "authorization" not in stream.calls[0].request.headers
    finally:
        provider.shutdown()


@respx.mock
def test_reconnect_catches_change_without_a_hint() -> None:
    v1, v2 = "e" * 64, "f" * 64
    current = 1
    discovery_count = 0

    def definitions(request: httpx.Request) -> httpx.Response:
        nonlocal discovery_count
        if request.headers.get("flagship-event-stream") == "sse":
            discovery_count += 1
        version = v2 if current == 2 else v1
        return httpx.Response(
            200,
            json={
                "flags": {
                    "switch": {
                        "key": "switch",
                        "enabled": True,
                        "default_variation": "on",
                        "variations": {"on": current == 2},
                        "rules": [],
                    }
                },
                "metadata": {"version": version},
                "eventStreams": [{"type": "sse", "url": f"https://events.example.com/stream?ticket={discovery_count}"}],
            },
            headers={"etag": version},
        )

    respx.get(url__regex=r".*/definitions.*").mock(side_effect=definitions)

    def first_stream(request: httpx.Request) -> httpx.Response:
        class DropStream(httpx.SyncByteStream):
            def __iter__(self):
                nonlocal current
                yield b": connected\n\n"
                if request.url.params.get("ticket") == "1":
                    current = 2

        return httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=DropStream())

    stream = respx.get(url__regex=r".*/stream.*").mock(side_effect=first_stream)
    provider = FlagshipServerProvider(
        endpoint="https://api.example.com/v1/acct/apps/app/evaluate",
        account_id="acct",
        local_evaluation=True,
        configuration_events=True,
        refresh_interval=60,
        retries=0,
    )
    changes: list[bool] = []
    provider.attach(
        lambda _provider, event, _details: (
            changes.append(provider.resolve_boolean_details("switch", False).value)
            if event == ProviderEvent.PROVIDER_CONFIGURATION_CHANGED
            else None
        )
    )
    try:
        provider.initialize(EvaluationContext())
        deadline = time.monotonic() + 3
        while not changes and time.monotonic() < deadline:
            time.sleep(0.01)
        assert changes == [True]
        while stream.call_count < 2 and time.monotonic() < deadline:
            time.sleep(0.01)
        assert stream.call_count >= 2
    finally:
        provider.shutdown()


def test_shutdown_closes_idle_sse_connection() -> None:
    opened = threading.Event()
    release = threading.Event()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            if self.path == "/definitions":
                body = json.dumps(
                    {
                        "flags": {},
                        "metadata": {"version": "a" * 64},
                        "eventStreams": [
                            {"type": "sse", "url": f"http://127.0.0.1:{self.server.server_port}/stream?ticket=one"}
                        ],
                    }
                ).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("ETag", "a" * 64)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            elif self.path.startswith("/stream"):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                self.wfile.write(b": connected\n\n")
                self.wfile.flush()
                opened.set()
                release.wait(5)

        def log_message(self, *args: Any) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    provider = FlagshipServerProvider(
        endpoint=f"http://127.0.0.1:{server.server_port}/evaluate",
        account_id="acct",
        local_evaluation=True,
        configuration_events=True,
        refresh_interval=60,
        retries=0,
    )
    try:
        provider.initialize(EvaluationContext())
        assert opened.wait(2)
        event_thread = provider._event_thread
        start = time.monotonic()
        provider.shutdown()
        assert time.monotonic() - start < 2
        assert event_thread is not None and not event_thread.is_alive()
    finally:
        release.set()
        server.shutdown()
        server.server_close()


@respx.mock
def test_deleted_configuration_reports_error_without_change_event() -> None:
    deleted = False

    def definitions(request: httpx.Request) -> httpx.Response:
        if deleted:
            return httpx.Response(404)
        body = {"flags": {}, "metadata": {"version": "a" * 64}}
        if request.headers.get("flagship-event-stream") == "sse":
            body["eventStreams"] = [{"type": "sse", "url": "https://events.example.com/stream?ticket=one"}]
        return httpx.Response(200, json=body, headers={"etag": "a" * 64})

    respx.get(url__regex=r".*/definitions.*").mock(side_effect=definitions)

    class DeletedStream(httpx.SyncByteStream):
        def __iter__(self):
            nonlocal deleted
            yield b": connected\n\n"
            deleted = True
            yield b'data: {"type":"refetchEvaluation"}\n\n'

    respx.get("https://events.example.com/stream?ticket=one").mock(
        return_value=httpx.Response(200, headers={"content-type": "text/event-stream"}, stream=DeletedStream())
    )
    provider = FlagshipServerProvider(
        endpoint="https://api.example.com/v1/acct/apps/app/evaluate",
        account_id="acct",
        local_evaluation=True,
        configuration_events=True,
        refresh_interval=60,
        retries=0,
    )
    received: list[ProviderEvent] = []
    provider.attach(lambda _provider, event, _details: received.append(event))
    try:
        provider.initialize(EvaluationContext())
        deadline = time.monotonic() + 3
        while ProviderEvent.PROVIDER_ERROR not in received and time.monotonic() < deadline:
            time.sleep(0.01)
        assert ProviderEvent.PROVIDER_ERROR in received
        assert ProviderEvent.PROVIDER_CONFIGURATION_CHANGED not in received
        assert provider._snapshot is None
    finally:
        provider.shutdown()
