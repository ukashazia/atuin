use std::{fmt, str::FromStr};

use eyre::{Result, bail};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClipboardId(pub String);

impl ClipboardId {
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }
}

impl Default for ClipboardId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ClipboardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for ClipboardId {
    type Err = eyre::Report;

    fn from_str(value: &str) -> Result<Self> {
        let uuid = Uuid::parse_str(value)?;
        if uuid.get_version_num() != 7 {
            bail!("clipboard entry ID must be a UUIDv7")
        }
        Ok(Self(uuid.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardEntry {
    pub id: ClipboardId,
    pub timestamp: OffsetDateTime,
    pub content: String,
    pub content_hash: String,
    pub hostname: String,
    pub mime_type: String,
    pub deleted_at: Option<OffsetDateTime>,
}

impl ClipboardEntry {
    pub fn new(content: String, hostname: String) -> Self {
        let content_hash = Self::hash(&content);
        Self {
            id: ClipboardId::new(),
            timestamp: OffsetDateTime::now_utc(),
            content,
            content_hash,
            hostname,
            mime_type: "text/plain".to_owned(),
            deleted_at: None,
        }
    }

    pub fn hash(content: &str) -> String {
        let digest = Sha256::digest(content.as_bytes());
        format!("{digest:x}")
    }

    pub fn byte_len(&self) -> usize {
        self.content.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_preserves_exact_bytes() {
        assert_ne!(ClipboardEntry::hash("a\n"), ClipboardEntry::hash("a"));
        assert_ne!(ClipboardEntry::hash(" é"), ClipboardEntry::hash("é"));
    }

    #[test]
    fn generated_ids_are_uuid_v7() {
        ClipboardId::new().0.parse::<ClipboardId>().unwrap();
    }
}
