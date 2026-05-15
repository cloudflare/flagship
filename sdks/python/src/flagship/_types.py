from dataclasses import dataclass
from typing import Any, Literal

__all__ = ["FlagshipEvaluationResponse"]

EvaluationReason = Literal["TARGETING_MATCH", "DEFAULT", "DISABLED", "SPLIT"]


@dataclass
class FlagshipEvaluationResponse:
    flag_key: str
    value: Any
    variant: str
    reason: EvaluationReason
