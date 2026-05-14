import logging
from collections.abc import Callable
from typing import TYPE_CHECKING

from openfeature.hook import Hook

if TYPE_CHECKING:
    from openfeature.flag_evaluation import FlagEvaluationDetails, FlagValueType
    from openfeature.hook import HookContext, HookHints

__all__ = ["LoggingHook"]

_default_logger = logging.getLogger("flagship").info


class LoggingHook(Hook):
    """Logs flag evaluation lifecycle events.

    Pass a custom callable to redirect output.
    """

    def __init__(self, logger: Callable[..., None] | None = None) -> None:
        self._logger = logger or _default_logger

    def before(self, hook_context: "HookContext", hints: "HookHints") -> None:
        self._logger(
            "[Flagship] Evaluating flag: %s",
            hook_context.flag_key,
        )

    def after(
        self,
        hook_context: "HookContext",
        details: "FlagEvaluationDetails[FlagValueType]",
        hints: "HookHints",
    ) -> None:
        self._logger(
            "[Flagship] Flag %s evaluated: value=%r reason=%s variant=%s",
            hook_context.flag_key,
            details.value,
            details.reason,
            details.variant,
        )

    def error(
        self,
        hook_context: "HookContext",
        exception: Exception,
        hints: "HookHints",
    ) -> None:
        self._logger(
            "[Flagship] Error evaluating flag %s: %s",
            hook_context.flag_key,
            exception,
        )
