use std::time::Duration;

use atuin_client::history::History;
use atuin_clipboard::ClipboardEntry;
use time::OffsetDateTime;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SearchItem {
    History(History),
    Clipboard(ClipboardEntry),
}

impl SearchItem {
    pub(super) fn content(&self) -> &str {
        match self {
            Self::History(history) => &history.command,
            Self::Clipboard(entry) => &entry.content,
        }
    }

    pub(super) fn timestamp(&self) -> OffsetDateTime {
        match self {
            Self::History(history) => history.timestamp,
            Self::Clipboard(entry) => entry.timestamp,
        }
    }

    pub(super) fn host(&self) -> &str {
        match self {
            Self::History(history) => history
                .hostname
                .split(':')
                .next()
                .unwrap_or(&history.hostname),
            Self::Clipboard(entry) => &entry.hostname,
        }
    }

    pub(super) fn user(&self) -> Option<&str> {
        match self {
            Self::History(history) => Some(history.hostname.split(':').nth(1).unwrap_or("")),
            Self::Clipboard(_) => None,
        }
    }

    pub(super) fn duration(&self) -> Option<Duration> {
        match self {
            Self::History(history) => Some(Duration::from_nanos(
                u64::try_from(history.duration).unwrap_or(0),
            )),
            Self::Clipboard(_) => None,
        }
    }

    pub(super) fn directory(&self) -> Option<&str> {
        match self {
            Self::History(history) => Some(&history.cwd),
            Self::Clipboard(_) => None,
        }
    }

    pub(super) fn exit(&self) -> Option<i64> {
        match self {
            Self::History(history) => Some(history.exit),
            Self::Clipboard(_) => None,
        }
    }

    pub(super) fn success(&self) -> Option<bool> {
        match self {
            Self::History(history) => Some(history.success()),
            Self::Clipboard(_) => None,
        }
    }

    pub(super) fn as_history(&self) -> Option<&History> {
        match self {
            Self::History(history) => Some(history),
            Self::Clipboard(_) => None,
        }
    }

    pub(super) fn as_clipboard(&self) -> Option<&ClipboardEntry> {
        match self {
            Self::Clipboard(entry) => Some(entry),
            Self::History(_) => None,
        }
    }

    pub(super) fn into_history(self) -> Option<History> {
        match self {
            Self::History(history) => Some(history),
            Self::Clipboard(_) => None,
        }
    }
}

impl From<History> for SearchItem {
    fn from(value: History) -> Self {
        Self::History(value)
    }
}

impl From<ClipboardEntry> for SearchItem {
    fn from(value: ClipboardEntry) -> Self {
        Self::Clipboard(value)
    }
}
