import time
import weakref
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, Literal

from openfeature.hook import Hook

if TYPE_CHECKING:
    from openfeature.exception import ErrorCode
    from openfeature.flag_evaluation import FlagEvaluationDetails, FlagValueType
    from openfeature.hook import HookContext, HookHints

__all__ = ["TelemetryEvent", "TelemetryHook"]


@dataclass
class TelemetryEvent:
    type: Literal["evaluation", "error"]
    flag_key: str
    timestamp: int
    duration_ms: float | None = None
    value: Any = None
    reason: str | None = None
    variant: str | None = None
    error_code: "ErrorCode | None" = None
    error_message: str | None = None
    context: Any = None
    hints: Mapping[str, Any] | None = None


class TelemetryHook(Hook):
    """Emits a :class:`TelemetryEvent` for every flag evaluation.

    Useful for sending evaluation metrics to an analytics service::

        api.add_hooks([TelemetryHook(analytics.track)])
    """

    def __init__(self, on_event: Callable[[TelemetryEvent], None]) -> None:
        self._on_event = on_event
        self._start_times: weakref.WeakKeyDictionary[Any, float] = weakref.WeakKeyDictionary()
        self._hints: weakref.WeakKeyDictionary[Any, Any] = weakref.WeakKeyDictionary()

    def before(self, hook_context: "HookContext", hints: "HookHints") -> None:
        self._start_times[hook_context] = time.monotonic()
        if hints:
            self._hints[hook_context] = hints

    def after(
        self,
        hook_context: "HookContext",
        details: "FlagEvaluationDetails[FlagValueType]",
        hints: "HookHints",
    ) -> None:
        self._on_event(
            TelemetryEvent(
                type="evaluation",
                flag_key=hook_context.flag_key,
                timestamp=_now_ms(),
                duration_ms=self._pop_duration(hook_context),
                value=details.value,
                reason=str(details.reason) if details.reason is not None else None,
                variant=details.variant,
                error_code=details.error_code,
                error_message=details.error_message,
                context=hook_context.evaluation_context,
                hints=self._hints.get(hook_context),
            )
        )

    def error(
        self,
        hook_context: "HookContext",
        exception: Exception,
        hints: "HookHints",
    ) -> None:
        self._on_event(
            TelemetryEvent(
                type="error",
                flag_key=hook_context.flag_key,
                timestamp=_now_ms(),
                duration_ms=self._pop_duration(hook_context),
                error_message=str(exception),
                context=hook_context.evaluation_context,
                hints=self._hints.get(hook_context),
            )
        )

    def finally_after(
        self,
        hook_context: "HookContext",
        details: "FlagEvaluationDetails[FlagValueType]",
        hints: "HookHints",
    ) -> None:
        self._start_times.pop(hook_context, None)
        self._hints.pop(hook_context, None)

    def _pop_duration(self, hook_context: "HookContext") -> float | None:
        start = self._start_times.pop(hook_context, None)
        return None if start is None else (time.monotonic() - start) * 1000.0


def _now_ms() -> int:
    return int(time.time() * 1000)
