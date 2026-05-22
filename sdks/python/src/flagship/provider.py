import logging
from collections.abc import Callable, Mapping, Sequence
from typing import Any

from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import (
    GeneralError,
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

from .client import FLAGSHIP_DEFAULT_BASE_URL, FlagshipClient

__all__ = ["FlagshipServerProvider"]

_logger = logging.getLogger("flagship")

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
    ) -> None:
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
        self._logging = logging

    def get_metadata(self) -> Metadata:
        return Metadata(name="Flagship Server Provider")

    def get_provider_hooks(self) -> list[Hook]:
        return []

    def initialize(self, _evaluation_context: EvaluationContext) -> None:
        """Initialize without probing the evaluation endpoint.

        The OpenFeature SDK owns provider readiness state. HTTP failures are
        reported by the individual flag evaluations that encounter them.
        """
        return None

    def shutdown(self) -> None:
        self._client.close()

    async def shutdown_async(self) -> None:
        await self._client.aclose()

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
        self._log_debug("[Flagship] Evaluating flag %r", flag_key)
        result = self._client.evaluate(flag_key, evaluation_context)
        details = _build_details(flag_type, default_value, result)
        self._log_debug(
            "[Flagship] Flag %r resolved: value=%r reason=%s variant=%s",
            flag_key,
            details.value,
            details.reason,
            details.variant,
        )
        return details

    async def _resolve_async(
        self,
        flag_type: FlagType,
        flag_key: str,
        default_value: FlagValueType,
        evaluation_context: EvaluationContext | None,
    ) -> FlagResolutionDetails[Any]:
        self._log_debug("[Flagship] Evaluating flag %r", flag_key)
        result = await self._client.evaluate_async(flag_key, evaluation_context)
        details = _build_details(flag_type, default_value, result)
        self._log_debug(
            "[Flagship] Flag %r resolved: value=%r reason=%s variant=%s",
            flag_key,
            details.value,
            details.reason,
            details.variant,
        )
        return details

    def _log_debug(self, msg: str, *args: Any) -> None:
        if self._logging:
            _logger.debug(msg, *args)

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
