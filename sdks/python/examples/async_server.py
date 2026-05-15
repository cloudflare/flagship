"""
Example: Using Flagship with OpenFeature (asynchronous)

Demonstrates async flag evaluation for Python applications using
asyncio-based frameworks such as FastAPI, Starlette, or aiohttp.
"""

import asyncio

from openfeature import api
from openfeature.evaluation_context import EvaluationContext

from flagship import FlagshipServerProvider

FLAGSHIP_APP_ID = "your-app-id"
FLAGSHIP_ACCOUNT_ID = "your-account-id"


async def main() -> None:
    # 1. Set up the Flagship provider
    api.set_provider(
        FlagshipServerProvider(
            app_id=FLAGSHIP_APP_ID,
            account_id=FLAGSHIP_ACCOUNT_ID,
            auth_token="your-token",
            timeout=5.0,
            retries=1,
        )
    )

    # 2. Get a client and evaluate flags asynchronously
    client = api.get_client()

    context = EvaluationContext(
        targeting_key="user-123",
        attributes={
            "plan": "premium",
            "country": "US",
        },
    )

    # Async evaluation — same API, just awaited
    dark_mode_enabled = await client.get_boolean_value_async("dark-mode", False, context)
    print("Dark mode enabled:", dark_mode_enabled)

    welcome_message = await client.get_string_value_async("welcome-message", "Welcome!", context)
    print("Welcome message:", welcome_message)

    max_uploads = await client.get_integer_value_async("max-uploads", 5, context)
    print("Max uploads:", max_uploads)

    # Evaluate multiple flags concurrently
    dark_mode, beta_access, upload_limit = await asyncio.gather(
        client.get_boolean_value_async("dark-mode", False, context),
        client.get_boolean_value_async("beta-access", False, context),
        client.get_integer_value_async("upload-limit", 10, context),
    )
    print("Concurrent results:", dark_mode, beta_access, upload_limit)

    # Detailed async evaluation
    details = await client.get_boolean_details_async("premium-features", False, context)
    print("Premium features details:")
    print("  value:  ", details.value)
    print("  reason: ", details.reason)
    print("  variant:", details.variant)

    # Shut down cleanly (closes the async HTTP client)
    await api.shutdown_async()


if __name__ == "__main__":
    asyncio.run(main())
