use atuin_client::{
    database::{QueryToken, QueryTokenizer},
    settings::{FilterMode, SearchMode},
};
use atuin_clipboard::{ClipboardDatabase, ClipboardEntry, SearchOptions};
use eyre::Result;
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use norm::{
    Metric,
    fzf::{FzfParser, FzfV2},
};

const INTERACTIVE_LIMIT: usize = 200;

pub(super) struct ClipboardSearch {
    entries: Vec<ClipboardEntry>,
    loaded: bool,
    options: SearchOptions,
}

impl ClipboardSearch {
    pub(super) fn new(mut options: SearchOptions) -> Self {
        options.host = None;
        options.limit = None;
        Self {
            entries: Vec::new(),
            loaded: false,
            options,
        }
    }

    pub(super) async fn refresh(&mut self, database: &ClipboardDatabase) -> Result<()> {
        self.entries = database.list(&self.options).await?;
        self.loaded = true;
        Ok(())
    }

    pub(super) async fn query(
        &mut self,
        database: &ClipboardDatabase,
        query: &str,
        mode: SearchMode,
        filter: FilterMode,
        hostname: &str,
    ) -> Result<Vec<ClipboardEntry>> {
        if !self.loaded {
            self.refresh(database).await?;
        }

        let entries = self.entries.iter().filter(|entry| {
            filter != FilterMode::Host || entry.hostname.eq_ignore_ascii_case(hostname)
        });

        if query.is_empty() {
            return Ok(entries.take(INTERACTIVE_LIMIT).cloned().collect());
        }

        match mode {
            SearchMode::Prefix => Ok(entries
                .filter(|entry| prefix_matches(&entry.content, query))
                .take(INTERACTIVE_LIMIT)
                .cloned()
                .collect()),
            SearchMode::FullText => Ok(entries
                .filter(|entry| fulltext_matches(&entry.content, query))
                .take(INTERACTIVE_LIMIT)
                .cloned()
                .collect()),
            SearchMode::Fuzzy | SearchMode::DaemonFuzzy => {
                let mut matcher = FzfV2::new();
                let mut parser = FzfParser::new();
                let parsed = parser.parse(query);
                let mut ranked = entries
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        matcher
                            .distance(parsed, &entry.content)
                            .map(|distance| (distance, index, entry.clone()))
                    })
                    .collect::<Vec<_>>();
                ranked
                    .sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
                Ok(ranked
                    .into_iter()
                    .take(INTERACTIVE_LIMIT)
                    .map(|(_, _, entry)| entry)
                    .collect())
            }
            SearchMode::Skim => {
                let matcher = SkimMatcherV2::default();
                let mut ranked = entries
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        matcher
                            .fuzzy_match(&entry.content, query)
                            .map(|score| (score, index, entry.clone()))
                    })
                    .collect::<Vec<_>>();
                ranked
                    .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
                Ok(ranked
                    .into_iter()
                    .take(INTERACTIVE_LIMIT)
                    .map(|(_, _, entry)| entry)
                    .collect())
            }
        }
    }

    pub(super) fn remove_id(&mut self, id: &atuin_clipboard::ClipboardId) {
        self.entries.retain(|entry| entry.id != *id);
    }

    pub(super) fn remove_content(&mut self, content: &str) {
        self.entries.retain(|entry| entry.content != content);
    }
}

pub(super) fn highlight_indices(mode: SearchMode, content: &str, query: &str) -> Vec<usize> {
    match mode {
        SearchMode::Prefix => Vec::new(),
        SearchMode::FullText => super::engines::db::get_highlight_indices_fulltext(content, query),
        SearchMode::Fuzzy | SearchMode::DaemonFuzzy => {
            let mut matcher = FzfV2::new();
            let mut parser = FzfParser::new();
            let parsed = parser.parse(query);
            let mut ranges = Vec::new();
            let _ = matcher.distance_and_ranges(parsed, content, &mut ranges);
            ranges.into_iter().flatten().collect()
        }
        SearchMode::Skim => SkimMatcherV2::default()
            .fuzzy_indices(content, query)
            .map_or_else(Vec::new, |(_, indices)| indices),
    }
}

fn prefix_matches(content: &str, query: &str) -> bool {
    if query.contains(char::is_uppercase) {
        content.starts_with(query)
    } else {
        content
            .to_ascii_lowercase()
            .starts_with(&query.to_ascii_lowercase())
    }
}

fn fulltext_matches(content: &str, query: &str) -> bool {
    let mut groups = vec![Vec::new()];
    for token in QueryTokenizer::new(query) {
        if matches!(token, QueryToken::Or) {
            groups.push(Vec::new());
        } else {
            groups
                .last_mut()
                .expect("at least one query group")
                .push(token);
        }
    }

    groups.into_iter().any(|tokens| {
        !tokens.is_empty()
            && tokens
                .into_iter()
                .all(|token| fulltext_token_matches(content, &token))
    })
}

fn fulltext_token_matches(content: &str, token: &QueryToken<'_>) -> bool {
    if let QueryToken::Regex(pattern) = token {
        return regex::Regex::new(pattern).is_ok_and(|regex| regex.is_match(content));
    }

    let inverse = token.is_inverse();
    let case_sensitive = token.has_uppercase();
    let lower_content;
    let candidate = if case_sensitive {
        content
    } else {
        lower_content = content.to_ascii_lowercase();
        &lower_content
    };
    let is_match = match token {
        QueryToken::Match(term, _) | QueryToken::MatchFull(term, _) => {
            let lower_term;
            let term: &str = if case_sensitive {
                term
            } else {
                lower_term = term.to_ascii_lowercase();
                lower_term.as_str()
            };
            candidate.contains(term)
        }
        QueryToken::MatchStart(term, _) => {
            let lower_term;
            let term: &str = if case_sensitive {
                term
            } else {
                lower_term = term.to_ascii_lowercase();
                lower_term.as_str()
            };
            candidate.starts_with(term)
        }
        QueryToken::MatchEnd(term, _) => {
            let lower_term;
            let term: &str = if case_sensitive {
                term
            } else {
                lower_term = term.to_ascii_lowercase();
                lower_term.as_str()
            };
            candidate.ends_with(term)
        }
        QueryToken::Or | QueryToken::Regex(_) => unreachable!(),
    };
    is_match != inverse
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn entry(content: &str, hostname: &str, timestamp: i64) -> ClipboardEntry {
        let mut entry = ClipboardEntry::new(content.to_owned(), hostname.to_owned());
        entry.timestamp = OffsetDateTime::from_unix_timestamp(timestamp).unwrap();
        entry
    }

    #[test]
    fn fulltext_supports_tokens_inverse_regex_and_or() {
        assert!(fulltext_matches("hello clipboard world", "hello world"));
        assert!(!fulltext_matches("hello clipboard", "hello !clipboard"));
        assert!(fulltext_matches("hello clipboard", "missing | clipboard"));
        assert!(fulltext_matches("hello clipboard", "r/^hello "));
    }

    #[test]
    fn daemon_fuzzy_uses_local_highlighting() {
        assert_eq!(
            highlight_indices(SearchMode::DaemonFuzzy, "clipboard", "cbd"),
            vec![0, 4, 8]
        );
    }

    #[tokio::test]
    async fn every_mode_filters_clipboard_content_and_preserves_duplicates() {
        let database = ClipboardDatabase::new("sqlite::memory:", 1.0)
            .await
            .unwrap();
        database
            .insert_batch(&[
                entry("alpha one", "host-a", 1),
                entry("beta two", "host-b", 2),
                entry("alpha one", "host-a", 3),
            ])
            .await
            .unwrap();
        let mut search = ClipboardSearch::new(SearchOptions::default());

        let newest = search
            .query(&database, "", SearchMode::FullText, FilterMode::Global, "")
            .await
            .unwrap();
        assert_eq!(
            newest
                .iter()
                .map(|entry| entry.content.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha one", "beta two", "alpha one"]
        );

        for mode in [SearchMode::Prefix, SearchMode::FullText] {
            let results = search
                .query(&database, "alpha", mode, FilterMode::Global, "")
                .await
                .unwrap();
            assert_eq!(results.len(), 2, "mode {mode:?}");
        }
        for mode in [SearchMode::Fuzzy, SearchMode::Skim, SearchMode::DaemonFuzzy] {
            let results = search
                .query(&database, "bt", mode, FilterMode::Global, "")
                .await
                .unwrap();
            assert_eq!(results[0].content, "beta two", "mode {mode:?}");
        }

        let host = search
            .query(
                &database,
                "",
                SearchMode::FullText,
                FilterMode::Host,
                "host-a",
            )
            .await
            .unwrap();
        assert_eq!(host.len(), 2);
        assert!(host.iter().all(|entry| entry.hostname == "host-a"));
    }
}
