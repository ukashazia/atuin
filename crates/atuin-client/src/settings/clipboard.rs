use eyre::{Result, WrapErr};
use regex::RegexSet;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub enabled: bool,
    pub db_path: String,
    pub poll_interval_ms: u64,
    pub max_bytes: usize,
    pub retention_days: u64,
    pub capture_empty: bool,
    pub secrets_filter: bool,
    pub exclude: Vec<String>,
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        RegexSet::new(&self.exclude).wrap_err("invalid regular expression in clipboard.exclude")?;
        if self.max_bytes == 0 {
            eyre::bail!("clipboard.max_bytes must be greater than zero");
        }
        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        let path = atuin_common::utils::data_dir().join("clipboard.db");
        Self {
            enabled: false,
            db_path: path.to_string_lossy().into_owned(),
            poll_interval_ms: 500,
            max_bytes: 262_144,
            retention_days: 0,
            capture_empty: false,
            secrets_filter: true,
            exclude: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_exclusion_regex() {
        let settings = Settings {
            exclude: vec!["[unterminated".to_owned()],
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }
}
