//! Helpers shared by the scrobble workers.

/// A duration longer than this is not sent to a scrobble service.
pub(crate) const MAX_DURATION_SECS: u64 = 24 * 60 * 60;

/// Parse a `m:ss` or `h:mm:ss` duration, in seconds.
pub(crate) fn duration_secs(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split(':').collect();
    let (seconds, minutes, hours) = match parts.as_slice() {
        [seconds] => (seconds.parse::<u64>().ok()?, 0, 0),
        [minutes, seconds] => (
            seconds.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            0,
        ),
        [hours, minutes, seconds] => (
            seconds.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            hours.parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    let total = hours * 3600 + minutes * 60 + seconds;
    (total > 0 && total <= MAX_DURATION_SECS).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_durations() {
        assert_eq!(duration_secs("3:00"), Some(180));
        assert_eq!(duration_secs("1:02:03"), Some(3723));
        assert_eq!(duration_secs("0:00"), None);
        assert_eq!(duration_secs(""), None);
        assert_eq!(duration_secs("bad"), None);
    }
}
