import pytest

from flagship import FlagshipServerProvider


@pytest.fixture
def provider() -> FlagshipServerProvider:
    return FlagshipServerProvider(app_id="app-123", account_id="acct-456")


@pytest.fixture
def endpoint_regex() -> str:
    return r".*/evaluate.*"
