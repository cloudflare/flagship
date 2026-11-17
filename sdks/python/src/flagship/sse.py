import json
from collections.abc import Iterable, Iterator
from datetime import datetime
from urllib.parse import urljoin, urlparse


class StreamAuthError(Exception):
    pass


def stream_url(descriptors: list[dict[str, object]], base: str) -> str | None:
    urls = stream_urls(descriptors, base)
    return urls[0] if urls else None


def stream_urls(descriptors: list[dict[str, object]], base: str) -> list[str]:
    """Resolve every usable ``sse`` descriptor, ignoring unknown types and unsafe URLs."""
    urls: list[str] = []
    for descriptor in descriptors:
        if descriptor.get("type") != "sse":
            continue
        raw = descriptor.get("url")
        origin = base
        if not isinstance(raw, str):
            endpoint = descriptor.get("endpoint")
            if not isinstance(endpoint, dict):
                continue
            raw = endpoint.get("requestUri")
            candidate = endpoint.get("origin")
            if isinstance(candidate, str):
                origin = candidate
        if not isinstance(raw, str) or not raw:
            continue
        address = urljoin(origin, raw)
        parsed = urlparse(address)
        if (
            parsed.username
            or parsed.password
            or parsed.fragment
            or not parsed.hostname
            or (
                parsed.scheme != "https"
                and not (parsed.scheme == "http" and parsed.hostname in ("localhost", "127.0.0.1"))
            )
        ):
            continue
        if address not in urls:
            urls.append(address)
    return urls


def inactivity_delay(descriptors: list[dict[str, object]]) -> float | None:
    """The first valid ``inactivityDelaySec`` advertised by a descriptor, in seconds."""
    for descriptor in descriptors:
        value = descriptor.get("inactivityDelaySec")
        if isinstance(value, int) and not isinstance(value, bool) and value >= 1:
            return float(value)
    return None


def last_modified(value: object) -> int | str | None:
    """Validate ``lastModified``: Unix seconds or an ISO 8601 date-time, per OFREP."""
    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
        return value
    if isinstance(value, str) and 0 < len(value) <= 256:
        try:
            datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return None
        return value
    return None


def parse_sse_lines(lines: Iterable[str]) -> Iterator[dict[str, object]]:
    data: list[str] = []
    event_type = ""
    size = 0
    for line in lines:
        line = line.rstrip("\r\n")
        if not line:
            if data and event_type in ("", "message"):
                try:
                    message = json.loads("\n".join(data))
                except (ValueError, TypeError):
                    message = None
                if (
                    isinstance(message, dict)
                    and message.get("type") == "refetchEvaluation"
                    and ("etag" not in message or (isinstance(message["etag"], str) and len(message["etag"]) <= 256))
                ):
                    yield message
            data = []
            event_type = ""
            size = 0
        elif line.startswith("data:"):
            size += len(line)
            if size > 65536:
                raise ValueError("Configuration event too large")
            data.append(line[5:].removeprefix(" "))
        elif line.startswith("event:"):
            event_type = line[6:].strip()
