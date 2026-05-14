"""End-to-end tests via the OpenFeature API."""

import httpx
import respx
from openfeature import api
from openfeature.evaluation_context import EvaluationContext

from flagship import FlagshipServerProvider


def _resp(value: object) -> httpx.Response:
    return httpx.Response(
        200,
        json={
            "flagKey": "k",
            "value": value,
            "variant": "on",
            "reason": "TARGETING_MATCH",
        },
    )


@respx.mock
def test_set_provider_and_evaluate_boolean() -> None:
    respx.get(url__regex=r".*/evaluate.*").mock(return_value=_resp(True))
    api.set_provider(FlagshipServerProvider(app_id="a", account_id="b"))
    client = api.get_client()
    value = client.get_boolean_value("dark-mode", False, EvaluationContext(targeting_key="u1"))
    assert value is True


@respx.mock
def test_get_boolean_details_returns_metadata() -> None:
    respx.get(url__regex=r".*/evaluate.*").mock(return_value=_resp(True))
    api.set_provider(FlagshipServerProvider(app_id="a", account_id="b"))
    client = api.get_client()
    details = client.get_boolean_details("k", False, EvaluationContext(targeting_key="u1"))
    assert details.value is True
    assert details.variant == "on"
    assert details.error_code is None


@respx.mock
def test_flag_not_found_returns_default_via_api() -> None:
    """SDK catches FlagNotFoundError and returns default with error_code."""
    respx.get(url__regex=r".*/evaluate.*").mock(return_value=httpx.Response(404))
    api.set_provider(FlagshipServerProvider(app_id="a", account_id="b"))
    client = api.get_client()
    details = client.get_boolean_details("missing", False, EvaluationContext())
    assert details.value is False
    assert details.error_code is not None
