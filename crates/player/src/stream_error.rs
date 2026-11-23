use std::time::{Duration, Instant};

/// Interval between log lines for one repeated stream error.
pub const STREAM_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Decision from the stream error throttle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamErrorLog {
    /// Log this error now.
    LogNow,
    /// Suppress this repeat of the error.
    Suppress,
    /// Log this error now with the count of repeats since the last log line.
    LogSummary { suppressed: u64 },
}

/// Throttle for repeated audio stream errors.
///
/// One device failure can report the same error on each poll, so it can
/// fill the log. This throttle logs the first error, then suppresses
/// repeats of the same error for one interval.
#[derive(Debug)]
pub struct StreamErrorThrottle {
    last_message: Option<String>,
    last_logged: Option<Instant>,
    suppressed: u64,
}

impl StreamErrorThrottle {
    /// Create a throttle with no error seen.
    pub fn new() -> Self {
        Self {
            last_message: None,
            last_logged: None,
            suppressed: 0,
        }
    }

    /// Record one error and decide what to log for it.
    pub fn record(&mut self, message: &str, now: Instant) -> StreamErrorLog {
        if self.last_message.as_deref() == Some(message) {
            if let Some(last_logged) = self.last_logged
                && now.duration_since(last_logged) < STREAM_ERROR_LOG_INTERVAL
            {
                self.suppressed += 1;
                return StreamErrorLog::Suppress;
            }
            let suppressed = self.suppressed;
            self.suppressed = 0;
            self.last_logged = Some(now);
            return StreamErrorLog::LogSummary { suppressed };
        }
        self.last_message = Some(message.to_string());
        self.last_logged = Some(now);
        self.suppressed = 0;
        StreamErrorLog::LogNow
    }
}

impl Default for StreamErrorThrottle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_error_logs_now() {
        let mut throttle = StreamErrorThrottle::new();
        let now = Instant::now();

        assert_eq!(
            throttle.record("alsa::poll() returned POLLERR", now),
            StreamErrorLog::LogNow
        );
    }

    #[test]
    fn repeats_within_the_interval_are_suppressed() {
        let mut throttle = StreamErrorThrottle::new();
        let now = Instant::now();
        assert_eq!(
            throttle.record("alsa::poll() returned POLLERR", now),
            StreamErrorLog::LogNow
        );

        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(1)
            ),
            StreamErrorLog::Suppress
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(30)
            ),
            StreamErrorLog::Suppress
        );
    }

    #[test]
    fn repeat_after_the_interval_logs_a_summary_with_the_count() {
        let mut throttle = StreamErrorThrottle::new();
        let now = Instant::now();
        assert_eq!(
            throttle.record("alsa::poll() returned POLLERR", now),
            StreamErrorLog::LogNow
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(1)
            ),
            StreamErrorLog::Suppress
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(2)
            ),
            StreamErrorLog::Suppress
        );

        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(61)
            ),
            StreamErrorLog::LogSummary { suppressed: 2 }
        );
    }

    #[test]
    fn a_different_error_logs_now() {
        let mut throttle = StreamErrorThrottle::new();
        let now = Instant::now();
        assert_eq!(
            throttle.record("alsa::poll() returned POLLERR", now),
            StreamErrorLog::LogNow
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(1)
            ),
            StreamErrorLog::Suppress
        );

        assert_eq!(
            throttle.record("device not available", now + Duration::from_secs(2)),
            StreamErrorLog::LogNow
        );
    }

    #[test]
    fn summary_resets_the_suppressed_count() {
        let mut throttle = StreamErrorThrottle::new();
        let now = Instant::now();
        assert_eq!(
            throttle.record("alsa::poll() returned POLLERR", now),
            StreamErrorLog::LogNow
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(1)
            ),
            StreamErrorLog::Suppress
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(61)
            ),
            StreamErrorLog::LogSummary { suppressed: 1 }
        );

        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(62)
            ),
            StreamErrorLog::Suppress
        );
        assert_eq!(
            throttle.record(
                "alsa::poll() returned POLLERR",
                now + Duration::from_secs(122)
            ),
            StreamErrorLog::LogSummary { suppressed: 1 }
        );
    }
}
