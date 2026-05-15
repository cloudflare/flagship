from urllib.parse import parse_qs, urlsplit

import httpx
import pytest
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    FlagNotFoundError,
    GeneralError,
    InvalidContextError,
    ParseError,
)

from flagship import FLAGSHIP_DEFAULT_BASE_URL, FlagshipClient

ENDPOINT_REGEX = r".*/evaluate.*"


def _ok(value: object = True) -> httpx.Response:
    return httpx.Response(
        200,
        json={
            "flagKey": "k",
            "value": value,
            "variant": "on",
            "reason": "TARGETING_MATCH",
        },
    )


def _client(**overrides: object) -> FlagshipClient:
    return FlagshipClient(app_id="app-123", account_id="acct-456", **overrides)  # type: ignore[arg-type]


# --- construction -----------------------------------------------------------


def test_constructor_requires_app_id_or_endpoint() -> None:
    with pytest.raises(ValueError, match='"app_id" or "endpoint"'):
        FlagshipClient()


def test_constructor_rejects_both_app_id_and_endpoint() -> None:
    with pytest.raises(ValueError, match="not both"):
        FlagshipClient(app_id="a", account_id="b", endpoint="https://x/y")


def test_constructor_rejects_app_id_without_account_id() -> None:
    with pytest.raises(ValueError, match='"account_id"'):
        FlagshipClient(app_id="a")


def test_constructor_rejects_invalid_endpoint() -> None:
    with pytest.raises(ValueError, match="invalid endpoint URL"):
        FlagshipClient(endpoint="not-a-url")


def test_constructor_builds_default_endpoint() -> None:
    c = _client()
    assert c.endpoint == (f"{FLAGSHIP_DEFAULT_BASE_URL}/client/v4/accounts/acct-456/flagship/apps/app-123/evaluate")


def test_constructor_url_encodes_segments() -> None:
    c = FlagshipClient(app_id="my app", account_id="a/b")
    assert "accounts/a%2Fb/flagship/apps/my%20app/evaluate" in c.endpoint


def test_constructor_strips_trailing_slashes_on_base_url() -> None:
    c = FlagshipClient(app_id="a", account_id="b", base_url="https://example.com///")
    assert c.endpoint.startswith("https://example.com/client/v4/")


# --- evaluate ---------------------------------------------------------------


@respx.mock
def test_evaluate_serialises_context_into_query() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok())
    _client().evaluate(
        "dark-mode",
        EvaluationContext(targeting_key="u1", attributes={"plan": "premium"}),
    )
    q = parse_qs(urlsplit(str(route.calls[0].request.url)).query)
    assert q["flagKey"] == ["dark-mode"]
    assert q["targetingKey"] == ["u1"]
    assert q["plan"] == ["premium"]


@respx.mock
def test_evaluate_returns_response_dataclass() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok(True))
    result = _client().evaluate("k")
    assert result.flag_key == "k"
    assert result.value is True
    assert result.variant == "on"
    assert result.reason == "TARGETING_MATCH"


@respx.mock
def test_evaluate_404_raises_flag_not_found() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(404))
    with pytest.raises(FlagNotFoundError):
        _client().evaluate("k")


@respx.mock
def test_evaluate_400_raises_general() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(400))
    with pytest.raises(GeneralError):
        _client().evaluate("k")


@respx.mock
def test_evaluate_500_raises_general() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(500))
    with pytest.raises(GeneralError):
        _client().evaluate("k")


@respx.mock
def test_evaluate_invalid_json_raises_parse_error() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(200, text="not json{"))
    with pytest.raises(ParseError):
        _client().evaluate("k")


@respx.mock
def test_evaluate_missing_fields_raises_parse_error() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(200, json={"flagKey": "k"}))
    with pytest.raises(ParseError):
        _client().evaluate("k")


@respx.mock
def test_evaluate_network_error_raises_general() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(side_effect=httpx.ConnectError("x"))
    with pytest.raises(GeneralError):
        _client().evaluate("k")


@respx.mock
def test_evaluate_timeout_raises_general() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(side_effect=httpx.ReadTimeout("x"))
    with pytest.raises(GeneralError, match="timeout"):
        _client().evaluate("k")


@respx.mock
def test_evaluate_invalid_context_raises_before_fetch() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok())
    ctx = EvaluationContext(attributes={"obj": {"x": 1}})
    with pytest.raises(InvalidContextError):
        _client().evaluate("k", ctx)
    assert route.call_count == 0


# --- auth -------------------------------------------------------------------


@respx.mock
def test_auth_token_sets_bearer_header() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok())
    _client(auth_token="secret").evaluate("k")
    assert route.calls[0].request.headers["authorization"] == "Bearer secret"


@respx.mock
def test_headers_factory_is_invoked_per_request() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok())
    calls: list[int] = []

    def factory() -> dict[str, str]:
        calls.append(1)
        return {"X-Custom": "value"}

    c = _client(headers_factory=factory)
    c.evaluate("k")
    c.evaluate("k")
    assert route.calls[0].request.headers["x-custom"] == "value"
    assert len(calls) == 2


@respx.mock
def test_headers_factory_merged_with_auth_token() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok())
    _client(auth_token="secret", headers_factory=lambda: {"X-Custom": "value"}).evaluate("k")
    headers = route.calls[0].request.headers
    assert headers["authorization"] == "Bearer secret"
    assert headers["x-custom"] == "value"


@respx.mock
def test_headers_factory_overrides_auth_token() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok())
    _client(
        auth_token="secret",
        headers_factory=lambda: {"Authorization": "Basic abc"},
    ).evaluate("k")
    assert route.calls[0].request.headers["authorization"] == "Basic abc"


# --- async ------------------------------------------------------------------


@respx.mock
async def test_async_evaluate_returns_response() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_ok(True))
    c = _client()
    result = await c.evaluate_async("k")
    assert result.value is True
    await c.aclose()


@respx.mock
async def test_async_evaluate_raises_flag_not_found() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(404))
    c = _client()
    with pytest.raises(FlagNotFoundError):
        await c.evaluate_async("k")


# --- retry ------------------------------------------------------------------


@respx.mock
def test_retries_on_transient_error(monkeypatch: pytest.MonkeyPatch) -> None:
    """Transient 500 should be retried; second call succeeds."""
    monkeypatch.setattr("time.sleep", lambda _: None)
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(side_effect=[httpx.Response(500), _ok()])
    result = _client(retries=1, retry_delay=0).evaluate("k")
    assert result.value is True
    assert route.call_count == 2


@respx.mock
def test_404_is_not_retried() -> None:
    """404 is terminal and must not be retried."""
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(404))
    with pytest.raises(FlagNotFoundError):
        _client(retries=2, retry_delay=0).evaluate("k")
    assert route.call_count == 1


@respx.mock
def test_400_is_not_retried() -> None:
    """400 is terminal and must not be retried."""
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(400))
    with pytest.raises(GeneralError):
        _client(retries=2, retry_delay=0).evaluate("k")
    assert route.call_count == 1


def test_retries_capped_at_max() -> None:
    c = FlagshipClient(app_id="a", account_id="b", retries=999)
    assert c.retries == 10


def test_retry_delay_capped_at_max() -> None:
    c = FlagshipClient(app_id="a", account_id="b", retry_delay=999.0)
    assert c.retry_delay == 30.0
