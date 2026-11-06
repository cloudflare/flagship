import math
from types import MappingProxyType
from typing import Any

from openfeature.flag_evaluation import FlagMetadata

__all__ = ["to_flag_metadata"]


def to_flag_metadata(value: Any) -> FlagMetadata:
    """Convert a metadata payload into a read-only OpenFeature ``flag_metadata`` mapping.

    Absent or malformed payloads yield an empty mapping, and non-primitive or
    non-finite entries are dropped.
    """
    if not isinstance(value, dict):
        return MappingProxyType({})
    return MappingProxyType(
        {
            key: entry
            for key, entry in value.items()
            if isinstance(entry, (str, int, float)) and not (isinstance(entry, float) and not math.isfinite(entry))
        }
    )
