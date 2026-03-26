//! Retry policy with exponential backoff (BUILD_CORE_ENGINE.md §8.3).
//!
//! ```rust,no_run
//! use orp_core::retry::RetryPolicy;
//!
//! # async fn example() {
//! let policy = RetryPolicy::default();
//! let result = policy.execute(|| async {
//!     // perform some fallible async operation
//!     Ok::<_, orp_core::error::OrpError>("done")
//! }).await;
//! # }
//! ```

use std::time::Duration;
use tokio::time::sleep;
use tracing::{warn, debug};

use crate::error::OrpError;

/// Exponential-backoff retry policy.
///
/// Default: 5 retries, starting at 100 ms, doubling each attempt, capped at 60 s.
///
/// # Example
/// ```rust,no_run
/// # use orp_core::retry::RetryPolicy;
/// # use orp_core::error::OrpError;
/// # async fn connect() -> Result<(), OrpError> { Ok(()) }
/// # tokio_test::block_on(async {
/// let result = RetryPolicy::default().execute(|| connect()).await;
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of attempts **including** the first one.
    ///
    /// 0 means "never execute"; 1 means "execute once, no retries".
    pub max_retries: u32,

    /// Initial backoff duration (before the first retry).
    pub initial_backoff: Duration,

    /// Maximum backoff duration — the backoff is capped at this value.
    pub max_backoff: Duration,

    /// Multiplier applied to the backoff after each failed attempt.
    pub backoff_multiplier: f64,

    /// Optional jitter fraction (0.0–1.0). When > 0, each backoff is randomised
    /// within `[backoff * (1 - jitter), backoff * (1 + jitter)]` to prevent
    /// thundering-herd on concurrent retries.
    pub jitter: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            jitter: 0.0,
        }
    }
}

impl RetryPolicy {
    /// Construct a policy with no retries (execute exactly once).
    pub fn no_retry() -> Self {
        Self {
            max_retries: 1,
            ..Default::default()
        }
    }

    /// Construct a more aggressive policy suitable for fast local operations.
    pub fn fast() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_secs(1),
            backoff_multiplier: 2.0,
            jitter: 0.0,
        }
    }

    /// Execute `f` with this retry policy.
    ///
    /// `f` is called at most `max_retries` times. If every attempt fails, the
    /// error from the **last** attempt is returned.
    ///
    /// # Panics
    /// Does not panic; returns the last error if all attempts fail.
    pub async fn execute<F, T, Fut>(&self, mut f: F) -> Result<T, OrpError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, OrpError>>,
    {
        if self.max_retries == 0 {
            return Err(OrpError::Unknown(
                "RetryPolicy.max_retries is 0; refusing to execute".to_string(),
            ));
        }

        let mut attempt: u32 = 0;
        let mut backoff = self.initial_backoff;

        loop {
            attempt += 1;
            debug!(attempt, "Executing with retry policy");

            match f().await {
                Ok(result) => {
                    if attempt > 1 {
                        debug!(attempt, "Succeeded on retry attempt");
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if attempt >= self.max_retries {
                        warn!(
                            attempt,
                            max_retries = self.max_retries,
                            error = %e,
                            "All retry attempts exhausted"
                        );
                        return Err(e);
                    }

                    // Apply jitter
                    let sleep_dur = if self.jitter > 0.0 {
                        apply_jitter(backoff, self.jitter)
                    } else {
                        backoff
                    };

                    warn!(
                        attempt,
                        max_retries = self.max_retries,
                        sleep_ms = sleep_dur.as_millis(),
                        error = %e,
                        "Attempt failed, retrying"
                    );

                    sleep(sleep_dur).await;

                    // Advance backoff (capped)
                    let next_secs =
                        (backoff.as_secs_f64() * self.backoff_multiplier)
                            .min(self.max_backoff.as_secs_f64());
                    backoff = Duration::from_secs_f64(next_secs);
                }
            }
        }
    }
}

/// Apply ±jitter fraction to a duration.
fn apply_jitter(base: Duration, jitter: f64) -> Duration {
    // Use a simple pseudo-random offset based on the current time nanos.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as f64;
    let factor = 1.0 + jitter * (2.0 * (nanos / 1_000_000_000.0) - 1.0);
    let factor = factor.clamp(1.0 - jitter, 1.0 + jitter);
    Duration::from_secs_f64((base.as_secs_f64() * factor).max(0.0))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_succeeds_immediately() {
        let policy = RetryPolicy::default();
        let result = policy.execute(|| async { Ok::<_, OrpError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retries_then_succeeds() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let policy = RetryPolicy {
            max_retries: 5,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            backoff_multiplier: 2.0,
            jitter: 0.0,
        };

        let result = policy
            .execute(|| {
                let cc = cc.clone();
                async move {
                    let n = cc.fetch_add(1, Ordering::SeqCst);
                    if n < 3 {
                        Err(OrpError::NetworkError("transient".to_string()))
                    } else {
                        Ok("ok")
                    }
                }
            })
            .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(call_count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn test_exhausts_retries() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let policy = RetryPolicy {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(10),
            backoff_multiplier: 2.0,
            jitter: 0.0,
        };

        let result: Result<i32, _> = policy
            .execute(|| {
                let cc = cc.clone();
                async move {
                    cc.fetch_add(1, Ordering::SeqCst);
                    Err(OrpError::NetworkError("always fails".to_string()))
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_zero_max_retries_returns_error() {
        let policy = RetryPolicy {
            max_retries: 0,
            ..RetryPolicy::default()
        };
        let result = policy.execute(|| async { Ok::<_, OrpError>(1) }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_no_retry_policy() {
        let call_count = Arc::new(AtomicU32::new(0));
        let cc = call_count.clone();

        let policy = RetryPolicy::no_retry();
        let _: Result<i32, _> = policy
            .execute(|| {
                let cc = cc.clone();
                async move {
                    cc.fetch_add(1, Ordering::SeqCst);
                    Err(OrpError::ConnectorError("fail".to_string()))
                }
            })
            .await;

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
