"""
Example: Using Flagship with OpenFeature (synchronous)

Demonstrates server-side flag evaluation for Python applications
such as Django, Flask, or any synchronous server environment.
"""

from openfeature import api
from openfeature.evaluation_context import EvaluationContext

from flagship import FlagshipServerProvider

FLAGSHIP_APP_ID = "your-app-id"
FLAGSHIP_ACCOUNT_ID = "your-account-id"


def main() -> None:
    # 1. Set up the Flagship provider
    api.set_provider(
        FlagshipServerProvider(
            app_id=FLAGSHIP_APP_ID,
            account_id=FLAGSHIP_ACCOUNT_ID,
            auth_token="your-token",
            timeout=5.0,
            retries=1,
            logging=True,
        )
    )

    # 2. Get a client and evaluate flags with context
    client = api.get_client()

    context = EvaluationContext(
        targeting_key="user-123",  # user identifier for targeting rules
        attributes={
            "email": "user@example.com",
            "plan": "premium",
            "country": "US",
            "age": 25,
        },
    )

    # Boolean flag
    dark_mode_enabled = client.get_boolean_value("dark-mode", False, context)
    print("Dark mode enabled:", dark_mode_enabled)

    # String flag — e.g. for A/B testing copy
    welcome_message = client.get_string_value("welcome-message", "Welcome!", context)
    print("Welcome message:", welcome_message)

    # Integer flag — e.g. for feature limits
    max_uploads = client.get_integer_value("max-uploads", 5, context)
    print("Max uploads:", max_uploads)

    # Float flag — e.g. for sampling rates
    sample_rate = client.get_float_value("sample-rate", 0.1, context)
    print("Sample rate:", sample_rate)

    # Object flag — e.g. for complex configuration
    theme_config = client.get_object_value("theme-config", {"primary_color": "#000000", "font_size": 14}, context)
    print("Theme config:", theme_config)

    # 3. Detailed evaluation — reason reflects how the flag resolved
    # ('TARGETING_MATCH', 'DEFAULT', 'DISABLED', 'SPLIT')
    details = client.get_boolean_details("premium-features", False, context)
    print("Premium features details:")
    print("  value:  ", details.value)
    print("  reason: ", details.reason)  # why this value was served
    print("  variant:", details.variant)  # which variation key was matched

    # 4. Non-existent flags return the default value — no exceptions raised
    unknown = client.get_boolean_value("non-existent-flag", False, context)
    print("Unknown flag (returns default):", unknown)

    # 5. Evaluate without context — rules that don't require targeting still apply
    beta_access = client.get_boolean_value("beta-access", False)
    print("Beta access:", beta_access)

    # 6. Shut down cleanly
    api.shutdown()


if __name__ == "__main__":
    main()
