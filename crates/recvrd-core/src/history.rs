use common_structs::ProviderId;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// A play counts toward history only after this many seconds of
/// the track played. Short skips stay out of the history.
pub const HISTORY_MIN_PLAY_SECS: f64 = 30.0;
/// The history keeps at most this many plays.
pub const HISTORY_MAX_ENTRIES: usize = 1000;
/// A play older than this many seconds leaves the history.
pub const HISTORY_RETENTION_SECS: u64 = 90 * 24 * 60 * 60;

/// Seconds since the Unix epoch, or zero when the clock is unavailable.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// One finished or abandoned play. `played_secs` counts the seconds
/// of the track played, from the start position to the position
/// reached. `completed` is true only when the track ended on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub provider: ProviderId,
    pub track_id: String,
    pub title: String,
    pub author: String,
    pub album: String,
    pub duration: String,
    pub started_at: u64,
    pub played_secs: f64,
    pub completed: bool,
}

/// Listen history, newest first. The file holds one JSON object
/// per line, oldest first, so new plays append without a rewrite.
#[derive(Debug, Default)]
pub struct History {
    entries: VecDeque<HistoryEntry>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries, newest first.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        self.entries.iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load the history file. A missing file loads as empty history.
    /// A malformed line is skipped, so one bad line cannot lose the
    /// whole history.
    pub fn load(path: &Path) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Self::new(),
            Err(error) => {
                log::warn!("could not read listen history {}: {error}", path.display());
                return Self::new();
            }
        };
        let mut history = Self::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<HistoryEntry>(line) {
                Ok(entry) => history.entries.push_back(entry),
                Err(error) => {
                    log::warn!(
                        "skipped malformed listen history line {} in {}: {error}",
                        index + 1,
                        path.display()
                    );
                }
            }
        }
        // The file stores oldest first; memory keeps newest first.
        history.entries = history.entries.into_iter().rev().collect();
        history.trim(now_unix());
        history
    }

    /// Add a play, newest first. Returns true when trimming dropped
    /// entries, so the caller rewrites the file instead of appending.
    pub fn push(&mut self, entry: HistoryEntry, now: u64) -> bool {
        let before = self.entries.len();
        self.entries.push_front(entry);
        self.trim(now);
        self.entries.len() < before + 1
    }

    /// Drop plays older than the retention window, then drop the
    /// oldest plays past the cap.
    fn trim(&mut self, now: u64) {
        let cutoff = now.saturating_sub(HISTORY_RETENTION_SECS);
        self.entries.retain(|entry| entry.started_at >= cutoff);
        self.entries.truncate(HISTORY_MAX_ENTRIES);
    }

    /// Append one entry to the history file.
    pub fn append_entry(path: &Path, entry: &HistoryEntry) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let line = serde_json::to_string(entry)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }

    /// Rewrite the whole history file, oldest first. Use this after
    /// trimming dropped entries that an append cannot remove.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = String::new();
        for entry in self.entries.iter().rev() {
            let line = serde_json::to_string(entry)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            text.push_str(&line);
            text.push('\n');
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, text)?;
        if let Err(error) = fs::rename(&tmp, path) {
            // Windows refuses to replace an existing file with rename.
            if path.exists() {
                fs::remove_file(path)?;
                fs::rename(&tmp, path)?;
            } else {
                return Err(error);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(id: &str, started_at: u64) -> HistoryEntry {
        HistoryEntry {
            provider: ProviderId::Youtube,
            track_id: id.to_string(),
            title: format!("Track {id}"),
            author: "Artist".to_string(),
            album: "Album".to_string(),
            duration: "3:00".to_string(),
            started_at,
            played_secs: 120.0,
            completed: true,
        }
    }

    #[test]
    fn round_trip_keeps_newest_first() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let mut history = History::new();
        let now = now_unix();
        history.push(entry("a", now - 60), now);
        history.push(entry("b", now), now);
        history.save(&path).unwrap();

        let loaded = History::load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.entries()[0].track_id, "b");
        assert_eq!(loaded.entries()[1].track_id, "a");
    }

    #[test]
    fn append_adds_without_rewrite() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let now = now_unix();
        History::append_entry(&path, &entry("a", now - 60)).unwrap();
        History::append_entry(&path, &entry("b", now)).unwrap();

        let loaded = History::load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.entries()[0].track_id, "b");
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempdir().unwrap();
        let loaded = History::load(&dir.path().join("missing.jsonl"));
        assert!(loaded.is_empty());
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history.jsonl");
        let now = now_unix();
        let good = serde_json::to_string(&entry("a", now)).unwrap();
        fs::write(&path, format!("not json\n{good}\n{{bad\n")).unwrap();

        let loaded = History::load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.entries()[0].track_id, "a");
    }

    #[test]
    fn cap_keeps_the_newest_plays() {
        let mut history = History::new();
        let now = now_unix();
        let mut rewrote = false;
        for index in 0..HISTORY_MAX_ENTRIES + 5 {
            rewrote |= history.push(entry(&format!("t{index}"), now), now);
        }
        assert_eq!(history.len(), HISTORY_MAX_ENTRIES);
        assert_eq!(history.entries()[0].track_id, "t1004");
        assert!(rewrote);
    }

    #[test]
    fn retention_drops_old_plays() {
        let mut history = History::new();
        let now = now_unix();
        let old = now - HISTORY_RETENTION_SECS - 60;
        history.push(entry("old", old), now);
        history.push(entry("new", now), now);
        assert_eq!(history.len(), 1);
        assert_eq!(history.entries()[0].track_id, "new");
    }
}
