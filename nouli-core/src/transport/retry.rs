//! Pure bounded retry decisions. No sleeping, provider calls, or model selection.

use std::time::{Duration, SystemTime};

use super::TransportError;
use crate::config::ExecutionSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAfter {
    Delay(Duration),
    Date(SystemTime),
}

impl RetryAfter {
    pub fn parse(value: &str) -> Option<Self> {
        if let Ok(seconds) = value.parse::<u64>() {
            return Some(Self::Delay(Duration::from_secs(seconds)));
        }
        httpdate::parse_http_date(value).ok().map(Self::Date)
    }
    fn delay(self, now: SystemTime) -> Duration {
        match self {
            Self::Delay(delay) => delay,
            Self::Date(date) => date.duration_since(now).unwrap_or(Duration::ZERO),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    max_attempts: u32,
    max_delay: Duration,
}

impl RetryPolicy {
    /// Builds a retry policy from the attempt and delay limits in `settings`.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Settings`] if the settings are invalid or the
    /// maximum retry delay overflows the clock.
    pub fn new(settings: &ExecutionSettings) -> Result<Self, TransportError> {
        settings.validate().map_err(|_| TransportError::Settings)?;
        let max_delay = Duration::from_secs(settings.max_retry_delay_seconds);
        if std::time::Instant::now().checked_add(max_delay).is_none() {
            return Err(TransportError::Settings);
        }
        Ok(Self {
            max_attempts: settings.max_attempts,
            max_delay,
        })
    }
    /// Completed attempts are one-based. The returned delay permits one next
    /// attempt; attempt zero and exhausted rounds never start extra calls.
    #[must_use]
    pub fn next_delay(
        &self,
        completed_attempts: u32,
        error: &TransportError,
        now: SystemTime,
    ) -> Option<Duration> {
        if completed_attempts == 0 || completed_attempts >= self.max_attempts {
            return None;
        }
        let retry_after = match error {
            TransportError::Connect | TransportError::Timeout => None,
            TransportError::RateLimited { retry_after }
            | TransportError::Status {
                status: 500 | 502 | 503 | 504 | 524 | 529,
                retry_after,
            } => *retry_after,
            _ => return None,
        };
        let seconds = 1_u64
            .checked_shl(completed_attempts - 1)
            .unwrap_or(u64::MAX);
        let backoff = Duration::from_secs(seconds).min(self.max_delay);
        Some(
            retry_after
                .map_or(Duration::ZERO, |hint| hint.delay(now))
                .max(backoff)
                .min(self.max_delay),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_attempt_accounting_and_error_classes() {
        let policy = RetryPolicy::new(&ExecutionSettings::default()).unwrap();
        let now = SystemTime::UNIX_EPOCH;
        assert_eq!(policy.next_delay(0, &TransportError::Timeout, now), None);
        assert_eq!(
            policy.next_delay(1, &TransportError::Timeout, now),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            policy.next_delay(2, &TransportError::Timeout, now),
            Some(Duration::from_secs(2))
        );
        assert_eq!(policy.next_delay(3, &TransportError::Timeout, now), None);
        for error in [
            TransportError::Authentication { status: 401 },
            TransportError::Contract,
            TransportError::Capability,
            TransportError::Capacity,
            TransportError::Canceled,
            TransportError::Status {
                status: 501,
                retry_after: None,
            },
        ] {
            assert_eq!(policy.next_delay(1, &error, now), None);
        }
    }

    #[test]
    fn retry_after_is_clock_injected_capped_and_never_overflows() {
        let policy = RetryPolicy::new(&ExecutionSettings::default()).unwrap();
        let now = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        let after = RetryAfter::parse("Wed, 21 Oct 2015 07:28:10 GMT");
        assert_eq!(
            policy.next_delay(1, &TransportError::RateLimited { retry_after: after }, now),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            policy.next_delay(
                1,
                &TransportError::RateLimited {
                    retry_after: RetryAfter::parse(&u64::MAX.to_string())
                },
                now
            ),
            Some(Duration::from_secs(30))
        );
        assert_eq!(RetryAfter::parse("nonsense"), None);
        let settings = ExecutionSettings {
            max_attempts: u32::MAX,
            ..Default::default()
        };
        assert_eq!(
            RetryPolicy::new(&settings)
                .unwrap()
                .next_delay(100, &TransportError::Connect, now),
            Some(Duration::from_secs(30))
        );
    }
}
