# cloudflare-flagship

[![PyPI version](https://img.shields.io/pypi/v/cloudflare-flagship.svg)](https://pypi.org/project/cloudflare-flagship/)
[![Python versions](https://img.shields.io/pypi/pyversions/cloudflare-flagship.svg)](https://pypi.org/project/cloudflare-flagship/)
[![license](https://img.shields.io/pypi/l/cloudflare-flagship.svg)](LICENSE)

Flagship is a globally distributed, low-latency feature flag platform built entirely on Cloudflare. This package is the Python SDK — an [OpenFeature](https://openfeature.dev)-compliant provider for evaluating feature flags from Python server environments.

> **Note:** The Python SDK supports HTTP mode only. The Cloudflare Workers binding mode (`env.FLAGS`) is exclusive to the TypeScript SDK and is not available in Python.

## Installation

```sh
# uv
uv add cloudflare-flagship

# pip
pip install cloudflare-flagship
```

## Quick start

```python
from openfeature import api
from openfeature.evaluation_context import EvaluationContext
from flagship import FlagshipServerProvider

api.set_provider(
    FlagshipServerProvider(
        app_id="your-app-id",
        account_id="your-account-id",
        auth_token="your-token",
    )
)

client = api.get_client()
enabled = client.get_boolean_value(
    "dark-mode",
    False,
    EvaluationContext(targeting_key="user-123", attributes={"plan": "premium"}),
)
```

See [`examples/server.py`](examples/server.py) for a full synchronous example and [`examples/async_server.py`](examples/async_server.py) for async usage with `asyncio`.

## Flag types

All four OpenFeature flag types are supported. Python's OpenFeature SDK splits the TypeScript `number` type into `integer` and `float`.

```python
enabled = client.get_boolean_value("new-checkout", False, context)
variant = client.get_string_value("homepage-hero", "control", context)
limit   = client.get_integer_value("upload-limit", 10, context)
rate    = client.get_float_value("sample-rate", 0.1, context)
config  = client.get_object_value("ui-config", {"theme": "light"}, context)
```

Use the `*_details` variants when you need the full resolution result:

```python
details = client.get_boolean_details("my-flag", False, context)

print(details.value)          # resolved value (or default on error)
print(details.reason)         # TARGETING_MATCH | SPLIT | DEFAULT | DISABLED | ERROR
print(details.variant)        # variation key, e.g. "on", "off", "v2"
print(details.error_code)     # set on error, e.g. FLAG_NOT_FOUND, TYPE_MISMATCH
print(details.error_message)
```

## Configuration

`FlagshipServerProvider` accepts either `app_id` + `account_id` (recommended) or a full `endpoint` URL — not both.

```python
FlagshipServerProvider(
    # Option A (recommended)
    app_id="your-app-id",
    account_id="your-account-id",

    # Option B: full URL (mutually exclusive with app_id)
    # endpoint="http://localhost:8787/v1/acct/apps/app-id/evaluate",

    # Static bearer token
    auth_token="your-token",

    # Dynamic credentials — called once per request, takes precedence over auth_token
    # headers_factory=lambda: {"Authorization": f"Bearer {get_token()}"},

    # Override the base URL for local dev
    # base_url="http://localhost:8787",

    timeout=5.0,      # seconds (default: 5.0)
    retries=1,        # retry attempts on transient errors, capped at 10 (default: 1)
    retry_delay=1.0,  # seconds between retries, capped at 30.0 (default: 1.0)
    logging=False,    # set True to enable SDK debug output (default: False)
)
```

| Option            | Type                           | Default                      | Description                                              |
| ----------------- | ------------------------------ | ---------------------------- | -------------------------------------------------------- |
| `app_id`          | `str`                          | —                            | Flagship app ID (mutually exclusive with `endpoint`)     |
| `account_id`      | `str`                          | —                            | Required with `app_id`                                   |
| `base_url`        | `str`                          | `https://api.cloudflare.com` | Base URL override (only used with `app_id`)              |
| `endpoint`        | `str`                          | —                            | Full evaluation URL (mutually exclusive with `app_id`)   |
| `auth_token`      | `str`                          | —                            | Bearer token added to every request                      |
| `headers_factory` | `Callable[[], dict[str, str]]` | —                            | Called per request; takes precedence over `auth_token`   |
| `timeout`         | `float`                        | `5.0`                        | Request timeout in seconds                               |
| `retries`         | `int`                          | `1`                          | Retry attempts on transient errors; capped at `10`       |
| `retry_delay`     | `float`                        | `1.0`                        | Delay between retries in seconds; capped at `30.0`       |
| `logging`         | `bool`                         | `False`                      | Enable SDK-level debug output via the `flagship` logger  |

## Evaluation context

Context attributes are sent as URL query parameters. Supported types:

| Type                  | Serialisation                                    |
| --------------------- | ------------------------------------------------ |
| `str`, `int`, `float` | Passed as a string                               |
| `bool`                | `"true"` or `"false"`                            |
| `datetime`            | ISO 8601                                         |
| `dict`, `list`, other | **Not supported** — raises `InvalidContextError` |

## Async

The async API mirrors the sync API — just `await` the `*_async` variants:

```python
enabled = await client.get_boolean_value_async("dark-mode", False, context)
details = await client.get_boolean_details_async("dark-mode", False, context)

# Evaluate multiple flags concurrently
import asyncio
dark_mode, beta_access = await asyncio.gather(
    client.get_boolean_value_async("dark-mode", False, context),
    client.get_boolean_value_async("beta-access", False, context),
)
```

When shutting down in an async context, use `shutdown_async()` to properly close the HTTP client:

```python
await api.shutdown_async()
```

## Error handling

The provider never throws from a resolution method. On error the OpenFeature SDK returns the default value with an `error_code` and `error_message`.

| Error code        | Cause                                                          |
| ----------------- | -------------------------------------------------------------- |
| `FLAG_NOT_FOUND`  | Flag key does not exist (HTTP 404)                             |
| `TYPE_MISMATCH`   | The flag's resolved type does not match the requested type     |
| `INVALID_CONTEXT` | The evaluation context contains unsupported types (dict, list) |
| `PARSE_ERROR`     | The API response was not a valid evaluation response           |
| `GENERAL`         | Network error, timeout, or any other transient failure         |

404 and 400 responses are never retried. All other failures are retried up to `retries` times.

## Hooks

```python
from flagship import LoggingHook, TelemetryHook

# Logs evaluation lifecycle events via the flagship logger (INFO level)
api.add_hooks([LoggingHook()])

# Emits a TelemetryEvent after every evaluation
api.add_hooks([TelemetryHook(lambda event: analytics.track("flag_evaluated", event))])
```

`TelemetryEvent` fields: `type`, `flag_key`, `timestamp`, `duration_ms`, `value`, `reason`, `variant`, `error_code`, `error_message`, `context`, `hints`.

## Provider events

```python
from openfeature.event import ProviderEvent

api.add_handler(ProviderEvent.PROVIDER_READY, lambda _: print("ready"))
api.add_handler(ProviderEvent.PROVIDER_ERROR, lambda d: print("error:", d.message))
```

During initialisation the provider probes the endpoint with a health-check request. A 404 is treated as success. Any other error transitions the provider to `ERROR` status and emits `PROVIDER_ERROR`.

## Development

```sh
uv sync --group dev        # install dependencies
uv run pytest              # run tests
uv run mypy src            # type check
uv build                   # build wheel and sdist
```

## License

[Apache-2.0](LICENSE)
