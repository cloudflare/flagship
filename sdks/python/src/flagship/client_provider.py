"""OFREP static-context provider: one bulk evaluation per context, cached in memory."""

import json
import logging
import queue
import random
import threading
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any, Literal
from urllib.parse import urlsplit, urlunsplit

import httpx
from openfeature.evaluation_context import EvaluationContext
from openfeature.event import ProviderEventDetails
from openfeature.exception import (
    ErrorCode,
    FlagNotFoundError,
    GeneralError,
    InvalidContextError,
    OpenFeatureError,
    ParseError,
    ProviderNotReadyError,
    TargetingKeyMissingError,
    TypeMismatchError,
)
from openfeature.flag_evaluation import FlagResolutionDetails, FlagType, FlagValueType
from openfeature.hook import Hook
from openfeature.provider import AbstractProvider, Metadata

from ._types import FlagshipEvaluationResponse
from .client import FLAGSHIP_DEFAULT_BASE_URL, BulkEvaluation, FlagshipClient, RateLimitedError
from .context import normalize_context
from .provider import _build_details
from .sse import StreamAuthError, inactivity_delay, last_modified, parse_sse_lines, stream_urls

__all__ = ["ChangeDetection", "FlagshipClientProvider"]

_logger = logging.getLogger("flagship")

ChangeDetection = Literal["sse", "polling", "none"]

_DEFAULT_TIMEOUT = 10.0
_DEFAULT_INACTIVITY = 120.0
_COALESCE_SECONDS = 0.05
_HINT_RETRY_DELAYS = (1.0, 2.0, 4.0, 8.0, 16.0)
_MAX_RECONNECT_DELAY = 30.0
_JOIN_TIMEOUT = 5.0

_ERRORS: dict[str, Callable[[str], OpenFeatureError]] = {
    "FLAG_NOT_FOUND": FlagNotFoundError,
    "PARSE_ERROR": ParseError,
    "TYPE_MISMATCH": TypeMismatchError,
    "TARGETING_KEY_MISSING": TargetingKeyMissingError,
    "INVALID_CONTEXT": InvalidContextError,
}

_CLOSED = object()


@dataclass(frozen=True)
class _Snapshot:
    context_key: str
    flags: dict[str, dict[str, Any]]
    etag: str | None
    version: str | None


@dataclass(frozen=True)
class _Hint:
    etag: str | None
    last_modified: int | str | None


class FlagshipClientProvider(AbstractProvider):
    """OpenFeature provider for one static evaluation context, such as a single user.

    It follows the OFREP static-context provider guideline: ``initialize`` evaluates
    every flag with one bulk request, and evaluations read only that cache.

    Change detection (``change_detection``):

    - ``"sse"`` (default): connect to every event stream the server advertises and
      refresh when notified. Timer polling runs only while no stream is connected.
    - ``"polling"``: ignore event streams and refresh every ``poll_interval`` seconds.
    - ``"none"``: refresh only on ``initialize`` and ``identify``.

    ``poll_interval`` defaults to ``0``, which disables timer polling.

    The OpenFeature Python SDK does not tell providers when the evaluation context
    changes. Call :meth:`identify` with the new context and keep the OpenFeature
    context in sync. An evaluation whose non-empty context differs from the
    provider's context raises ``INVALID_CONTEXT`` rather than serving another
    context's cached values.

    Provide one of ``app_id`` + ``account_id``, ``endpoint`` (ending in
    ``/evaluate``), or ``ofrep_base_url`` (any OFREP server, up to but not
    including ``/ofrep``).
    """

    def __init__(
        self,
        *,
        app_id: str | None = None,
        account_id: str | None = None,
        endpoint: str | None = None,
        ofrep_base_url: str | None = None,
        base_url: str = FLAGSHIP_DEFAULT_BASE_URL,
        auth_token: str | None = None,
        headers: Mapping[str, str] | None = None,
        headers_factory: Callable[[], dict[str, str]] | None = None,
        context: EvaluationContext | None = None,
        timeout: float = _DEFAULT_TIMEOUT,
        retries: int = 1,
        retry_delay: float = 1.0,
        change_detection: ChangeDetection = "sse",
        poll_interval: float = 0.0,
        inactivity_delay: float | None = None,
        logging: bool = False,
    ) -> None:
        if change_detection not in ("sse", "polling", "none"):
            raise ValueError('Flagship: "change_detection" must be "sse", "polling", or "none"')
        if poll_interval < 0:
            raise ValueError('Flagship: "poll_interval" must be 0 or greater')
        if ofrep_base_url is not None:
            if app_id or endpoint:
                raise ValueError('Flagship: provide only one of "ofrep_base_url", "app_id", or "endpoint"')
            endpoint = _ofrep_endpoint(ofrep_base_url)
        static_headers = dict(headers or {})
        factory = headers_factory
        if static_headers:
            factory = lambda: {**static_headers, **(headers_factory() if headers_factory else {})}  # noqa: E731
        self._client = FlagshipClient(
            app_id=app_id,
            account_id=account_id,
            endpoint=endpoint,
            base_url=base_url,
            auth_token=auth_token,
            headers_factory=factory,
            timeout=timeout,
            retries=retries,
            retry_delay=retry_delay,
        )
        if not self._client.bulk_endpoint:
            raise ValueError('Flagship: the static-context provider requires an endpoint ending in "/evaluate"')
        self._initial_context = context
        self._change_detection: ChangeDetection = change_detection
        self._poll_interval = poll_interval
        self._inactivity_override = inactivity_delay
        self._logging = logging

        self._context: EvaluationContext = EvaluationContext()
        self._snapshot: _Snapshot | None = None
        self._lock = threading.RLock()
        self._refresh_lock = threading.Lock()
        self._epoch = 0
        self._stop = threading.Event()
        self._threads: list[threading.Thread] = []
        self._session: _StreamSession | None = None
        self._pause_timer: threading.Timer | None = None
        self._blocked_until = 0.0
        self._stale = False
        self._fatal = False

    def get_metadata(self) -> Metadata:
        return Metadata(name="Flagship Client Provider")

    def get_provider_hooks(self) -> list[Hook]:
        return []

    def initialize(self, evaluation_context: EvaluationContext) -> None:
        """Evaluate every flag for the provider's context and start change detection.

        Uses ``context`` from the constructor when given, otherwise the context the
        OpenFeature SDK passes. Raises on failure, putting the provider in ``ERROR``.
        """
        self._load(self._initial_context if self._initial_context is not None else evaluation_context, initial=True)

    def identify(self, evaluation_context: EvaluationContext) -> None:
        """Re-evaluate every flag for a new context and reconnect change detection.

        The request is unconditional, so the previous context's ``ETag`` never
        validates the new context's results. On failure the cache is cleared and
        the exception propagates.
        """
        self._load(evaluation_context, initial=False)

    def shutdown(self) -> None:
        self._stop_background()
        with self._lock:
            self._snapshot = None
            self._stale = False
            self._fatal = False

    def pause(self) -> None:
        """Tell the provider the host became inactive, such as a backgrounded app.

        Event streams close after the effective inactivity delay: the constructor's
        ``inactivity_delay``, else the server's ``inactivityDelaySec``, else 120 s.
        """
        with self._lock:
            session = self._session
            if session is None or self._pause_timer is not None:
                return
            delay = self._inactivity_override or session.inactivity_delay or _DEFAULT_INACTIVITY
            timer = threading.Timer(delay, session.suspend)
            timer.daemon = True
            self._pause_timer = timer
        timer.start()

    def resume(self) -> None:
        """Tell the provider the host became active again.

        Reconnects a stream closed for inactivity with an unconditional refresh.
        Without a stream, revalidates the cache with ``If-None-Match`` unless
        ``change_detection`` is ``"none"``.
        """
        with self._lock:
            if self._pause_timer is not None:
                self._pause_timer.cancel()
                self._pause_timer = None
            session = self._session
        if session is not None:
            session.resume()
        elif self._change_detection != "none" and self._snapshot is not None and not self._fatal:
            self._refresh("revalidate")

    def resolve_boolean_details(
        self, flag_key: str, default_value: bool, evaluation_context: EvaluationContext | None = None
    ) -> FlagResolutionDetails[bool]:
        return self._resolve(FlagType.BOOLEAN, flag_key, default_value, evaluation_context)

    def resolve_string_details(
        self, flag_key: str, default_value: str, evaluation_context: EvaluationContext | None = None
    ) -> FlagResolutionDetails[str]:
        return self._resolve(FlagType.STRING, flag_key, default_value, evaluation_context)

    def resolve_integer_details(
        self, flag_key: str, default_value: int, evaluation_context: EvaluationContext | None = None
    ) -> FlagResolutionDetails[int]:
        return self._resolve(FlagType.INTEGER, flag_key, default_value, evaluation_context)

    def resolve_float_details(
        self, flag_key: str, default_value: float, evaluation_context: EvaluationContext | None = None
    ) -> FlagResolutionDetails[float]:
        return self._resolve(FlagType.FLOAT, flag_key, default_value, evaluation_context)

    def resolve_object_details(
        self,
        flag_key: str,
        default_value: Sequence[FlagValueType] | Mapping[str, FlagValueType],
        evaluation_context: EvaluationContext | None = None,
    ) -> FlagResolutionDetails[Sequence[FlagValueType] | Mapping[str, FlagValueType]]:
        return self._resolve(FlagType.OBJECT, flag_key, default_value, evaluation_context)

    def _resolve(
        self,
        flag_type: FlagType,
        flag_key: str,
        default_value: Any,
        evaluation_context: EvaluationContext | None,
    ) -> FlagResolutionDetails[Any]:
        snapshot = self._snapshot
        if snapshot is None:
            raise ProviderNotReadyError("Flagship flags have not been evaluated for a context yet")
        if _has_values(evaluation_context) and _context_key(evaluation_context) != snapshot.context_key:
            raise InvalidContextError(
                "The evaluation context differs from the provider's static context; call identify() with it"
            )
        entry = snapshot.flags.get(flag_key)
        if entry is None:
            raise FlagNotFoundError(f'Flag "{flag_key}" was not returned by the bulk evaluation')
        error_code = entry.get("errorCode")
        if isinstance(error_code, str):
            details = entry.get("errorDetails")
            message = details if isinstance(details, str) else f'Flag "{flag_key}" evaluation failed'
            raise _ERRORS.get(error_code, GeneralError)(message)
        variant = entry.get("variant")
        reason = entry.get("reason")
        response = FlagshipEvaluationResponse(
            flag_key=flag_key,
            value=entry.get("value"),
            variant=variant if isinstance(variant, str) else "",
            reason=reason if isinstance(reason, str) else "DEFAULT",
            metadata=entry["metadata"] if isinstance(entry.get("metadata"), dict) else {},
        )
        return _build_details(flag_type, default_value, response)

    def _load(self, evaluation_context: EvaluationContext, *, initial: bool) -> None:
        self._stop_background()
        with self._lock:
            self._epoch += 1
            epoch = self._epoch
            previous = self._snapshot
            self._snapshot = None
            self._context = evaluation_context
            self._stale = False
            self._fatal = False
            self._stop = threading.Event()
        try:
            result = self._fetch("discover")
        except Exception:
            self._log("[Flagship] Bulk evaluation failed during %s", "initialization" if initial else "identify")
            raise
        if result is None:
            raise ParseError("Unexpected 304 for an unconditional bulk evaluation")
        with self._lock:
            if epoch != self._epoch:
                return
            self._snapshot = _snapshot(evaluation_context, result)
            stop = self._stop
            if self._change_detection == "sse" and stream_urls(result.event_streams, self._client.bulk_endpoint or ""):
                self._session = _StreamSession(self, epoch, stop, result.event_streams)
                self._start(self._session.run, "flagship-event-streams")
            if self._change_detection != "none" and self._poll_interval > 0:
                self._start(lambda: self._poll_loop(epoch, stop), "flagship-poll")
        if not initial:
            self._emit_changed(previous, self._snapshot)

    def _start(self, target: Callable[[], None], name: str) -> None:
        thread = threading.Thread(target=target, name=name, daemon=True)
        self._threads.append(thread)
        thread.start()

    def _stop_background(self) -> None:
        with self._lock:
            self._epoch += 1
            self._stop.set()
            if self._pause_timer is not None:
                self._pause_timer.cancel()
                self._pause_timer = None
            session, self._session = self._session, None
            threads, self._threads = self._threads, []
        if session is not None:
            session.close()
        current = threading.current_thread()
        for thread in threads:
            if thread is not current and thread.is_alive():
                thread.join(timeout=_JOIN_TIMEOUT)

    def _poll_loop(self, epoch: int, stop: threading.Event) -> None:
        while not stop.wait(self._poll_interval):
            if epoch != self._epoch or self._fatal:
                return
            session = self._session
            if self._change_detection == "sse" and session is not None and session.connected:
                continue
            self._refresh("revalidate")

    def _fetch(
        self, kind: Literal["discover", "hint", "revalidate"], hint: _Hint | None = None
    ) -> BulkEvaluation | None:
        """``discover`` is unconditional and requests streams; ``hint`` forwards event
        metadata; ``revalidate`` sends the stored ``ETag``."""
        wait = self._blocked_until - time.monotonic()
        if wait > 0:
            raise RateLimitedError("Bulk evaluation is rate limited until the Retry-After time", wait)
        snapshot = self._snapshot
        etag = snapshot.etag if snapshot is not None else None
        session = self._session
        wants_stream = self._change_detection == "sse" and (session is None or not session.connected)
        try:
            if kind == "discover":
                return self._client.fetch_bulk(self._context, request_event_stream=self._change_detection == "sse")
            if kind == "hint":
                return self._client.fetch_bulk(
                    self._context,
                    if_none_match=etag,
                    flag_config_etag=hint.etag if hint else None,
                    flag_config_last_modified=hint.last_modified if hint else None,
                )
            return self._client.fetch_bulk(self._context, if_none_match=etag, request_event_stream=wants_stream)
        except RateLimitedError as error:
            if error.retry_after is not None:
                self._blocked_until = time.monotonic() + error.retry_after
            raise

    def _refresh(
        self,
        kind: Literal["discover", "hint", "revalidate"],
        hint: _Hint | None = None,
        notification: bool = False,
        epoch: int | None = None,
    ) -> BulkEvaluation | None | bool:
        """Apply one refresh. Returns the result (``None`` for 304), or ``False`` on failure."""
        epoch = self._epoch if epoch is None else epoch
        if self._fatal:
            return False
        with self._refresh_lock:
            try:
                result = self._fetch(kind, hint)
            except StreamAuthError as error:
                if epoch == self._epoch:
                    self._fail(str(error))
                return False
            except Exception as error:
                if epoch == self._epoch:
                    self._mark_stale(str(error))
                return False
            with self._lock:
                if epoch != self._epoch:
                    return False
                previous = self._snapshot
                if result is not None:
                    self._snapshot = _snapshot(self._context, result)
                current = self._snapshot
            self._recover()
            if result is not None:
                self._emit_changed(previous, current, force=notification)
                session = self._session
                if (
                    self._change_detection == "sse"
                    and session is None
                    and stream_urls(result.event_streams, self._client.bulk_endpoint or "")
                ):
                    with self._lock:
                        if epoch == self._epoch and self._session is None:
                            self._session = _StreamSession(self, epoch, self._stop, result.event_streams)
                            self._start(self._session.run, "flagship-event-streams")
            return result

    def _emit_changed(self, previous: _Snapshot | None, current: _Snapshot | None, *, force: bool = False) -> None:
        if current is None:
            return
        old = previous.flags if previous is not None else {}
        changed = sorted(key for key in set(old) | set(current.flags) if old.get(key) != current.flags.get(key))
        version_changed = previous is not None and current.version is not None and current.version != previous.version
        if force or changed or version_changed:
            self.emit_provider_configuration_changed(ProviderEventDetails(flags_changed=changed or None))

    def _mark_stale(self, message: str) -> None:
        self._log("[Flagship] Refresh failed; keeping cached flags: %s", message)
        if self._stale or self._fatal or self._snapshot is None:
            return
        self._stale = True
        self.emit_provider_stale(ProviderEventDetails(message=message))

    def _recover(self) -> None:
        if not self._stale:
            return
        self._stale = False
        self.emit_provider_ready(ProviderEventDetails())

    def _fail(self, message: str) -> None:
        """Stop background refreshes after the server rejects the credentials."""
        self._fatal = True
        with self._lock:
            self._stop.set()
            session, self._session = self._session, None
        if session is not None:
            session.close()
        self.emit_provider_error(ProviderEventDetails(message=message, error_code=ErrorCode.PROVIDER_FATAL))

    def _log(self, message: str, *args: Any) -> None:
        if self._logging:
            _logger.warning(message, *args)


class _StreamSession:
    """Holds every advertised event stream open and turns their hints into refreshes.

    Tickets in stream URLs are single-use, so a dropped connection is replaced by
    rediscovering fresh descriptors, never by reopening an old URL.
    """

    def __init__(
        self, provider: FlagshipClientProvider, epoch: int, stop: threading.Event, descriptors: list[dict[str, object]]
    ) -> None:
        self._provider = provider
        self._epoch = epoch
        self._stop = stop
        self._descriptors: list[dict[str, object]] | None = descriptors
        self._lock = threading.Lock()
        self._responses: list[httpx.Response] = []
        self._events: queue.Queue[object] = queue.Queue()
        self._suspended = threading.Event()
        self._wake = threading.Event()
        self.connected = False
        self.inactivity_delay = inactivity_delay(descriptors)

    def run(self) -> None:
        delay = 0.5
        while self._active():
            if self._suspended.is_set():
                self._wake.wait(0.5)
                self._wake.clear()
                continue
            descriptors = self._descriptors
            self._descriptors = None
            if descriptors is None:
                result = self._provider._refresh("discover", epoch=self._epoch)
                descriptors = result.event_streams if isinstance(result, BulkEvaluation) else []
            urls = stream_urls(descriptors, self._provider._client.bulk_endpoint or "")
            if urls:
                self.inactivity_delay = inactivity_delay(descriptors) or self.inactivity_delay
                if self._listen(urls):
                    delay = 0.5
            if not self._active() or self._suspended.is_set():
                continue
            self._provider._mark_stale("Configuration stream interrupted")
            if self._stop.wait(delay * (0.75 + random.random() * 0.5)):
                return
            delay = min(_MAX_RECONNECT_DELAY, delay * 2)

    def suspend(self) -> None:
        """Close the streams for inactivity; :meth:`resume` reconnects them."""
        self._suspended.set()
        self._close_responses()

    def resume(self) -> None:
        if not self._suspended.is_set():
            return
        self._suspended.clear()
        self._wake.set()

    def close(self) -> None:
        self._stop.set()
        self._wake.set()
        self._close_responses()

    def _active(self) -> bool:
        return not self._stop.is_set() and self._epoch == self._provider._epoch

    def _listen(self, urls: list[str]) -> bool:
        """Hold every stream until one ends. Returns whether all of them connected."""
        self._events = queue.Queue()
        readers = [threading.Thread(target=self._read, args=(url,), daemon=True) for url in urls]
        for reader in readers:
            reader.start()
        ever_connected = False
        try:
            while self._active():
                try:
                    item = self._events.get(timeout=0.5)
                except queue.Empty:
                    with self._lock:
                        self.connected = len(self._responses) == len(urls)
                    ever_connected = ever_connected or self.connected
                    continue
                if item is _CLOSED:
                    return ever_connected
                hint = item
                closed = False
                # Coalesce bursts, including one change announced on several streams.
                deadline = time.monotonic() + _COALESCE_SECONDS
                while (remaining := deadline - time.monotonic()) > 0:
                    try:
                        following = self._events.get(timeout=remaining)
                    except queue.Empty:
                        break
                    if following is _CLOSED:
                        closed = True
                        break
                    hint = following
                assert isinstance(hint, _Hint)
                self._refresh_for(hint)
                if closed:
                    return ever_connected
            return ever_connected
        finally:
            self.connected = False
            self._close_responses()
            for reader in readers:
                reader.join(timeout=_JOIN_TIMEOUT)

    def _refresh_for(self, hint: _Hint) -> None:
        """Refresh until the observed version matches the hinted one, with bounded retries."""
        for attempt, delay in enumerate((0.0, *_HINT_RETRY_DELAYS)):
            if delay and self._stop.wait(delay):
                return
            result = self._provider._refresh("hint", hint, notification=attempt == 0, epoch=self._epoch)
            if result is False:
                continue
            snapshot = self._provider._snapshot
            if hint.etag is None or (snapshot is not None and snapshot.version == hint.etag):
                return

    def _read(self, url: str) -> None:
        client = self._provider._client
        timeout = httpx.Timeout(None, connect=client.timeout if client.timeout > 0 else _DEFAULT_TIMEOUT)
        try:
            with client._sync_client.stream(
                "GET", url, headers={"Accept": "text/event-stream"}, timeout=timeout, follow_redirects=False
            ) as response:
                content_type = response.headers.get("content-type", "").lower()
                if response.status_code != 200 or not content_type.startswith("text/event-stream"):
                    return
                with self._lock:
                    self._responses.append(response)
                for message in parse_sse_lines(response.iter_lines()):
                    etag = message.get("etag")
                    self._events.put(
                        _Hint(
                            etag=etag if isinstance(etag, str) else None,
                            last_modified=last_modified(message.get("lastModified")),
                        )
                    )
        except Exception:
            # Stream URLs carry credentials, so the error (which may include the URL) is not logged.
            pass
        finally:
            self._events.put(_CLOSED)

    def _close_responses(self) -> None:
        with self._lock:
            responses, self._responses = self._responses, []
        for response in responses:
            try:
                response.close()
            except Exception:
                pass


def _ofrep_endpoint(base: str) -> str:
    parts = urlsplit(base)
    if parts.scheme not in ("http", "https") or not parts.netloc:
        raise ValueError(f'Flagship: "ofrep_base_url" must be an absolute http or https URL: {base}')
    return urlunsplit((parts.scheme, parts.netloc, parts.path.rstrip("/") + "/evaluate", "", ""))


def _has_values(context: EvaluationContext | None) -> bool:
    return context is not None and (bool(context.targeting_key) or bool(context.attributes))


def _context_key(context: EvaluationContext | None) -> str:
    return json.dumps(normalize_context(context).values, sort_keys=True, separators=(",", ":"), default=str)


def _snapshot(context: EvaluationContext, result: BulkEvaluation) -> _Snapshot:
    return _Snapshot(context_key=_context_key(context), flags=result.flags, etag=result.etag, version=result.version)
