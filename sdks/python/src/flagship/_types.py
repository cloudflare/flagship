from dataclasses import dataclass
from typing import Any

__all__ = ["FlagshipEvaluationResponse"]


@dataclass
class FlagshipEvaluationResponse:
    flag_key: str
    value: Any
    variant: str
    reason: str
