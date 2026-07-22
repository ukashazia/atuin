use std::time::Duration;

use atuin_client::secrets::SECRET_PATTERNS_RE;
use eyre::{Result, WrapErr};
use regex::RegexSet;

use crate::{ClipboardBackend, ClipboardEntry, ClipboardError};

pub const MIN_POLL_INTERVAL_MS: u64 = 100;

#[derive(Clone, Debug)]
pub struct CaptureSettings {
    pub enabled: bool,
    pub poll_interval: Duration,
    pub max_bytes: usize,
    pub capture_empty: bool,
    pub secrets_filter: bool,
    exclude: RegexSet,
}

impl CaptureSettings {
    pub fn new(
        enabled: bool,
        poll_interval_ms: u64,
        max_bytes: usize,
        capture_empty: bool,
        secrets_filter: bool,
        exclude: &[String],
    ) -> Result<Self> {
        let exclude =
            RegexSet::new(exclude).wrap_err("invalid regular expression in clipboard.exclude")?;
        Ok(Self {
            enabled,
            poll_interval: Duration::from_millis(poll_interval_ms.max(MIN_POLL_INTERVAL_MS)),
            max_bytes,
            capture_empty,
            secrets_filter,
            exclude,
        })
    }

    fn rejects(&self, content: &str) -> bool {
        (!self.capture_empty && content.is_empty())
            || content.len() > self.max_bytes
            || self.exclude.is_match(content)
            || (self.secrets_filter && SECRET_PATTERNS_RE.is_match(content))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObserveOutcome {
    Disabled,
    Unavailable,
    Filtered,
    Duplicate,
    Captured(ClipboardEntry),
}

pub struct ClipboardWatcher<B> {
    backend: B,
    settings: CaptureSettings,
    hostname: String,
    last_hash: Option<String>,
}

impl<B: ClipboardBackend> ClipboardWatcher<B> {
    pub fn new(backend: B, settings: CaptureSettings, hostname: String) -> Self {
        Self {
            backend,
            settings,
            hostname,
            last_hash: None,
        }
    }

    pub fn settings(&self) -> &CaptureSettings {
        &self.settings
    }

    pub fn update_settings(&mut self, settings: CaptureSettings) {
        self.settings = settings;
    }

    pub fn write_text(&mut self, content: String) -> Result<(), ClipboardError> {
        self.backend.write_text(content)
    }

    pub fn observe(&mut self) -> ObserveOutcome {
        if !self.settings.enabled {
            return ObserveOutcome::Disabled;
        }
        let content = match self.backend.read_text() {
            Ok(content) => content,
            Err(error) => {
                tracing::debug!(%error, "clipboard backend temporarily unavailable");
                return ObserveOutcome::Unavailable;
            }
        };
        if self.settings.rejects(&content) {
            return ObserveOutcome::Filtered;
        }
        let hash = ClipboardEntry::hash(&content);
        if self.last_hash.as_deref() == Some(&hash) {
            return ObserveOutcome::Duplicate;
        }
        self.last_hash = Some(hash);
        ObserveOutcome::Captured(ClipboardEntry::new(content, self.hostname.clone()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct FakeBackend {
        reads: VecDeque<Result<String, ClipboardError>>,
        writes: Vec<String>,
    }

    impl FakeBackend {
        fn new(reads: impl IntoIterator<Item = Result<String, ClipboardError>>) -> Self {
            Self {
                reads: reads.into_iter().collect(),
                writes: Vec::new(),
            }
        }
    }

    impl ClipboardBackend for FakeBackend {
        fn read_text(&mut self) -> Result<String, ClipboardError> {
            self.reads
                .pop_front()
                .unwrap_or_else(|| Err(ClipboardError("no value".to_owned())))
        }

        fn write_text(&mut self, content: String) -> Result<(), ClipboardError> {
            self.writes.push(content);
            Ok(())
        }
    }

    fn settings() -> CaptureSettings {
        CaptureSettings::new(true, 1, 32, false, false, &[]).unwrap()
    }

    #[test]
    fn suppresses_only_consecutive_duplicates() {
        let backend = FakeBackend::new([
            Ok("A".into()),
            Ok("A".into()),
            Ok("B".into()),
            Ok("A".into()),
        ]);
        let mut watcher = ClipboardWatcher::new(backend, settings(), "host".into());
        assert!(matches!(watcher.observe(), ObserveOutcome::Captured(_)));
        assert_eq!(watcher.observe(), ObserveOutcome::Duplicate);
        assert!(matches!(watcher.observe(), ObserveOutcome::Captured(_)));
        assert!(matches!(watcher.observe(), ObserveOutcome::Captured(_)));
    }

    #[test]
    fn filters_empty_size_regex_and_secrets() {
        let excludes = vec!["private".to_owned()];
        let settings = CaptureSettings::new(true, 500, 4, false, true, &excludes).unwrap();
        let backend = FakeBackend::new([
            Ok(String::new()),
            Ok("12345".into()),
            Ok("private".into()),
            Ok("AWS_SECRET_ACCESS_KEY=x".into()),
        ]);
        let mut watcher = ClipboardWatcher::new(backend, settings, "host".into());
        for _ in 0..4 {
            assert_eq!(watcher.observe(), ObserveOutcome::Filtered);
        }
    }

    #[test]
    fn recovers_after_backend_failure_and_does_not_read_when_disabled() {
        let backend =
            FakeBackend::new([Err(ClipboardError("busy".into())), Ok("available".into())]);
        let mut watcher = ClipboardWatcher::new(backend, settings(), "host".into());
        assert_eq!(watcher.observe(), ObserveOutcome::Unavailable);
        assert!(matches!(watcher.observe(), ObserveOutcome::Captured(_)));
        watcher.update_settings(CaptureSettings::new(false, 500, 32, false, false, &[]).unwrap());
        assert_eq!(watcher.observe(), ObserveOutcome::Disabled);
    }
}
