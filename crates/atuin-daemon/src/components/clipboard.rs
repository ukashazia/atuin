//! Daemon-managed clipboard capture.

use std::time::Duration;

use atuin_client::settings::{Settings, clipboard};
use atuin_clipboard::{
    ArboardBackend, CaptureSettings, ClipboardStore, ClipboardWatcher, ObserveOutcome,
};
use eyre::Result;
use time::OffsetDateTime;
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};

use crate::{
    daemon::{Component, DaemonHandle},
    events::DaemonEvent,
};

enum ClipboardCommand {
    Update(clipboard::Settings),
    Stop,
}

pub struct ClipboardComponent {
    task: Option<JoinHandle<()>>,
    command_tx: Option<mpsc::Sender<ClipboardCommand>>,
    handle: Option<DaemonHandle>,
}

impl ClipboardComponent {
    pub const fn new() -> Self {
        Self {
            task: None,
            command_tx: None,
            handle: None,
        }
    }
}

impl Default for ClipboardComponent {
    fn default() -> Self {
        Self::new()
    }
}

fn capture_settings(settings: &clipboard::Settings) -> Result<CaptureSettings> {
    CaptureSettings::new(
        settings.enabled,
        settings.poll_interval_ms,
        settings.max_bytes,
        settings.capture_empty,
        settings.secrets_filter,
        &settings.exclude,
    )
}

#[tonic::async_trait]
impl Component for ClipboardComponent {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    async fn start(&mut self, handle: DaemonHandle) -> Result<()> {
        let settings = handle.settings().await.clone();
        let host_id = Settings::host_id().await?;
        let store = ClipboardStore::new(handle.store().clone(), host_id, *handle.encryption_key());
        let (command_tx, command_rx) = mpsc::channel(8);
        self.command_tx = Some(command_tx);
        self.handle = Some(handle.clone());
        self.task = Some(tokio::spawn(capture_loop(
            handle,
            store,
            settings.clipboard,
            command_rx,
        )));
        Ok(())
    }

    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
        if matches!(event, DaemonEvent::SettingsReloaded)
            && let Some(command_tx) = &self.command_tx
            && let Some(handle) = &self.handle
        {
            let settings = handle.settings().await.clipboard.clone();
            let _ = command_tx.send(ClipboardCommand::Update(settings)).await;
        }
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(command_tx) = &self.command_tx {
            let _ = command_tx.send(ClipboardCommand::Stop).await;
        }
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
        {
            tracing::warn!("clipboard watcher did not stop in time; aborting it");
            task.abort();
        }
        self.command_tx = None;
        self.handle = None;
        Ok(())
    }
}

async fn capture_loop(
    handle: DaemonHandle,
    store: ClipboardStore,
    mut configured: clipboard::Settings,
    mut commands: mpsc::Receiver<ClipboardCommand>,
) {
    let hostname = whoami::hostname().unwrap_or_else(|_| "unknown".to_owned());
    let mut watcher: Option<ClipboardWatcher<ArboardBackend>> = None;
    let mut next_retention = Instant::now();

    loop {
        let settings = match capture_settings(&configured) {
            Ok(settings) => settings,
            Err(error) => {
                tracing::error!(%error, "invalid clipboard settings; capture paused");
                match commands.recv().await {
                    Some(ClipboardCommand::Update(settings)) => configured = settings,
                    Some(ClipboardCommand::Stop) | None => break,
                }
                continue;
            }
        };

        if !settings.enabled {
            watcher = None;
            match commands.recv().await {
                Some(ClipboardCommand::Update(settings)) => configured = settings,
                Some(ClipboardCommand::Stop) | None => break,
            }
            continue;
        }

        if watcher.is_none() {
            match ArboardBackend::new() {
                Ok(backend) => {
                    watcher = Some(ClipboardWatcher::new(
                        backend,
                        settings.clone(),
                        hostname.clone(),
                    ));
                    tracing::info!("clipboard capture enabled");
                }
                Err(error) => {
                    tracing::debug!(%error, "clipboard backend unavailable; retrying");
                }
            }
        } else if let Some(watcher) = &mut watcher {
            watcher.update_settings(settings.clone());
        }

        if Instant::now() >= next_retention {
            if let Err(error) = run_retention(&handle, &store, configured.retention_days).await {
                tracing::warn!(%error, "clipboard retention maintenance failed");
            }
            next_retention = Instant::now() + Duration::from_secs(60 * 60);
        }

        tokio::select! {
            command = commands.recv() => match command {
                Some(ClipboardCommand::Update(settings)) => configured = settings,
                Some(ClipboardCommand::Stop) | None => break,
            },
            () = tokio::time::sleep(settings.poll_interval) => {
                let Some(watcher) = &mut watcher else {
                    continue;
                };
                let database = match handle.ensure_clipboard_db().await {
                    Ok(database) => database,
                    Err(error) => {
                        tracing::debug!(%error, "clipboard database unavailable; retrying");
                        continue;
                    }
                };
                if let ObserveOutcome::Captured(entry) = watcher.observe() {
                    let entry_id = entry.id.clone();
                    let byte_len = entry.byte_len();
                    if let Err(error) = database.insert(&entry).await {
                        tracing::warn!(entry_id = %entry_id, %error, "failed to persist clipboard entry");
                        continue;
                    }
                    if let Err(error) = store.push(entry).await {
                        tracing::warn!(entry_id = %entry_id, %error, "failed to append clipboard sync record");
                        continue;
                    }
                    tracing::debug!(entry_id = %entry_id, byte_len, "captured clipboard entry");
                    handle.emit(DaemonEvent::ClipboardCaptured(entry_id));
                }
            }
        }
    }
    tracing::info!("clipboard capture stopped");
}

async fn run_retention(
    handle: &DaemonHandle,
    store: &ClipboardStore,
    retention_days: u64,
) -> Result<()> {
    if retention_days == 0 {
        return Ok(());
    }
    let days: i64 = retention_days.try_into()?;
    let cutoff = OffsetDateTime::now_utc() - time::Duration::days(days);
    let database = handle.ensure_clipboard_db().await?;
    loop {
        let entries = database.entries_older_than(cutoff, 250).await?;
        let count = entries.len();
        for entry in entries {
            store.delete(entry.id.clone()).await?;
            database
                .soft_delete(&entry.id, OffsetDateTime::now_utc())
                .await?;
            handle.emit(DaemonEvent::ClipboardDeleted(entry.id));
        }
        if count < 250 {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(())
}
