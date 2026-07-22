use std::{
    io::{self, IsTerminal, Write},
    str::FromStr,
};

use atuin_client::{
    database::Database, encryption, record::sqlite_store::SqliteStore, settings::Settings,
    theme::Theme,
};
use atuin_clipboard::{
    ArboardBackend, ClipboardBackend, ClipboardDatabase, ClipboardEntry, ClipboardId,
    ClipboardStore, SearchOptions,
};
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use clap::Subcommand;
use eyre::{Context, Result, bail, eyre};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Search clipboard history
    Search {
        /// Text to find (substring search)
        query: Vec<String>,
        /// Open interactive clipboard search
        #[arg(long, short)]
        interactive: bool,
        #[arg(long)]
        host: Option<String>,
        /// RFC 3339 timestamp
        #[arg(long)]
        before: Option<String>,
        /// RFC 3339 timestamp
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long, short)]
        reverse: bool,
        /// Include soft-deleted entries (diagnostics only)
        #[arg(long)]
        include_deleted: bool,
    },
    /// List clipboard entries newest first
    #[command(alias = "ls")]
    List {
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long, short)]
        reverse: bool,
    },
    /// Print one clipboard entry exactly
    Show { entry_id: String },
    /// Write one stored entry to the system clipboard
    Copy { entry_id: String },
    /// Soft-delete one entry and synchronize the deletion
    #[command(alias = "rm")]
    Delete { entry_id: String },
    /// Soft-delete all active entries
    Clear {
        /// Skip the destructive confirmation prompt
        #[arg(long)]
        force: bool,
    },
    /// Report clipboard capture and storage status without printing content
    Status,
}

impl Cmd {
    #[allow(clippy::too_many_lines)]
    pub async fn run(
        self,
        history_database: &mut impl Database,
        settings: &Settings,
        record_store: &SqliteStore,
        theme: &Theme,
    ) -> Result<()> {
        let database =
            ClipboardDatabase::new(&settings.clipboard.db_path, settings.local_timeout).await?;
        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().await?;
        let history_store = atuin_client::history::store::HistoryStore::new(
            record_store.clone(),
            host_id,
            encryption_key,
        );
        let store = ClipboardStore::new(record_store.clone(), host_id, encryption_key);

        match self {
            Self::Search {
                query,
                interactive,
                host,
                before,
                after,
                limit,
                reverse,
                include_deleted,
            } => {
                let options = SearchOptions {
                    host,
                    before: parse_timestamp(before.as_deref())?,
                    after: parse_timestamp(after.as_deref())?,
                    limit: Some(limit),
                    reverse,
                    include_deleted,
                };
                if interactive {
                    let item = super::search::interactive::history(
                        &query,
                        settings,
                        history_database,
                        &history_store,
                        &database,
                        &store,
                        theme,
                        super::search::interactive::SearchDomain::Clipboard,
                        options,
                    )
                    .await?;
                    if !io::stdout().is_terminal() {
                        println!("{item}");
                    } else if io::stderr().is_terminal() {
                        eprintln!("{}", item.escape_non_printable());
                    } else {
                        eprintln!("{item}");
                    }
                    Ok(())
                } else {
                    let query = query.join(" ");
                    print_entries(&database.search(&query, &options).await?)
                }
            }
            Self::List { limit, reverse } => print_entries(
                &database
                    .list(&SearchOptions {
                        limit: Some(limit),
                        reverse,
                        ..SearchOptions::default()
                    })
                    .await?,
            ),
            Self::Show { entry_id } => {
                let entry = load_entry(&database, &entry_id).await?;
                print!("{}", entry.content);
                io::stdout().flush()?;
                Ok(())
            }
            Self::Copy { entry_id } => {
                let entry = load_entry(&database, &entry_id).await?;
                let mut backend = ArboardBackend::new()?;
                restore_entry(&mut backend, &entry)
            }
            Self::Delete { entry_id } => {
                let entry = load_entry(&database, &entry_id).await?;
                delete_entry(&database, &store, &entry).await
            }
            Self::Clear { force } => {
                if !confirm_clear(force)? {
                    println!("Clipboard history was not cleared.");
                    return Ok(());
                }
                let entries = database
                    .list(&SearchOptions {
                        limit: Some(u32::MAX),
                        ..SearchOptions::default()
                    })
                    .await?;
                let count = entries.len();
                for (index, entry) in entries.into_iter().enumerate() {
                    delete_entry(&database, &store, &entry).await?;
                    if index > 0 && index % 250 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
                println!("Soft-deleted {count} clipboard entries.");
                Ok(())
            }
            Self::Status => print_status(settings, &database).await,
        }
    }
}

fn parse_timestamp(value: Option<&str>) -> Result<Option<OffsetDateTime>> {
    value
        .map(|value| {
            OffsetDateTime::parse(value, &Rfc3339)
                .wrap_err_with(|| format!("invalid RFC 3339 timestamp: {value}"))
        })
        .transpose()
}

async fn load_entry(database: &ClipboardDatabase, id: &str) -> Result<ClipboardEntry> {
    let id = ClipboardId::from_str(id).wrap_err("invalid clipboard entry ID")?;
    database
        .load(&id)
        .await?
        .filter(|entry| entry.deleted_at.is_none())
        .ok_or_else(|| eyre!("clipboard entry {id} was not found"))
}

fn one_line(content: &str) -> String {
    content
        .replace('\r', "\\r")
        .replace('\n', " ↵ ")
        .replace('\t', " ⇥ ")
}

fn print_entries(entries: &[ClipboardEntry]) -> Result<()> {
    for entry in entries {
        let timestamp = entry.timestamp.format(&Rfc3339)?;
        println!(
            "{}\t{}\t{}\t{}",
            entry.id,
            timestamp,
            entry.hostname,
            one_line(&entry.content)
        );
    }
    Ok(())
}

pub(super) fn restore_entry(
    backend: &mut impl ClipboardBackend,
    entry: &ClipboardEntry,
) -> Result<()> {
    backend
        .write_text(entry.content.clone())
        .wrap_err("failed to write to the system clipboard")
}

pub(super) async fn delete_entry(
    database: &ClipboardDatabase,
    store: &ClipboardStore,
    entry: &ClipboardEntry,
) -> Result<()> {
    store.delete(entry.id.clone()).await?;
    database
        .soft_delete(&entry.id, OffsetDateTime::now_utc())
        .await?;
    Ok(())
}

fn confirm_clear(force: bool) -> Result<bool> {
    if force {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        bail!("clipboard clear requires an interactive confirmation; pass --force to continue");
    }
    eprint!("Soft-delete all active clipboard history entries? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

async fn print_status(settings: &Settings, database: &ClipboardDatabase) -> Result<()> {
    let backend_available = ArboardBackend::new().is_ok();
    #[cfg(feature = "daemon")]
    let daemon_reachable = atuin_daemon::ControlClient::from_settings(settings)
        .await
        .is_ok();
    #[cfg(not(feature = "daemon"))]
    let daemon_reachable = false;

    println!("capture enabled: {}", settings.clipboard.enabled);
    println!("daemon reachable: {daemon_reachable}");
    println!("database path: {}", settings.clipboard.db_path);
    println!("active entries: {}", database.count_active().await?);
    if settings.clipboard.retention_days == 0 {
        println!("retention: indefinite");
    } else {
        println!("retention: {} days", settings.clipboard.retention_days);
    }
    println!("backend available: {backend_available}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: Cmd,
    }

    #[derive(Default)]
    struct FakeBackend(String);

    impl ClipboardBackend for FakeBackend {
        fn read_text(&mut self) -> std::result::Result<String, atuin_clipboard::ClipboardError> {
            Ok(self.0.clone())
        }

        fn write_text(
            &mut self,
            content: String,
        ) -> std::result::Result<(), atuin_clipboard::ClipboardError> {
            self.0 = content;
            Ok(())
        }
    }

    #[test]
    fn restores_multiline_content_exactly() {
        let entry = ClipboardEntry::new("line one\n世界\n".to_owned(), "host".to_owned());
        let mut backend = FakeBackend::default();
        restore_entry(&mut backend, &entry).unwrap();
        assert_eq!(backend.0, entry.content);
    }

    #[test]
    fn preview_is_single_line() {
        assert_eq!(one_line("a\nb\tc"), "a ↵ b ⇥ c");
    }

    #[test]
    fn command_parsing_and_force_confirmation() {
        let parsed = TestCli::try_parse_from(["test", "search", "needle", "-i"]).unwrap();
        assert!(matches!(
            parsed.command,
            Cmd::Search {
                interactive: true,
                ..
            }
        ));
        assert!(confirm_clear(true).unwrap());
    }

    #[tokio::test]
    async fn invalid_and_missing_ids_are_errors() {
        let database = ClipboardDatabase::new("sqlite::memory:", 1.0)
            .await
            .unwrap();
        assert!(load_entry(&database, "not-an-id").await.is_err());
        assert!(load_entry(&database, &ClipboardId::new().0).await.is_err());
    }
}
