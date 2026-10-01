import json
import logging
import math
import threading
from collections.abc import Callable, Hashable, Mapping, Sequence
from dataclasses import replace
from typing import Any

from cachetools import TTLCache
from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    FlagNotFoundError,
    GeneralError,
    ParseError,
    ProviderNotReadyError,
    TypeMismatchError,
)
from openfeature.flag_evaluation import (
    FlagResolutionDetails,
    FlagType,
    FlagValueType,
    Reason,
)
from openfeature.hook import Hook
from openfeature.provider import AbstractProvider, Metadata

from ._evaluate import FlagConfigError, evaluate_flag
from ._types import FlagshipEvaluationResponse
from .client import FLAGSHIP_DEFAULT_BASE_URL, FlagshipClient, resolve_definitions_endpoint
from .context import normalize_context

__all__ = ["FlagshipServerProvider"]

_logger = logging.getLogger("flagship")

_DEFAULT_REFRESH_INTERVAL = 30.0
_REFRESH_JOIN_TIMEOUT = 5.0

_TYPE_MAP: dict[FlagType, type | tuple[type, ...]] = {
    FlagType.BOOLEAN: bool,
    FlagType.STRING: str,
    FlagType.INTEGER: int,
    FlagType.FLOAT: float,
    FlagType.OBJECT: (dict, list),
}


class FlagshipServerProvider(AbstractProvider):
    """OpenFeature server-side provider for Cloudflare Flagship (HTTP mode).

    Provide either ``app_id`` + ``account_id`` or ``endpoint``::

        provider = FlagshipServerProvider(
            app_id="app-abc123",
            account_id="acct-456",
            auth_token="token",
        )

    For dynamic credentials (e.g. rotating JWTs), pass ``headers_factory``
    instead of ``auth_token``; it is invoked once per request.

    Set ``logging=True`` to enable SDK-level debug output. When ``False``
    (the default) the SDK produces no log output of its own.

    Set ``cache_ttl`` (seconds) to enable an opt-in TTL + LRU response cache,
    keyed by flag key, type, and evaluation context. Caching is disabled by
    default; cached values may be up to ``cache_ttl`` stale.

    Set ``local_evaluation=True`` to download flag definitions once, evaluate
    flags in-process, and refresh definitions in the background. Requires
    ``account_id`` and a token with app **read** permission. Incompatible with
    ``cache_ttl``.
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
        logging: bool = False,
        cache_ttl: float | None = None,
        cache_max_size: int = 1000,
        local_evaluation: bool = False,
        refresh_interval: float = _DEFAULT_REFRESH_INTERVAL,
    ) -> None:
        self._logging = logging
        self._local_evaluation = local_evaluation is True
        self._refresh_interval = refresh_interval
        self._account_id = account_id

        if self._local_evaluation:
            if (
                not isinstance(refresh_interval, (int, float))
                or not math.isfinite(refresh_interval)
                or refresh_interval <= 0
            ):
                raise ValueError('Flagship: "refresh_interval" must be a finite number of seconds greater than 0')
            if cache_ttl is not None and cache_ttl > 0:
                raise ValueError(
                    "Flagship: local_evaluation is incompatible with cache_ttl — local evaluation has nothing to cache"
                )
            if not account_id:
                raise ValueError('Flagship: "account_id" is required when local_evaluation is enabled')
            # Fail fast if the definitions URL cannot be derived.
            resolve_definitions_endpoint(app_id=app_id, account_id=account_id, endpoint=endpoint, base_url=base_url)

        self._client = FlagshipClient(
            app_id=app_id,
            account_id=account_id,
            endpoint=endpoint,
            base_url=base_url,
            auth_token=auth_token,
            headers_factory=headers_factory,
            timeout=timeout,
            retries=retries,
            retry_delay=retry_delay,
        )
        self._cache: TTLCache[Hashable, FlagResolutionDetails[Any]] | None = (
            TTLCache(maxsize=cache_max_size, ttl=cache_ttl) if cache_ttl is not None and cache_ttl > 0 else None
        )
        self._cache_lock = threading.Lock()

        # Local-evaluation lifecycle state.
        # Snapshot is an immutable (etag, flags) tuple swapped atomically under the GIL.
        self._snapshot: tuple[str, dict[str, Any]] | None = None
        self._stop_event = threading.Event()
        self._refresh_thread: threading.Thread | None = None
        # Generation counter: bumped on every initialize() and shutdown() so
        # in-flight refreshes from a previous session cannot apply their result
        # after close or re-init. A boolean "closed" flag is not enough.
        self._epoch = 0
        self._lifecycle_lock = threading.Lock()

    def get_metadata(self) -> Metadata:
        return Metadata(name="Flagship Server Provider")

    def get_provider_hooks(self) -> list[Hook]:
        return []

    def initialize(self, evaluation_context: EvaluationContext) -> None:
        """Load definitions (local mode) and start the background refresh thread.

        Called by the OpenFeature registry. Failure raises and puts the provider
        in ``ERROR``. No-op when local evaluation is disabled.
        """
        if not self._local_evaluation:
            return

        # Stop any prior refresh thread before starting a new session. Bump the
        # epoch so in-flight fetches from the previous session cannot apply.
        old_thread: threading.Thread | None
        with self._lifecycle_lock:
            self._epoch += 1
            epoch = self._epoch
            self._snapshot = None
            self._stop_event.set()
            old_thread = self._refresh_thread
            self._refresh_thread = None
            self._stop_event = threading.Event()

        if old_thread is not None and old_thread.is_alive():
            old_thread.join(timeout=_REFRESH_JOIN_TIMEOUT)

        self._fetch_and_apply_snapshot(initial=True, epoch=epoch)

        with self._lifecycle_lock:
            # Only start the refresh thread if this initialize was not superseded
            # by shutdown/re-init while the blocking fetch ran.
            if epoch == self._epoch:
                thread = threading.Thread(
                    target=self._refresh_loop,
                    args=(epoch,),
                    name="flagship-definitions-refresh",
                    daemon=True,
                )
                self._refresh_thread = thread
                thread.start()

    def shutdown(self) -> None:
        self._shutdown_local()
        self._clear_cache()
        self._client.close()

    async def shutdown_async(self) -> None:
        self._shutdown_local()
        self._clear_cache()
        await self._client.aclose()

    def _shutdown_local(self) -> None:
        if not self._local_evaluation:
            return
        with self._lifecycle_lock:
            # Bump the epoch so in-flight fetches observe a stale generation and
            # refuse to write.
            self._epoch += 1
            self._snapshot = None
            self._stop_event.set()
            thread = self._refresh_thread
            self._refresh_thread = None
        if thread is not None and thread.is_alive():
            thread.join(timeout=_REFRESH_JOIN_TIMEOUT)

    def resolve_boolean_details(
        self,
        flag_key: str,
        default_value: bool,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[bool]:
        return self._resolve(FlagType.BOOLEAN, flag_key, default_value, evaluation_context)

    def resolve_string_details(
        self,
        flag_key: str,
        default_value: str,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[str]:
        return self._resolve(FlagType.STRING, flag_key, default_value, evaluation_context)

    def resolve_integer_details(
        self,
        flag_key: str,
        default_value: int,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[int]:
        return self._resolve(FlagType.INTEGER, flag_key, default_value, evaluation_context)

    def resolve_float_details(
        self,
        flag_key: str,
        default_value: float,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[float]:
        return self._resolve(FlagType.FLOAT, flag_key, default_value, evaluation_context)

    def resolve_object_details(
        self,
        flag_key: str,
        default_value: Sequence[FlagValueType] | Mapping[str, FlagValueType],
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[Sequence[FlagValueType] | Mapping[str, FlagValueType]]:
        return self._resolve(FlagType.OBJECT, flag_key, default_value, evaluation_context)

    async def resolve_boolean_details_async(
        self,
        flag_key: str,
        default_value: bool,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[bool]:
        return await self._resolve_async(FlagType.BOOLEAN, flag_key, default_value, evaluation_context)

    async def resolve_string_details_async(
        self,
        flag_key: str,
        default_value: str,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[str]:
        return await self._resolve_async(FlagType.STRING, flag_key, default_value, evaluation_context)

    async def resolve_integer_details_async(
        self,
        flag_key: str,
        default_value: int,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[int]:
        return await self._resolve_async(FlagType.INTEGER, flag_key, default_value, evaluation_context)

    async def resolve_float_details_async(
        self,
        flag_key: str,
        default_value: float,
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[float]:
        return await self._resolve_async(FlagType.FLOAT, flag_key, default_value, evaluation_context)

    async def resolve_object_details_async(
        self,
        flag_key: str,
        default_value: Sequence[FlagValueType] | Mapping[str, FlagValueType],
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[Sequence[FlagValueType] | Mapping[str, FlagValueType]]:
        return await self._resolve_async(FlagType.OBJECT, flag_key, default_value, evaluation_context)

    def _resolve(
        self,
        flag_type: FlagType,
        flag_key: str,
        default_value: FlagValueType,
        evaluation_context: EvaluationContext | None,
    ) -> FlagResolutionDetails[Any]:
        if self._local_evaluation:
            return self._resolve_local(flag_type, flag_key, default_value, evaluation_context)

        self._log_debug("[Flagship] Evaluating flag %r", flag_key)
        key = self._cache_key(flag_type, flag_key, evaluation_context)
        if key is not None:
            cached = self._cache_get(key)
            if cached is not None:
                return cached
        result = self._client.evaluate(flag_key, evaluation_context)
        details = _build_details(flag_type, default_value, result)
        self._log_debug(
            "[Flagship] Flag %r resolved: value=%r reason=%s variant=%s",
            flag_key,
            details.value,
            details.reason,
            details.variant,
        )
        if key is not None and result.reason != "DISABLED":
            self._cache_store(key, details)
        return details

    async def _resolve_async(
        self,
        flag_type: FlagType,
        flag_key: str,
        default_value: FlagValueType,
        evaluation_context: EvaluationContext | None,
    ) -> FlagResolutionDetails[Any]:
        if self._local_evaluation:
            # Local evaluation does no I/O — same sync path for both APIs.
            return self._resolve_local(flag_type, flag_key, default_value, evaluation_context)

        self._log_debug("[Flagship] Evaluating flag %r", flag_key)
        key = self._cache_key(flag_type, flag_key, evaluation_context)
        if key is not None:
            cached = self._cache_get(key)
            if cached is not None:
                return cached
        result = await self._client.evaluate_async(flag_key, evaluation_context)
        details = _build_details(flag_type, default_value, result)
        self._log_debug(
            "[Flagship] Flag %r resolved: value=%r reason=%s variant=%s",
            flag_key,
            details.value,
            details.reason,
            details.variant,
        )
        if key is not None and result.reason != "DISABLED":
            self._cache_store(key, details)
        return details

    def _resolve_local(
        self,
        flag_type: FlagType,
        flag_key: str,
        default_value: FlagValueType,
        evaluation_context: EvaluationContext | None,
    ) -> FlagResolutionDetails[Any]:
        self._log_debug("[Flagship] Evaluating flag %r locally", flag_key)

        snapshot = self._snapshot
        if snapshot is None:
            msg = "Flagship local evaluation is not ready — definitions have not been loaded"
            self._log_error("[Flagship] Flag %r evaluation failed (PROVIDER_NOT_READY): %s", flag_key, msg)
            raise ProviderNotReadyError(error_message=msg)

        _etag, flags = snapshot
        flag_def = flags.get(flag_key) if isinstance(flags, dict) else None
        if not isinstance(flag_def, dict):
            msg = f'Flag "{flag_key}" was not found in local definitions'
            self._log_error("[Flagship] %s", msg)
            raise FlagNotFoundError(error_message=msg)

        try:
            normalized = normalize_context(evaluation_context)
            assert self._account_id is not None
            result = evaluate_flag(flag_def, normalized.values, self._account_id)
            response = FlagshipEvaluationResponse(
                flag_key=flag_key,
                value=result["value"],
                variant=result["variant"],
                reason=result["reason"],
            )
            details = _build_details(flag_type, default_value, response)
            self._log_debug(
                "[Flagship] Flag %r resolved: value=%r reason=%s variant=%s",
                flag_key,
                details.value,
                details.reason,
                details.variant,
            )
            return details
        except FlagConfigError as e:
            msg = str(e)
            self._log_error("[Flagship] Flag %r evaluation failed (PARSE_ERROR): %s", flag_key, msg)
            raise ParseError(error_message=msg) from e
        except (TypeMismatchError, FlagNotFoundError, ProviderNotReadyError, ParseError):
            raise
        except Exception as e:
            # The engine is pure, so any other throw means the flag entry is malformed.
            msg = f'Flag "{flag_key}" definition is malformed: {e}'
            self._log_error("[Flagship] %s (PARSE_ERROR)", msg)
            raise ParseError(error_message=msg) from e

    def _refresh_loop(self, epoch: int) -> None:
        """Background daemon: wait ``refresh_interval``, then refresh, until stopped."""
        while not self._stop_event.wait(self._refresh_interval):
            # Re-check generation before starting the fetch so a shutdown that
            # landed during the wait is observed immediately.
            if epoch != self._epoch:
                return
            self._fetch_and_apply_snapshot(initial=False, epoch=epoch)

    def _fetch_and_apply_snapshot(self, *, initial: bool, epoch: int) -> None:
        try:
            current_etag = self._snapshot[0] if self._snapshot is not None else None
            # Skip empty etag — conditional refresh is unavailable without one.
            etag = current_etag if current_etag else None
            result = self._client.fetch_definitions(etag)
            # Stale session (closed or superseded by a newer initialize).
            if epoch != self._epoch:
                return

            if result is None:
                self._log_debug("[Flagship] Definitions not modified (304)")
                return

            flags, new_etag = result
            # Assignment of the tuple is atomic under the GIL.
            self._snapshot = (new_etag, flags)
            if not new_etag:
                self._log_debug("[Flagship] Definitions response had no ETag; conditional refresh (304) is unavailable")
            self._log_debug("[Flagship] Definitions snapshot updated (%d flags)", len(flags))
        except Exception as error:
            if epoch != self._epoch:
                return
            if initial:
                raise
            self._log_error(
                "[Flagship] Definitions refresh failed; keeping last good snapshot: %s",
                error,
            )

    def _cache_key(
        self,
        flag_type: FlagType,
        flag_key: str,
        evaluation_context: EvaluationContext | None,
    ) -> Hashable | None:
        if self._cache is None:
            return None
        context = normalize_context(evaluation_context).values
        serialized = json.dumps(context, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
        return (flag_key, flag_type, serialized)

    def _cache_get(self, key: Hashable) -> FlagResolutionDetails[Any] | None:
        assert self._cache is not None
        with self._cache_lock:
            cached = self._cache.get(key)
        if cached is None:
            return None
        return replace(cached, reason=Reason.CACHED)

    def _cache_store(self, key: Hashable, details: FlagResolutionDetails[Any]) -> None:
        assert self._cache is not None
        with self._cache_lock:
            self._cache[key] = details

    def _clear_cache(self) -> None:
        if self._cache is not None:
            with self._cache_lock:
                self._cache.clear()

    def _log_debug(self, msg: str, *args: Any) -> None:
        if self._logging:
            _logger.debug(msg, *args)

    def _log_error(self, msg: str, *args: Any) -> None:
        if self._logging:
            _logger.error(msg, *args)


def _build_details(
    flag_type: FlagType,
    default_value: Any,
    result: Any,
) -> FlagResolutionDetails[Any]:
    if result.reason == "DISABLED":
        return FlagResolutionDetails(
            value=default_value,
            reason=Reason.DISABLED,
            variant=result.variant or None,
        )

    value = _typecheck_flag_value(result.value, flag_type)

    return FlagResolutionDetails(
        value=value,
        variant=result.variant or None,
        reason=_map_reason(result.reason),
    )


def _typecheck_flag_value(value: Any, flag_type: FlagType) -> Any:
    """Validate the resolved value matches the requested flag type.

    Raises :class:`openfeature.exception.TypeMismatchError` on mismatch.
    Coerces ``int`` to ``float`` for FLOAT flags.
    """
    if flag_type == FlagType.BOOLEAN:
        if not isinstance(value, bool):
            raise TypeMismatchError(f"Expected bool, got {type(value).__name__}")
        return value

    if flag_type == FlagType.INTEGER:
        if not isinstance(value, int) or isinstance(value, bool):
            raise TypeMismatchError(f"Expected int, got {type(value).__name__}")
        return value

    if flag_type == FlagType.FLOAT:
        if isinstance(value, bool):
            raise TypeMismatchError(f"Expected float, got {type(value).__name__}")
        if isinstance(value, int):
            return float(value)
        if not isinstance(value, float):
            raise TypeMismatchError(f"Expected float, got {type(value).__name__}")
        return value

    expected = _TYPE_MAP.get(flag_type)
    if expected is None:
        raise GeneralError(f"Unknown flag type: {flag_type}")
    if not isinstance(value, expected):
        raise TypeMismatchError(f"Expected {flag_type.name.lower()}, got {type(value).__name__}")
    return value


_REASON_MAP: dict[str, Reason] = {r.value: r for r in Reason}


def _map_reason(reason: str) -> str | Reason:
    return _REASON_MAP.get(reason, reason)
