from openfeature.evaluation_context import EvaluationContext
from openfeature.flag_evaluation import FlagEvaluationDetails, FlagType
from openfeature.hook import HookContext

from flagship import LoggingHook


def _ctx() -> HookContext:
    return HookContext(
        flag_key="k",
        flag_type=FlagType.BOOLEAN,
        default_value=False,
        evaluation_context=EvaluationContext(targeting_key="u"),
    )


def test_uses_custom_logger() -> None:
    captured: list[tuple] = []
    hook = LoggingHook(lambda msg, *args: captured.append((msg, args)))
    hook.before(_ctx(), {})
    assert captured
    assert "Evaluating flag" in captured[0][0]


def test_after_logs_value() -> None:
    captured: list[tuple] = []
    hook = LoggingHook(lambda msg, *args: captured.append((msg, args)))
    details = FlagEvaluationDetails(flag_key="k", value=True, reason="DEFAULT")
    hook.after(_ctx(), details, {})
    assert "evaluated" in captured[0][0]


def test_error_logs_message() -> None:
    captured: list[tuple] = []
    hook = LoggingHook(lambda msg, *args: captured.append((msg, args)))
    hook.error(_ctx(), RuntimeError("boom"), {})
    assert "Error evaluating flag" in captured[0][0]
