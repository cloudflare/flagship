import math

import httpx
import pytest
import respx
from openfeature.evaluation_context import EvaluationContext
from openfeature.flag_evaluation import Reason

from flagship import FlagshipServerProvider
from flagship._evaluate import evaluate_flag
from flagship._metadata import to_flag_metadata

METADATA = {"owner": "team-payments", "jira.id": "PROJ-123", "experiment.active": False, "weight": 2}


class TestToFlagMetadata:
    @pytest.mark.parametrize("value", [None, "x", 1, True, ["a"]])
    def test_non_dict_yields_empty(self, value):
        assert dict(to_flag_metadata(value)) == {}

    def test_copies_primitives_read_only(self):
        result = to_flag_metadata(METADATA)
        assert dict(result) == METADATA
        with pytest.raises(TypeError):
            result["owner"] = "other"  # type: ignore[index]

    def test_drops_non_primitive_and_non_finite(self):
        value = {"ok": "yes", "nested": {"a": 1}, "list": [1], "nil": None, "nan": math.nan, "inf": math.inf}
        assert dict(to_flag_metadata(value)) == {"ok": "yes"}


class TestEvaluateFlagMetadata:
    flag = {
        "key": "f",
        "enabled": True,
        "default_variation": "on",
        "variations": {"on": True},
        "rules": [],
        "metadata": METADATA,
    }

    def test_includes_metadata(self):
        assert evaluate_flag(self.flag, {}, "acct")["metadata"] == METADATA

    def test_includes_metadata_when_disabled(self):
        result = evaluate_flag({**self.flag, "enabled": False}, {}, "acct")
        assert result["reason"] == "DISABLED"
        assert result["metadata"] == METADATA

    def test_omits_metadata_when_absent(self):
        flag = {k: v for k, v in self.flag.items() if k != "metadata"}
        assert "metadata" not in evaluate_flag(flag, {}, "acct")


ENDPOINT_REGEX = r".*/evaluate.*"


def _resp(**extra: object) -> httpx.Response:
    return httpx.Response(200, json={"flagKey": "f", "value": True, "variant": "on", "reason": "STATIC", **extra})


@respx.mock
def test_http_maps_metadata() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(metadata=METADATA))
    provider = FlagshipServerProvider(app_id="app", account_id="acct", retries=0)
    assert dict(provider.resolve_boolean_details("f", False).flag_metadata) == METADATA


@respx.mock
def test_http_disabled_keeps_metadata() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(reason="DISABLED", value=False, metadata=METADATA))
    provider = FlagshipServerProvider(app_id="app", account_id="acct", retries=0)
    result = provider.resolve_boolean_details("f", True)
    assert result.reason == Reason.DISABLED
    assert dict(result.flag_metadata) == METADATA


@respx.mock
@pytest.mark.parametrize("metadata", ["oops", None, ["a"]])
def test_http_malformed_metadata_is_empty(metadata: object) -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(metadata=metadata))
    provider = FlagshipServerProvider(app_id="app", account_id="acct", retries=0)
    assert dict(provider.resolve_boolean_details("f", False).flag_metadata) == {}


@respx.mock
def test_http_cache_hit_preserves_metadata() -> None:
    route = respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(metadata=METADATA))
    provider = FlagshipServerProvider(app_id="app", account_id="acct", retries=0, cache_ttl=60)
    provider.resolve_boolean_details("f", False)
    hit = provider.resolve_boolean_details("f", False)
    assert route.call_count == 1
    assert hit.reason == Reason.CACHED
    assert dict(hit.flag_metadata) == METADATA


@respx.mock
async def test_async_maps_metadata() -> None:
    respx.get(url__regex=ENDPOINT_REGEX).mock(return_value=_resp(metadata=METADATA))
    provider = FlagshipServerProvider(app_id="app", account_id="acct", retries=0)
    result = await provider.resolve_boolean_details_async("f", False)
    assert dict(result.flag_metadata) == METADATA


@respx.mock
def test_local_evaluation_maps_definition_metadata() -> None:
    flag = {"key": "f", "enabled": True, "default_variation": "on", "variations": {"on": True}, "rules": []}
    definitions = {"flags": {"f": {**flag, "metadata": METADATA}, "g": {**flag, "key": "g"}}}
    respx.get(url__regex=r".*/definitions.*").mock(return_value=httpx.Response(200, json=definitions))
    provider = FlagshipServerProvider(app_id="app", account_id="acct", retries=0, local_evaluation=True)
    provider.initialize(EvaluationContext())
    try:
        assert dict(provider.resolve_boolean_details("f", False).flag_metadata) == METADATA
        assert dict(provider.resolve_boolean_details("g", False).flag_metadata) == {}
    finally:
        provider.shutdown()
