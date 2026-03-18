use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use serde::{Deserialize, Serialize};
use std::{
    io::{Error, ErrorKind},
    time::Duration,
};

/// Endpoint-level resilience policy for retries, backoff, and rate-limit handling.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct EndpointRetryPolicy {
    /// Maximum total attempts (initial request + retries).
    pub max_attempts: usize,
    /// Base backoff delay in milliseconds for retry calculation.
    pub base_backoff_ms: u64,
    /// Maximum exponential backoff delay in milliseconds.
    pub max_backoff_ms: u64,
    /// Maximum delay applied from any retry rule in milliseconds.
    pub max_retry_delay_ms: u64,
    /// Fallback delay in milliseconds for rate-limited responses when `Retry-After` is absent.
    pub rate_limit_backoff_ms: u64,
    /// Whether transport I/O failures should be retried.
    pub retry_on_io_errors: bool,
    /// Whether timeout failures should be retried.
    pub retry_on_timeout: bool,
    /// Whether to honor `Retry-After` on status `429`.
    pub respect_retry_after: bool,
    /// HTTP statuses eligible for retries.
    pub retry_on_status: Vec<u16>,
}

impl Default for EndpointRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            base_backoff_ms: 100,
            max_backoff_ms: 5_000,
            max_retry_delay_ms: 30_000,
            rate_limit_backoff_ms: 1_000,
            retry_on_io_errors: true,
            retry_on_timeout: true,
            respect_retry_after: true,
            retry_on_status: vec![429, 500, 502, 503, 504],
        }
    }
}

impl EndpointRetryPolicy {
    pub(super) fn validate(&self) -> std::io::Result<()> {
        if self.max_attempts == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "retry_policy.max_attempts must be > 0",
            ));
        }
        if self.max_backoff_ms < self.base_backoff_ms {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "retry_policy.max_backoff_ms must be >= base_backoff_ms",
            ));
        }
        if self.max_retry_delay_ms == 0 {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "retry_policy.max_retry_delay_ms must be > 0",
            ));
        }
        for status in &self.retry_on_status {
            if !(100..=599).contains(status) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("retry_policy.retry_on_status contains invalid status code `{status}`"),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn should_retry_status(&self, status: u16) -> bool {
        self.retry_on_status.contains(&status)
    }

    pub(super) fn should_retry_transport_error(&self, err: &std::io::Error) -> bool {
        if err.kind() == ErrorKind::TimedOut {
            return self.retry_on_timeout;
        }
        self.retry_on_io_errors
    }

    pub(super) fn retry_delay_for_transport(&self, attempt: usize) -> Duration {
        Duration::from_millis(self.backoff_delay_ms(attempt))
    }

    pub(super) fn retry_delay_for_status(
        &self,
        status: u16,
        headers: &HeaderMap,
        attempt: usize,
    ) -> Duration {
        let delay_ms = if status == 429 {
            self.rate_limit_delay_ms(headers, attempt)
        } else {
            self.backoff_delay_ms(attempt)
        };
        Duration::from_millis(delay_ms)
    }

    fn rate_limit_delay_ms(&self, headers: &HeaderMap, attempt: usize) -> u64 {
        let retry_after_ms = if self.respect_retry_after {
            headers
                .get(RETRY_AFTER)
                .and_then(Self::parse_retry_after_ms)
                .map(|v| v.min(self.max_retry_delay_ms))
        } else {
            None
        };
        retry_after_ms.unwrap_or_else(|| {
            self.rate_limit_backoff_ms
                .max(self.backoff_delay_ms(attempt))
                .min(self.max_retry_delay_ms)
        })
    }

    fn parse_retry_after_ms(value: &HeaderValue) -> Option<u64> {
        let seconds = value.to_str().ok()?.trim().parse::<u64>().ok()?;
        Some(seconds.saturating_mul(1_000))
    }

    fn backoff_delay_ms(&self, attempt: usize) -> u64 {
        let exp = (attempt.saturating_sub(1)).min(20);
        let exp_u32 = u32::try_from(exp).unwrap_or(20);
        let factor = 2u64.saturating_pow(exp_u32);
        self.base_backoff_ms
            .saturating_mul(factor)
            .min(self.max_backoff_ms)
            .min(self.max_retry_delay_ms)
    }
}
