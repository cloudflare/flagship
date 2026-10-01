"""
Example: Local in-process evaluation with FlagshipServerProvider

Downloads flag definitions once, evaluates flags without a network call per
evaluation, and refreshes definitions in the background.

Requires a token with app **read** permission (not evaluate). Local evaluations
do not appear in server-side analytics.
"""

from openfeature import api
from openfeature.evaluation_context import EvaluationContext

from flagship import FlagshipServerProvider, LoggingHook

FLAGSHIP_APP_ID = "your-app-id"
FLAGSHIP_ACCOUNT_ID = "your-account-id"
FLAGSHIP_READ_TOKEN = "your-read-token"


def main() -> None:
    # set_provider_and_wait blocks until the first definitions fetch succeeds.
    api.set_provider_and_wait(
        FlagshipServerProvider(
            app_id=FLAGSHIP_APP_ID,
            account_id=FLAGSHIP_ACCOUNT_ID,  # required — used as the rollout hash seed
            auth_token=FLAGSHIP_READ_TOKEN,  # needs app **read** permission
            local_evaluation=True,
            refresh_interval=30.0,  # seconds between background refreshes (default 30s)
            logging=True,
        )
    )

    api.add_hooks([LoggingHook()])

    client = api.get_client()
    context = EvaluationContext(
        targeting_key="user-123",
        attributes={
            "email": "user@example.com",
            "plan": "premium",
        },
    )

    dark_mode = client.get_boolean_value("dark-mode", False, context)
    print("Dark mode:", dark_mode)

    details = client.get_boolean_details("premium-features", False, context)
    print("Premium features:")
    print("  value:  ", details.value)
    print("  reason: ", details.reason)  # STATIC | TARGETING_MATCH | SPLIT | DEFAULT | DISABLED
    print("  variant:", details.variant)

    api.shutdown()


if __name__ == "__main__":
    main()
