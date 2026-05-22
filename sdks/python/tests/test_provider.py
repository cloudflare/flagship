import httpx
import pytest
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    FlagNotFoundError,
    GeneralError,
    InvalidContextError,
    ParseError,
    TypeMismatchError,
)
from openfeature.flag_evaluation import Reason

from flagship import FlagshipServerProvider

ENDPOINT_REGEX = r".*/evaluate.*"


def _resp(value: object, *, reason: str = "TARGETING_MATCH", variant: str = "on") -> httpx.Response:
    return httpx.Response(
        200,
        json={"flagKey": "k", "value": value, "variant": variant, "reason": reason},
    )


# --- lifecycle --------------------------------------------------------------


@respx.mock
def test_initialize_does_not_make_http_request(provider: FlagshipServerProvider) -> None:
    provider.initialize(EvaluationContext())
    assert not respx.calls


@respx.mock
def test_initialize_ignores_unreachable_endpoint(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(500))
    provider.initialize(EvaluationContext())
    assert not respx.calls


def test_shutdown_closes_client(provider: FlagshipServerProvider) -> None:
    provider.shutdown()


def test_metadata_name(provider: FlagshipServerProvider) -> None:
    assert provider.get_metadata().name == "Flagship Server Provider"


def test_provider_hooks_default_empty(provider: FlagshipServerProvider) -> None:
    assert provider.get_provider_hooks() == []


# --- successful resolution --------------------------------------------------


@respx.mock
@pytest.mark.parametrize(
    "method, value, default",
    [
        ("resolve_boolean_details", True, False),
        ("resolve_string_details", "v2", "default"),
        ("resolve_integer_details", 42, 0),
        ("resolve_float_details", 1.5, 0.0),
        ("resolve_object_details", {"theme": "dark"}, {}),
        ("resolve_object_details", [1, 2, 3], []),
    ],
)
def test_successful_resolution(
    provider: FlagshipServerProvider,
    method: str,
    value: object,
    default: object,
) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(value))
    details = getattr(provider, method)("k", default, EvaluationContext())
    assert details.value == value
    assert details.error_code is None
    assert details.variant == "on"
    assert details.reason == Reason.TARGETING_MATCH


# --- type checking ----------------------------------------------------------


@respx.mock
def test_boolean_rejects_string(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp("not a bool"))
    with pytest.raises(TypeMismatchError):
        provider.resolve_boolean_details("k", False)


@respx.mock
def test_integer_rejects_float(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(7.5))
    with pytest.raises(TypeMismatchError):
        provider.resolve_integer_details("k", 0)


@respx.mock
def test_integer_rejects_bool(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    with pytest.raises(TypeMismatchError):
        provider.resolve_integer_details("k", 0)


@respx.mock
def test_float_accepts_int(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(7))
    details = provider.resolve_float_details("k", 0.0)
    assert details.value == 7.0
    assert isinstance(details.value, float)


@respx.mock
def test_float_rejects_bool(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    with pytest.raises(TypeMismatchError):
        provider.resolve_float_details("k", 0.0)


@respx.mock
def test_object_rejects_string(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp("string"))
    with pytest.raises(TypeMismatchError):
        provider.resolve_object_details("k", {})


# --- DISABLED ---------------------------------------------------------------


@respx.mock
def test_disabled_returns_default_without_error(
    provider: FlagshipServerProvider,
) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True, reason="DISABLED", variant=""))
    details = provider.resolve_boolean_details("k", False)
    assert details.value is False
    assert details.error_code is None
    assert details.reason == Reason.DISABLED


# --- error propagation ------------------------------------------------------


@respx.mock
def test_404_raises_flag_not_found(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(404))
    with pytest.raises(FlagNotFoundError):
        provider.resolve_boolean_details("k", False)


@respx.mock
def test_500_raises_general(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(500))
    with pytest.raises(GeneralError):
        provider.resolve_boolean_details("k", False)


@respx.mock
def test_parse_error(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=httpx.Response(200, text="not json{"))
    with pytest.raises(ParseError):
        provider.resolve_boolean_details("k", False)


def test_invalid_context(provider: FlagshipServerProvider) -> None:
    ctx = EvaluationContext(attributes={"obj": {"x": 1}})
    with pytest.raises(InvalidContextError):
        provider.resolve_boolean_details("k", False, ctx)


# --- logging ----------------------------------------------------------------


@respx.mock
def test_logging_disabled_by_default_produces_no_output(
    provider: FlagshipServerProvider, caplog: pytest.LogCaptureFixture
) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    with caplog.at_level("DEBUG", logger="flagship"):
        provider.resolve_boolean_details("k", False)
    assert caplog.records == []


@respx.mock
def test_logging_enabled_emits_debug_records(caplog: pytest.LogCaptureFixture) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    p = FlagshipServerProvider(app_id="app-123", account_id="acct-456", logging=True)
    with caplog.at_level("DEBUG", logger="flagship"):
        p.resolve_boolean_details("k", False)
    assert any("k" in r.message for r in caplog.records)


# --- async ------------------------------------------------------------------


@respx.mock
async def test_async_resolution(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(True))
    details = await provider.resolve_boolean_details_async("k", False)
    assert details.value is True


@respx.mock
async def test_async_type_mismatch_raises(provider: FlagshipServerProvider) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp("not a bool"))
    with pytest.raises(TypeMismatchError):
        await provider.resolve_boolean_details_async("k", False)
