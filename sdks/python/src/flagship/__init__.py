from importlib.metadata import version

from ._types import FlagshipEvaluationResponse
from .client import FLAGSHIP_DEFAULT_BASE_URL, FlagshipClient
from .context import context_to_query_params
from .provider import FlagshipServerProvider

__version__ = version("cloudflare-flagship")

__all__ = [
    "__version__",
    "FLAGSHIP_DEFAULT_BASE_URL",
    "FlagshipClient",
    "FlagshipEvaluationResponse",
    "FlagshipServerProvider",
    "context_to_query_params",
]
