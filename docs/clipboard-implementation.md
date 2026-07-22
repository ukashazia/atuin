# Clipboard history implementation

## Repository map

- Daemon boot is `atuin_daemon::boot`. It constructs `HistoryComponent`,
  `SearchComponent`, `SemanticComponent`, and `SyncComponent`, registers their
  gRPC services, and drives them through the `Component` lifecycle. Settings
  reloads are broadcast as `DaemonEvent::SettingsReloaded`.
- `DaemonState` owns the history SQLite database, the generic encrypted
  `SqliteStore`, settings, and the encryption key. Clipboard uses its own
  materialized database and is held by its component rather than being added to
  history state.
- Record transport is tag-agnostic. `record::sync::sync` returns downloaded
  record IDs. `SyncComponent` passes those IDs to `HistoryStore` and rebuilds
  aliases/variables after transport. Clipboard materialization can consume the
  same IDs without changing the server or network protocol.
- `HistoryStore` is the closest record convention: per-host/tag indexes,
  PASETO V4 encryption, versioned MessagePack payloads, and tolerant handling
  of unknown/corrupt records. KV and dotfile stores additionally demonstrate
  domain-specific materialized databases.
- Settings are defined in `atuin-client/src/settings.rs`, with small domain
  modules for database-backed features. Defaults are supplied by
  `builder_with_data_dir`, paths are expanded during config loading, and regex
  validation happens during deserialization/explicit validation.
- Client commands are registered by the `client::Cmd` clap enum and dispatched
  after opening the history and record databases. Clipboard will be dispatched
  independently and open only `clipboard.db` plus the generic record store.
- Interactive search keeps one terminal lifecycle and one renderer. An
  internal `SearchItem` adapter exposes content, time, host, and optional
  shell-only columns to the existing list and preview code; it never converts
  clipboard entries into `History`. Ctrl-V swaps the active query source while
  preserving the input editor, tab, keymap mode, selection, and viewport.
- Existing clipboard writes use optional `arboard` support in the `atuin`
  crate. The new domain crate owns a backend trait, a production arboard
  backend, and a fake backend for tests.
- CI checks Linux, macOS, and Windows, including workspace all-features and
  no-default-feature configurations. Platform clipboard dependencies must be
  target-scoped and unsupported platforms must continue compiling.

## Incremental plan

1. Add `atuin-clipboard` with independent entry/record types, SQLite
   migrations and APIs, backend abstraction, watcher filtering/deduplication,
   materialization, and unit tests.
2. Add opt-in clipboard settings and validate exclusion regexes and polling
   bounds while preserving the documented defaults.
3. Register a daemon clipboard component that owns the database/store/watcher,
   responds to reload, performs synchronized retention, and shuts down with a
   bounded wait.
4. Add `atuin clipboard` commands and route both interactive entry points
   through the existing history-search terminal, layout, list, preview, and
   inspector container. Clipboard actions provide exact restore/copy and
   synchronized single or exact-content deletion.
5. Pass downloaded record IDs to the clipboard materializer after sync and
   emit clipboard domain events. Clipboard decode failures remain isolated.
6. Document privacy implications and validate affected crates, then the full
   workspace where practical.

No history schema/type changes, standalone daemon/binary, or sync-server API
changes are planned.
