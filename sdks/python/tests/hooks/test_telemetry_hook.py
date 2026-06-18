import time

from openfeature.evaluation_context import EvaluationContext
from openfeature.exception import ErrorCode
from openfeature.flag_evaluation import FlagEvaluationDetails, FlagType
from openfeature.hook import HookContext

from flagship import TelemetryEvent, TelemetryHook


def _ctx(flag_key: str = "k") -> HookContext:
    return HookContext(
        flag_key=flag_key,
        flag_type=FlagType.BOOLEAN,
        default_value=False,
        evaluation_context=EvaluationContext(targeting_key="u"),
    )


def test_before_and_after_emits_evaluation_event() -> None:
    events: list[TelemetryEvent] = []
    hook = TelemetryHook(events.append)
    ctx = _ctx()
    hook.before(ctx, {"trace_id": "abc"})
    time.sleep(0.01)
    hook.after(
        ctx,
        FlagEvaluationDetails(flag_key="k", value=True, variant="on", reason="TARGETING_MATCH"),
        {"trace_id": "abc"},
    )
    assert len(events) == 1
    e = events[0]
    assert e.type == "evaluation"
    assert e.value is True
    assert e.duration_ms is not None and e.duration_ms >= 0
    assert e.hints == {"trace_id": "abc"}


def test_error_emits_error_event() -> None:
    events: list[TelemetryEvent] = []
    hook = TelemetryHook(events.append)
    ctx = _ctx()
    hook.before(ctx, {})
    hook.error(ctx, RuntimeError("boom"), {})
    assert events[0].type == "error"
    assert events[0].error_message == "boom"


def test_after_without_before_has_no_duration() -> None:
    events: list[TelemetryEvent] = []
    hook = TelemetryHook(events.append)
    hook.after(_ctx(), FlagEvaluationDetails(flag_key="k", value=True), {})
    assert events[0].duration_ms is None


def test_concurrent_evaluations_tracked_independently() -> None:
    events: list[TelemetryEvent] = []
    hook = TelemetryHook(events.append)
    a, b = _ctx("flagA"), _ctx("flagB")
    hook.before(a, {"id": "A"})
    hook.before(b, {"id": "B"})
    hook.after(a, FlagEvaluationDetails(flag_key="flagA", value=1), {})
    hook.after(b, FlagEvaluationDetails(flag_key="flagB", value=2), {})
    by_key = {e.flag_key: e for e in events}
    assert by_key["flagA"].hints == {"id": "A"}
    assert by_key["flagB"].hints == {"id": "B"}


def test_finally_after_cleans_up() -> None:
    hook = TelemetryHook(lambda _: None)
    ctx = _ctx()
    hook.before(ctx, {"x": 1})
    hook.finally_after(ctx, FlagEvaluationDetails(flag_key="k", value=True), {})
    assert ctx not in hook._start_times  # type: ignore[attr-defined]
    assert ctx not in hook._hints  # type: ignore[attr-defined]


def test_error_code_propagated() -> None:
    events: list[TelemetryEvent] = []
    hook = TelemetryHook(events.append)
    ctx = _ctx()
    hook.before(ctx, {})
    hook.after(
        ctx,
        FlagEvaluationDetails(
            flag_key="k",
            value=False,
            error_code=ErrorCode.TYPE_MISMATCH,
            error_message="mismatch",
            reason="ERROR",
        ),
        {},
    )
    assert events[0].error_code == ErrorCode.TYPE_MISMATCH
