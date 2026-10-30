//! Transport-agnostic HTTP retry policy and execution.

use std::future::Future;
use std::time::Duration;

use crate::config::{MAX_RETRIES, MAX_RETRY_DELAY};
use crate::error::FlagshipError;

/// A fixed-delay retry policy.
///
/// `retries` counts attempts after the initial attempt. Values are capped here
/// as a defensive measure even though client configuration performs the same
/// validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetryPolicy {
    retries: u32,
    delay: Duration,
}

impl RetryPolicy {
    /// Creates a policy with bounded retry count and delay.
    pub(crate) fn new(retries: u32, delay: Duration) -> Self {
        Self {
            retries: retries.min(MAX_RETRIES),
            delay: delay.min(MAX_RETRY_DELAY),
        }
    }

    /// Returns each permitted attempt, including the initial attempt.
    fn attempts(self) -> Attempts {
        Attempts {
            next: 0,
            total: self.retries as usize + 1,
        }
    }

    /// Runs an operation until it succeeds or the policy stops retrying it.
    ///
    /// The operation receives a zero-based [`Attempt`]. It remains responsible
    /// for all per-attempt work, including rebuilding request bodies, producing
    /// headers, and applying a timeout. Dropping this future also drops any
    /// in-progress operation or Tokio delay, so cancellation does not start a
    /// later attempt.
    pub(crate) async fn run<T, F, Fut>(self, mut operation: F) -> Result<T, FlagshipError>
    where
        F: FnMut(Attempt) -> Fut,
        Fut: Future<Output = Result<T, FlagshipError>>,
    {
        for attempt in self.attempts() {
            match operation(attempt).await {
                Ok(value) => return Ok(value),
                Err(error) if !error.is_retryable() || attempt.is_last() => {
                    return Err(error);
                }
                Err(error) => drop(error),
            }

            if !self.delay.is_zero() {
                sleep(self.delay).await;
            }
        }

        unreachable!("a retry policy always permits an initial attempt")
    }
}

/// One invocation allowed by a [`RetryPolicy`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Attempt {
    number: usize,
    total: usize,
}

impl Attempt {
    /// Returns the zero-based attempt number.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "reserved for per-attempt diagnostics")
    )]
    pub(crate) const fn number(self) -> usize {
        self.number
    }

    /// Returns whether this is the final attempt permitted by the policy.
    pub(crate) const fn is_last(self) -> bool {
        self.number + 1 == self.total
    }
}

/// Iterator over the initial attempt and all configured retry attempts.
#[derive(Clone, Debug)]
struct Attempts {
    next: usize,
    total: usize,
}

impl Iterator for Attempts {
    type Item = Attempt;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.total {
            return None;
        }

        let attempt = Attempt {
            number: self.next,
            total: self.total,
        };
        self.next += 1;

        Some(attempt)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.total - self.next;

        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Attempts {}

/// Waits for a retry delay using Tokio's cancellation-safe timer.
///
/// Dropping the returned future cancels the wait.
async fn sleep(delay: Duration) {
    tokio::time::sleep(delay).await;
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};

    use tokio::task::yield_now;
    use tokio::time::{Instant, advance};

    use super::*;
    use crate::error::ErrorCode;

    fn retryable_error(message: &str) -> FlagshipError {
        FlagshipError::new(ErrorCode::NetworkError, message)
    }

    fn terminal_error(message: &str) -> FlagshipError {
        FlagshipError::new(ErrorCode::BadRequest, message)
    }

    fn poll_ready<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);

        match Future::poll(Pin::as_mut(&mut future), &mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("future unexpectedly required an executor"),
        }
    }

    #[test]
    fn attempts_include_initial_and_configured_retries() {
        let zero = RetryPolicy::new(0, Duration::ZERO)
            .attempts()
            .map(Attempt::number)
            .collect::<Vec<_>>();
        let one = RetryPolicy::new(1, Duration::ZERO)
            .attempts()
            .map(Attempt::number)
            .collect::<Vec<_>>();
        let maximum = RetryPolicy::new(MAX_RETRIES, Duration::ZERO)
            .attempts()
            .map(Attempt::number)
            .collect::<Vec<_>>();

        assert_eq!(zero, [0]);
        assert_eq!(one, [0, 1]);
        assert_eq!(maximum, (0..=MAX_RETRIES as usize).collect::<Vec<_>>());
    }

    #[test]
    fn policy_defensively_caps_retries_and_delay() {
        let policy = RetryPolicy::new(u32::MAX, Duration::MAX);
        let attempts = policy.attempts().collect::<Vec<_>>();

        assert_eq!(attempts.len(), MAX_RETRIES as usize + 1);
        assert_eq!(
            attempts.last().map(|attempt| attempt.number()),
            Some(MAX_RETRIES as usize)
        );
        assert!(attempts.last().is_some_and(|attempt| attempt.is_last()));
        assert_eq!(policy.delay, MAX_RETRY_DELAY);
    }

    #[tokio::test(start_paused = true)]
    async fn succeeds_after_a_transient_failure() {
        let mut attempts = Vec::new();
        let start = Instant::now();

        let result = RetryPolicy::new(1, Duration::from_secs(2))
            .run(|attempt| {
                attempts.push(attempt.number());
                let result = if attempt.number() == 0 {
                    Err(retryable_error("transient"))
                } else {
                    Ok("done")
                };

                std::future::ready(result)
            })
            .await;

        assert_eq!(result.expect("second attempt should succeed"), "done");
        assert_eq!(attempts, [0, 1]);
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn returns_terminal_failure_without_retrying() {
        let calls = AtomicUsize::new(0);
        let start = Instant::now();

        let error = RetryPolicy::new(10, Duration::from_secs(1))
            .run(|_| {
                calls.fetch_add(1, Ordering::Relaxed);

                std::future::ready(Err::<(), _>(terminal_error("terminal")))
            })
            .await
            .expect_err("terminal failure should be returned");

        assert_eq!(error.message(), "terminal");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn returns_last_failure_after_exhausting_retries() {
        let calls = AtomicUsize::new(0);
        let start = Instant::now();

        let error = RetryPolicy::new(2, Duration::from_millis(50))
            .run(|attempt| {
                calls.fetch_add(1, Ordering::Relaxed);

                std::future::ready(Err::<(), _>(retryable_error(&format!(
                    "attempt {}",
                    attempt.number()
                ))))
            })
            .await
            .expect_err("all attempts should fail");

        assert_eq!(error.message(), "attempt 2");
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert_eq!(start.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn waits_exact_fixed_delay_between_attempts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let operation_calls = Arc::clone(&calls);
        let delay = Duration::from_secs(5);
        let start = Instant::now();
        let task = tokio::spawn(async move {
            RetryPolicy::new(2, delay)
                .run(|attempt| {
                    operation_calls.fetch_add(1, Ordering::Relaxed);
                    let result = if attempt.is_last() {
                        Ok(())
                    } else {
                        Err(retryable_error("retry"))
                    };

                    std::future::ready(result)
                })
                .await
        });

        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(start.elapsed(), Duration::ZERO);

        advance(delay.saturating_sub(Duration::from_nanos(1))).await;
        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            start.elapsed(),
            delay.saturating_sub(Duration::from_nanos(1))
        );

        advance(Duration::from_nanos(1)).await;
        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(start.elapsed(), delay);

        advance(delay).await;
        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert_eq!(start.elapsed(), delay * 2);
        assert!(task.is_finished());

        task.await
            .expect("retry task should not panic")
            .expect("final attempt should succeed");
        assert_eq!(start.elapsed(), delay * 2);
    }

    #[test]
    fn zero_delay_does_not_require_a_tokio_time_driver() {
        let calls = AtomicUsize::new(0);
        let result = poll_ready(RetryPolicy::new(2, Duration::ZERO).run(|_| {
            let call = calls.fetch_add(1, Ordering::Relaxed);
            let result = if call < 2 {
                Err(retryable_error("retry"))
            } else {
                Ok("done")
            };

            std::future::ready(result)
        }));

        assert_eq!(result.expect("third attempt should succeed"), "done");
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_overrides_work_in_both_directions() {
        let delay = Duration::from_secs(4);
        let start = Instant::now();

        let retried = RetryPolicy::new(1, delay)
            .run(|attempt| {
                let result = if attempt.number() == 0 {
                    Err(terminal_error("overridden").with_retryable(true))
                } else {
                    Ok(())
                };

                std::future::ready(result)
            })
            .await;

        assert!(retried.is_ok());
        assert_eq!(start.elapsed(), delay);

        let calls = AtomicUsize::new(0);
        let terminal_start = Instant::now();
        let error = RetryPolicy::new(2, delay)
            .run(|_| {
                calls.fetch_add(1, Ordering::Relaxed);

                std::future::ready(Err::<(), _>(
                    retryable_error("overridden").with_retryable(false),
                ))
            })
            .await
            .expect_err("non-retryable override should stop immediately");

        assert!(!error.is_retryable());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(terminal_start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_then_terminal_failure_stops_after_one_delay() {
        let delay = Duration::from_secs(7);
        let calls = AtomicUsize::new(0);
        let start = Instant::now();

        let error = RetryPolicy::new(2, delay)
            .run(|_| {
                let call = calls.fetch_add(1, Ordering::Relaxed);
                let error = if call == 0 {
                    retryable_error("transient")
                } else {
                    terminal_error("terminal")
                };

                std::future::ready(Err::<(), _>(error))
            })
            .await
            .expect_err("terminal second failure should stop retries");

        assert_eq!(error.message(), "terminal");
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(start.elapsed(), delay);
    }

    #[test]
    fn run_future_is_send() {
        fn assert_send<T: Send>(_: T) {}

        let future = RetryPolicy::new(0, Duration::ZERO)
            .run(|_| std::future::ready(Ok::<(), FlagshipError>(())));

        assert_send(future);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_run_future_cancels_delay_and_later_attempts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let operation_calls = Arc::clone(&calls);
        let task = tokio::spawn(async move {
            RetryPolicy::new(2, Duration::from_secs(60))
                .run(|_| {
                    operation_calls.fetch_add(1, Ordering::Relaxed);

                    std::future::ready(Err::<(), _>(retryable_error("retry")))
                })
                .await
        });

        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        task.abort();
        assert!(
            task.await
                .expect_err("task should be cancelled")
                .is_cancelled()
        );

        advance(Duration::from_secs(180)).await;
        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_run_future_cancels_pending_operation() {
        struct DropFlag(Arc<AtomicBool>);

        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicBool::new(false));
        let operation_calls = Arc::clone(&calls);
        let operation_dropped = Arc::clone(&dropped);
        let task = tokio::spawn(async move {
            RetryPolicy::new(2, Duration::from_secs(60))
                .run(|_| {
                    operation_calls.fetch_add(1, Ordering::Relaxed);
                    let drop_flag = DropFlag(Arc::clone(&operation_dropped));

                    async move {
                        let _drop_flag = drop_flag;

                        std::future::pending::<Result<(), FlagshipError>>().await
                    }
                })
                .await
        });

        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(!dropped.load(Ordering::Relaxed));

        task.abort();
        assert!(
            task.await
                .expect_err("task should be cancelled")
                .is_cancelled()
        );
        assert!(dropped.load(Ordering::Relaxed));

        advance(Duration::from_secs(180)).await;
        yield_now().await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_uses_tokio_time() {
        let delay = Duration::from_secs(3);
        let start = Instant::now();
        let task = tokio::spawn(sleep(delay));

        yield_now().await;
        advance(delay.saturating_sub(Duration::from_nanos(1))).await;
        yield_now().await;
        assert!(!task.is_finished());
        assert_eq!(
            start.elapsed(),
            delay.saturating_sub(Duration::from_nanos(1))
        );

        advance(Duration::from_nanos(1)).await;
        yield_now().await;
        assert_eq!(start.elapsed(), delay);
        assert!(task.is_finished());

        task.await.expect("Tokio delay task should complete");
    }
}
