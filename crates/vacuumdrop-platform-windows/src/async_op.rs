//! Awaiting WinRT async operations.
//!
//! # Spike finding (ADR-0009 Q2)
//!
//! The Phase 3 spike initially concluded that `windows` 0.62.2 shipped no
//! `IntoFuture` and that a hand-written bridge was needed. **That conclusion was
//! wrong**, and the correction is worth recording because it nearly cost us a
//! maintenance liability.
//!
//! The mistake: the search was run against `windows-core`, the wrong crate.
//! `IntoFuture` for `IAsyncOperation<T>` is implemented in **`windows-future`
//! 0.3.2**, which `windows` depends on but re-exports selectively.
//!
//! # What the real implementation does
//!
//! `windows_future::AsyncFuture` is a genuine `std::future::Future`:
//!
//! 1. Checks `Status()`. Anything other than `Started` is finished, so results
//!    are read immediately.
//! 2. Otherwise registers a `Completed` handler holding a `Mutex<Waker>`, so a
//!    wakeup on an OS thread wakes the task that is *currently* awaiting it.
//! 3. Re-checks status after registering, closing the race where completion
//!    lands between the check and the handler install.
//!
//! That is callback-driven with a proper waker handshake - strictly better than
//! polling, and already tested upstream.
//!
//! # What we rejected
//!
//! | Option | Verdict |
//! | --- | --- |
//! | `join()` | **Blocking.** Deadlocks a Tokio worker. Never use inside the runtime. |
//! | `AsyncFuture` + `tokio::time::timeout` | **Adopted.** Non-blocking, deadline-enforced. |
//! | Hand-written `oneshot` bridge | **Written, then deleted.** Duplicated upstream logic. |
//! | Polling loop on `Status()` | Rejected: burns CPU on an operation measured in seconds. |
//!
//! # The one real hazard: `join()`
//!
//! `Async::join()` reads like the obvious choice and will silently wedge a Tokio
//! worker. [`await_op`] exists to make the safe form the only one this crate
//! uses, so nobody reaches for `join()` by accident.

use std::time::Duration;

/// Await a WinRT async operation with a deadline.
///
/// Charter 65 forbids an operation that can wait forever, so there is no
/// deadline-free variant: the parameter is required rather than defaulted.
///
/// # Panics
///
/// Never. A timeout produces [`AwaitError::Timeout`], and an OS failure produces
/// [`AwaitError::Failed`].
pub async fn await_op<T, E>(
    future: impl std::future::Future<Output = Result<T, E>>,
    operation: &'static str,
    limit: Duration,
) -> Result<T, AwaitError>
where
    E: std::fmt::Debug,
{
    match tokio::time::timeout(limit, future).await {
        Ok(Ok(value)) => Ok(value),
        // `E` is `windows::core::Error` at the call site. It renders with the
        // HRESULT and the OS message, so both survive into the log line.
        Ok(Err(e)) => Err(AwaitError::Failed {
            operation,
            message: format!("{e:?}"),
        }),
        Err(_) => Err(AwaitError::Timeout { operation, limit }),
    }
}

/// Errors from awaiting an async operation.
#[derive(Debug, thiserror::Error)]
pub enum AwaitError {
    /// The operation exceeded its deadline.
    #[error("operation `{operation}` exceeded {limit:?}")]
    Timeout {
        /// Human-readable operation name, for logs.
        operation: &'static str,
        /// The deadline that elapsed.
        limit: Duration,
    },

    /// The operation itself failed.
    #[error("operation `{operation}` failed: {message}")]
    Failed {
        /// Human-readable operation name, for logs.
        operation: &'static str,
        /// The underlying error, rendered.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_ready_value_passes_through() {
        let r = await_op(
            async { Ok::<u32, &str>(42) },
            "instant",
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(r.unwrap(), 42);
    }

    #[tokio::test]
    async fn a_slow_operation_times_out() {
        // Charter 65: an operation must not be able to wait forever.
        let r = await_op(
            async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok::<u8, &str>(1)
            },
            "slow",
            Duration::from_millis(80),
        )
        .await;
        match r {
            Err(AwaitError::Timeout { operation, limit }) => {
                assert_eq!(operation, "slow");
                assert_eq!(limit, Duration::from_millis(80));
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_operation_reports_its_own_error() {
        let r = await_op(
            async { Err::<u8, &str>("boom") },
            "failing",
            Duration::from_secs(1),
        )
        .await;
        match r {
            Err(AwaitError::Failed { operation, message }) => {
                assert_eq!(operation, "failing");
                assert!(message.contains("boom"), "message lost: {message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_message_names_the_operation() {
        // "it timed out" is unactionable when several operations can stall.
        let r = await_op(
            async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<(), &str>(())
            },
            "WiFiDirectDevice::FromIdAsync2",
            Duration::from_millis(50),
        )
        .await;
        let s = r.unwrap_err().to_string();
        assert!(s.contains("FromIdAsync2"));
    }

    #[tokio::test]
    async fn timeouts_do_not_leak_the_task() {
        // After a timeout the underlying WinRT operation is NOT cancelled - it
        // continues in the OS. Nothing we can do about that; what we can verify is
        // that we released the awaiting task, which is what Charter 64 requires.
        let handle = tokio::spawn(async {
            let _ = await_op(
                async {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok::<(), &str>(())
                },
                "leaky",
                Duration::from_millis(50),
            )
            .await;
        });
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("the awaiting task must be released")
            .expect("the spawned task must not panic");
    }
}
