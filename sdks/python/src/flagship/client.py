import asyncio
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from typing import Any, TypeVar
from urllib.parse import quote, urlsplit, urlunsplit

import httpx
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    FlagNotFoundError,
    GeneralError,
    ParseError,
)

from ._types import FlagshipEvaluationResponse
from .context import JsonValue, context_to_query_params, normalize_context
from .sse import StreamAuthError

__all__ = ["FLAGSHIP_DEFAULT_BASE_URL", "FlagshipClient", "resolve_definitions_endpoint"]

FLAGSHIP_DEFAULT_BASE_URL = "https://api.cloudflare.com"

_MAX_RETRIES = 10
_MAX_RETRY_DELAY = 30.0

T = TypeVar("T")


class _BadRequestError(GeneralError):
    """400 Bad Request — terminal, never retried."""


class RateLimitedError(GeneralError):
    """429 Too Many Requests. Never retried; callers wait ``retry_after`` seconds."""

    def __init__(self, message: str, retry_after: float | None) -> None:
        super().__init__(message)
        self.retry_after = retry_after


@dataclass(frozen=True)
class BulkEvaluation:
    """A successful OFREP bulk evaluation.

    ``etag`` is the HTTP response validator for this context's results;
    ``version`` is the app-wide configuration version from ``metadata``.
    """

    flags: dict[str, dict[str, Any]]
    etag: str | None
    version: str | None
    event_streams: list[dict[str, object]] = field(default_factory=list)


@dataclass(frozen=True)
class _HttpRequest:
    method: str
    url: str
    params: dict[str, str] | None = None
    body: dict[str, str | dict[str, JsonValue]] | None = None
    extra_headers: dict[str, str] | None = None


class FlagshipClient:
    """HTTP client for the Flagship evaluation API.

    Independent of OpenFeature wiring. Used internally by
    :class:`flagship.FlagshipServerProvider` but usable standalone.

    Provide either ``app_id`` + ``account_id`` or ``endpoint``::

        client = FlagshipClient(app_id="app-abc123", account_id="acct-456", auth_token="token")

    Retries are attempted on transient errors. 404 and 400 responses are never retried.
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
        retries: int = 1,
        retry_delay: float = 1.0,
    ) -> None:
        self.endpoint = _resolve_endpoint(app_id=app_id, account_id=account_id, endpoint=endpoint, base_url=base_url)
        # Definitions URL is derived when possible; left as None when the endpoint
        # path does not end in /evaluate (local evaluation will reject that case).
        self.definitions_endpoint = _try_resolve_definitions_endpoint(
            app_id=app_id, account_id=account_id, endpoint=endpoint, base_url=base_url
        )
        parsed = urlsplit(self.endpoint)
        self.bulk_endpoint = (
            urlunsplit(
                (
                    parsed.scheme,
                    parsed.netloc,
                    parsed.path.removesuffix("/evaluate") + "/ofrep/v1/evaluate/flags",
                    "",
                    "",
                )
            )
            if parsed.path.endswith("/evaluate")
            else None
        )
        self.timeout = timeout
        self.retries = min(retries, _MAX_RETRIES)
        self.retry_delay = min(retry_delay, _MAX_RETRY_DELAY)
        self._headers_factory = _compose_headers_factory(auth_token, headers_factory)
        self._sync_client = httpx.Client(timeout=timeout)
        self._async_client = httpx.AsyncClient(timeout=timeout)

    def close(self) -> None:
        self._sync_client.close()

    async def aclose(self) -> None:
        await self._async_client.aclose()

    def evaluate(self, flag_key: str, context: EvaluationContext | None = None) -> FlagshipEvaluationResponse:
        """Evaluate a flag synchronously.

        Primitive-only context uses query parameters. Structured context uses a
        JSON request body. Unsupported or cyclic values raise
        :class:`openfeature.exception.InvalidContextError`.
        Raises :class:`openfeature.exception.FlagNotFoundError` on 404.
        Raises :class:`openfeature.exception.GeneralError` on network or server errors.
        Raises :class:`openfeature.exception.ParseError` on malformed responses.
        """
        request = self._build_request(flag_key, context)
        return self._fetch_with_retry_sync(request, retries_left=self.retries, parse=_parse_evaluation_response)

    async def evaluate_async(
        self, flag_key: str, context: EvaluationContext | None = None
    ) -> FlagshipEvaluationResponse:
        """Evaluate a flag asynchronously.

        Same error contract as :meth:`evaluate`.
        """
        request = self._build_request(flag_key, context)
        return await self._fetch_with_retry_async(request, retries_left=self.retries, parse=_parse_evaluation_response)

    def fetch_definitions(self, etag: str | None = None) -> tuple[dict[str, Any], str] | None:
        """Fetch the app's flag definitions for local evaluation.

        Sends ``If-None-Match`` when ``etag`` is provided. A ``304`` response
        yields ``None``; a ``200`` yields ``(flags, etag)``.

        Sync only — local evaluation performs no per-request I/O after init.
        """
        if not self.definitions_endpoint:
            raise ValueError(
                "Flagship: definitions endpoint is not configured. "
                'Provide app_id+account_id, or an endpoint ending in "/evaluate".'
            )

        result = self.fetch_definitions_response(etag)
        if result is None:
            return None
        flags, new_etag, _, _ = result
        return flags, new_etag

    def fetch_definitions_response(
        self,
        etag: str | None = None,
        flag_config_etag: str | None = None,
        request_event_stream: bool = False,
    ) -> tuple[dict[str, Any], str, str, list[dict[str, object]]] | None:
        if not self.definitions_endpoint:
            raise ValueError("Flagship: definitions endpoint is not configured")
        headers = {"If-None-Match": etag} if etag else {}
        if request_event_stream:
            headers["Flagship-Event-Stream"] = "sse"
        request = _HttpRequest(
            method="GET",
            url=self.definitions_endpoint,
            params={"flagConfigEtag": flag_config_etag} if flag_config_etag else None,
            extra_headers=headers,
        )
        return self._fetch_with_retry_sync(request, retries_left=self.retries, parse=_parse_definitions_with_stream)

    def fetch_bulk_version(
        self,
        flag_config_etag: str | None = None,
        request_event_stream: bool = False,
    ) -> tuple[str, list[dict[str, object]]]:
        if not self.bulk_endpoint:
            raise ValueError('Flagship: configuration events require an endpoint ending in "/evaluate"')
        request = _HttpRequest(
            method="POST",
            url=self.bulk_endpoint,
            params={"flagConfigEtag": flag_config_etag} if flag_config_etag else None,
            body={"context": {}},
            extra_headers={"Flagship-Event-Stream": "sse"} if request_event_stream else None,
        )
        return self._fetch_with_retry_sync(request, retries_left=self.retries, parse=_parse_bulk_version)

    def fetch_bulk(
        self,
        context: EvaluationContext | None,
        *,
        if_none_match: str | None = None,
        flag_config_etag: str | None = None,
        flag_config_last_modified: int | str | None = None,
        request_event_stream: bool = False,
    ) -> BulkEvaluation | None:
        """Evaluate every flag for ``context`` with one OFREP bulk request.

        Returns ``None`` on ``304 Not Modified``. ``flag_config_etag`` and
        ``flag_config_last_modified`` carry change-event metadata as query
        parameters. Raises :class:`RateLimitedError` on ``429`` without retrying.
        """
        if not self.bulk_endpoint:
            raise ValueError('Flagship: bulk evaluation requires an endpoint ending in "/evaluate"')
        params: dict[str, str] = {}
        if flag_config_etag:
            params["flagConfigEtag"] = flag_config_etag
        if flag_config_last_modified is not None:
            params["flagConfigLastModified"] = str(flag_config_last_modified)
        headers: dict[str, str] = {}
        if if_none_match:
            headers["If-None-Match"] = if_none_match
        if request_event_stream:
            headers["Flagship-Event-Stream"] = "sse"
        request = _HttpRequest(
            method="POST",
            url=self.bulk_endpoint,
            params=params or None,
            body={"context": normalize_context(context).values},
            extra_headers=headers or None,
        )
        return self._fetch_with_retry_sync(request, retries_left=self.retries, parse=_parse_bulk_evaluation)

    def _build_request(self, flag_key: str, context: EvaluationContext | None) -> _HttpRequest:
        normalized = normalize_context(context)
        if normalized.requires_post:
            return _HttpRequest(
                "POST",
                self.endpoint,
                body={"flagKey": flag_key, "context": normalized.values},
            )
        params: dict[str, str] = {"flagKey": flag_key}
        params.update(context_to_query_params(context))
        return _HttpRequest("GET", self.endpoint, params=params)

    def _headers(self, extra: dict[str, str] | None = None) -> dict[str, str] | None:
        base = self._headers_factory() if self._headers_factory else {}
        if extra:
            # Explicit per-request headers (e.g. If-None-Match) win over factory.
            merged = {**base, **extra}
            return merged
        return base or None

    def _fetch_with_retry_sync(
        self,
        request: _HttpRequest,
        retries_left: int,
        parse: Callable[[httpx.Response], T],
    ) -> T:
        try:
            try:
                response = self._sync_client.request(
                    request.method,
                    request.url,
                    params=request.params,
                    json=request.body,
                    headers=self._headers(request.extra_headers),
                )
            except httpx.TimeoutException as e:
                raise GeneralError(f"Request timeout after {self.timeout}s") from e
            except httpx.HTTPError as e:
                raise GeneralError(f"Network error: {e}") from e
            return parse(response)
        except (FlagNotFoundError, _BadRequestError, ParseError, StreamAuthError, RateLimitedError):
            # 404, 400, 401/403, 429, parse errors — deterministic or server-paced, never retry here.
            raise
        except Exception:
            if retries_left > 0:
                time.sleep(self.retry_delay)
                return self._fetch_with_retry_sync(request, retries_left - 1, parse)
            raise

    async def _fetch_with_retry_async(
        self,
        request: _HttpRequest,
        retries_left: int,
        parse: Callable[[httpx.Response], T],
    ) -> T:
        try:
            try:
                response = await self._async_client.request(
                    request.method,
                    request.url,
                    params=request.params,
                    json=request.body,
                    headers=self._headers(request.extra_headers),
                )
            except httpx.TimeoutException as e:
                raise GeneralError(f"Request timeout after {self.timeout}s") from e
            except httpx.HTTPError as e:
                raise GeneralError(f"Network error: {e}") from e
            return parse(response)
        except (FlagNotFoundError, _BadRequestError, ParseError):
            # 404, 400, parse errors — deterministic, never retry.
            raise
        except Exception:
            if retries_left > 0:
                await asyncio.sleep(self.retry_delay)
                return await self._fetch_with_retry_async(request, retries_left - 1, parse)
            raise


def _parse_evaluation_response(response: httpx.Response) -> FlagshipEvaluationResponse:
    status = response.status_code

    if status == 404:
        raise FlagNotFoundError(_error_detail(response))
    if status == 400:
        raise _BadRequestError(_error_detail(response))
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
        metadata=data["metadata"] if isinstance(data.get("metadata"), dict) else {},
    )


def _parse_definitions_response(response: httpx.Response) -> tuple[dict[str, Any], str] | None:
    status = response.status_code

    if status == 304:
        return None

    if status == 404:
        raise FlagNotFoundError(_error_detail(response))
    if status == 400:
        raise _BadRequestError(_error_detail(response))
    if status >= 400:
        raise GeneralError(f"HTTP {status}: {response.reason_phrase}")

    try:
        data: Any = response.json()
    except Exception as e:
        raise ParseError(f"Invalid JSON response: {e}") from e

    # Only validate the top-level shape; individual malformed flags surface at evaluation time.
    if not isinstance(data, dict) or not isinstance(data.get("flags"), dict):
        raise ParseError("Invalid definitions response format from Flagship API")

    etag = response.headers.get("etag") or ""
    return data["flags"], etag


def _parse_definitions_with_stream(
    response: httpx.Response,
) -> tuple[dict[str, Any], str, str, list[dict[str, object]]] | None:
    if response.status_code in (401, 403):
        raise StreamAuthError("Configuration discovery was not authorized")
    result = _parse_definitions_response(response)
    if result is None:
        return None
    flags, etag = result
    body: Any = response.json()
    metadata = body.get("metadata")
    version = metadata.get("version") if isinstance(metadata, dict) else None
    streams = body.get("eventStreams")
    return (
        flags,
        etag,
        version if isinstance(version, str) else etag.strip('"'),
        [item for item in streams if isinstance(item, dict)] if isinstance(streams, list) else [],
    )


def _parse_bulk_version(response: httpx.Response) -> tuple[str, list[dict[str, object]]]:
    if response.status_code in (401, 403):
        raise StreamAuthError("Configuration discovery was not authorized")
    if response.status_code == 404:
        raise FlagNotFoundError("Configuration not found")
    if response.status_code != 200:
        raise GeneralError(f"Configuration discovery returned HTTP {response.status_code}")
    try:
        body: Any = response.json()
    except ValueError as error:
        raise ParseError("Invalid configuration discovery response") from error
    if not isinstance(body, dict) or not isinstance(body.get("metadata"), dict):
        raise ParseError("Missing configuration metadata")
    version = body["metadata"].get("version")
    if not isinstance(version, str) or not version:
        raise ParseError("Missing configuration version")
    streams = body.get("eventStreams")
    return version, [item for item in streams if isinstance(item, dict)] if isinstance(streams, list) else []


def _parse_bulk_evaluation(response: httpx.Response) -> BulkEvaluation | None:
    status = response.status_code
    if status == 304:
        return None
    if status in (401, 403):
        raise StreamAuthError(f"Bulk evaluation was not authorized (HTTP {status})")
    if status == 429:
        raise RateLimitedError("Bulk evaluation was rate limited (HTTP 429)", _retry_after(response))
    if status == 404:
        raise FlagNotFoundError(_error_detail(response))
    if status == 400:
        raise _BadRequestError(_error_detail(response))
    if status >= 400:
        raise GeneralError(f"HTTP {status}: {response.reason_phrase}")
    try:
        body: Any = response.json()
    except ValueError as error:
        raise ParseError("Invalid bulk evaluation response") from error
    if not isinstance(body, dict) or not isinstance(body.get("flags"), list):
        raise ParseError("Invalid bulk evaluation response format")
    flags = {item["key"]: item for item in body["flags"] if isinstance(item, dict) and isinstance(item.get("key"), str)}
    metadata = body.get("metadata")
    version = metadata.get("version") if isinstance(metadata, dict) else None
    streams = body.get("eventStreams")
    return BulkEvaluation(
        flags=flags,
        etag=response.headers.get("etag") or None,
        version=version if isinstance(version, str) and version else None,
        event_streams=[item for item in streams if isinstance(item, dict)] if isinstance(streams, list) else [],
    )


def _retry_after(response: httpx.Response) -> float | None:
    """Seconds from a ``Retry-After`` header, given as a delay or an HTTP date."""
    value = (response.headers.get("retry-after") or "").strip()
    if value.isdigit():
        return float(value)
    try:
        when = parsedate_to_datetime(value)
    except (TypeError, ValueError):
        return None
    if when.tzinfo is None:
        when = when.replace(tzinfo=timezone.utc)
    return max(0.0, (when - datetime.now(timezone.utc)).total_seconds())


def _error_detail(response: httpx.Response) -> str:
    try:
        return str(response.json().get("errorDetails") or response.text)
    except Exception:
        return response.text or response.reason_phrase


def _compose_headers_factory(
    auth_token: str | None,
    headers_factory: Callable[[], dict[str, str]] | None,
) -> Callable[[], dict[str, str]] | None:
    """Merge auth_token and headers_factory.

    The factory's ``Authorization`` header takes precedence over ``auth_token``,
    matching the TypeScript SDK's behaviour where explicit headers win.
    """
    if auth_token is None and headers_factory is None:
        return None

    bearer = {"Authorization": f"Bearer {auth_token}"} if auth_token else {}

    if headers_factory is None:
        return lambda: dict(bearer)

    if not bearer:
        return headers_factory

    def combined() -> dict[str, str]:
        # Factory wins on Authorization (explicit headers take precedence).
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
        parsed = httpx.URL(endpoint)
        if not parsed.scheme or not parsed.host:
            raise ValueError(f"Flagship: invalid endpoint URL: {endpoint}")
        return endpoint

    if not account_id:
        raise ValueError('Flagship: "account_id" is required when using "app_id"')

    assert app_id is not None
    base = base_url.rstrip("/")
    return f"{base}/client/v4/accounts/{quote(account_id, safe='')}/flagship/apps/{quote(app_id, safe='')}/evaluate"


def resolve_definitions_endpoint(
    *,
    app_id: str | None = None,
    account_id: str | None = None,
    endpoint: str | None = None,
    base_url: str = FLAGSHIP_DEFAULT_BASE_URL,
) -> str:
    """Derive the definitions URL from provider options.

    - With ``app_id``: ``…/apps/{app_id}/definitions``
    - With ``endpoint``: replace a trailing ``/evaluate`` with ``/definitions``

    Raises :class:`ValueError` when the URL cannot be derived.
    """
    if app_id and endpoint:
        raise ValueError('Flagship: provide either "app_id" or "endpoint", not both')

    if endpoint:
        parsed = httpx.URL(endpoint)
        if not parsed.scheme or not parsed.host:
            raise ValueError(f"Flagship: invalid endpoint URL: {endpoint}")
        path = parsed.path.rstrip("/")
        if not path.endswith("/evaluate"):
            raise ValueError(
                'Flagship: when local_evaluation is enabled with "endpoint", '
                'the URL path must end in "/evaluate" so the definitions URL can be derived'
            )
        new_path = f"{path[: -len('/evaluate')]}/definitions"
        return str(parsed.copy_with(path=new_path))

    if not app_id:
        raise ValueError('Flagship: either "app_id" or "endpoint" is required')
    if not account_id:
        raise ValueError('Flagship: "account_id" is required when using "app_id"')

    base = base_url.rstrip("/")
    return f"{base}/client/v4/accounts/{quote(account_id, safe='')}/flagship/apps/{quote(app_id, safe='')}/definitions"


def _try_resolve_definitions_endpoint(
    *,
    app_id: str | None,
    account_id: str | None,
    endpoint: str | None,
    base_url: str,
) -> str | None:
    try:
        return resolve_definitions_endpoint(app_id=app_id, account_id=account_id, endpoint=endpoint, base_url=base_url)
    except ValueError:
        return None
