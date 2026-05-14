from collections.abc import Callable
from typing import Any
from urllib.parse import quote

import httpx
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    FlagNotFoundError,
    GeneralError,
    ParseError,
)

from ._types import FlagshipEvaluationResponse
from .context import context_to_query_params

__all__ = ["FLAGSHIP_DEFAULT_BASE_URL", "FlagshipClient"]

FLAGSHIP_DEFAULT_BASE_URL = "https://api.cloudflare.com"


class FlagshipClient:
    """HTTP client for the Flagship evaluation API.

    Independent of OpenFeature wiring. Used internally by
    :class:`flagship.FlagshipServerProvider` but usable standalone.
    """

    def __init__(
        self,
        *,
        app_id: str | None = None,
        account_id: str | None = None,
        endpoint: str | None = None,
        base_url: str = FLAGSHIP_DEFAULT_BASE_URL,
        auth_token: str | None = None,
        headers_factory: Callable[[], dict[str, str]] | None = None,
        timeout: float = 5.0,
    ) -> None:
        self.endpoint = _resolve_endpoint(app_id=app_id, account_id=account_id, endpoint=endpoint, base_url=base_url)
        self.timeout = timeout
        self.headers_factory = _compose_headers_factory(auth_token, headers_factory)
        self._sync_client = httpx.Client(timeout=timeout)
        self._async_client = httpx.AsyncClient(timeout=timeout)

    def close(self) -> None:
        self._sync_client.close()

    async def aclose(self) -> None:
        await self._async_client.aclose()

    def evaluate(self, flag_key: str, context: EvaluationContext | None = None) -> FlagshipEvaluationResponse:
        params = self._build_params(flag_key, context)
        try:
            response = self._sync_client.get(self.endpoint, params=params, headers=self._headers())
        except httpx.TimeoutException as e:
            raise GeneralError(f"Request timeout after {self.timeout}s") from e
        except httpx.HTTPError as e:
            raise GeneralError(f"Network error: {e}") from e
        return _process_response(response)

    async def evaluate_async(
        self, flag_key: str, context: EvaluationContext | None = None
    ) -> FlagshipEvaluationResponse:
        params = self._build_params(flag_key, context)
        try:
            response = await self._async_client.get(self.endpoint, params=params, headers=self._headers())
        except httpx.TimeoutException as e:
            raise GeneralError(f"Request timeout after {self.timeout}s") from e
        except httpx.HTTPError as e:
            raise GeneralError(f"Network error: {e}") from e
        return _process_response(response)

    def _build_params(self, flag_key: str, context: EvaluationContext | None) -> dict[str, str]:
        params = {"flagKey": flag_key}
        params.update(context_to_query_params(context))
        return params

    def _headers(self) -> dict[str, str] | None:
        return self.headers_factory() if self.headers_factory else None


def _process_response(response: httpx.Response) -> FlagshipEvaluationResponse:
    status = response.status_code

    if status == 404:
        raise FlagNotFoundError(_error_detail(response))
    if status == 400:
        raise GeneralError(_error_detail(response))
    if status >= 400:
        raise GeneralError(f"HTTP {status}: {response.reason_phrase}")

    try:
        data: Any = response.json()
    except Exception as e:
        raise ParseError(f"Invalid JSON response: {e}") from e

    if not isinstance(data, dict) or "flagKey" not in data or "value" not in data:
        raise ParseError("Invalid response format from Flagship API")

    return FlagshipEvaluationResponse(
        flag_key=data["flagKey"],
        value=data["value"],
        variant=data.get("variant", ""),
        reason=data.get("reason", "DEFAULT"),
    )


def _error_detail(response: httpx.Response) -> str:
    try:
        return str(response.json().get("errorDetails") or response.text)
    except Exception:
        return response.text or response.reason_phrase


def _compose_headers_factory(
    auth_token: str | None,
    headers_factory: Callable[[], dict[str, str]] | None,
) -> Callable[[], dict[str, str]] | None:
    if auth_token is None and headers_factory is None:
        return None

    bearer = {"Authorization": f"Bearer {auth_token}"} if auth_token else {}

    if headers_factory is None:
        return lambda: dict(bearer)

    if not bearer:
        return headers_factory

    def combined() -> dict[str, str]:
        return {**bearer, **headers_factory()}

    return combined


def _resolve_endpoint(
    *,
    app_id: str | None,
    account_id: str | None,
    endpoint: str | None,
    base_url: str,
) -> str:
    if app_id and endpoint:
        raise ValueError('Flagship: provide either "app_id" or "endpoint", not both')
    if not app_id and not endpoint:
        raise ValueError('Flagship: either "app_id" or "endpoint" is required')

    if endpoint:
        parts = httpx.URL(endpoint)
        if not parts.scheme or not parts.host:
            raise ValueError(f"Flagship: invalid endpoint URL: {endpoint}")
        return endpoint

    if not account_id:
        raise ValueError('Flagship: "account_id" is required when using "app_id"')

    base = base_url.rstrip("/")
    return (
        f"{base}/client/v4/accounts/{quote(account_id, safe='')}"
        f"/flagship/apps/{quote(app_id, safe='')}/evaluate"  # type: ignore[arg-type]
    )
