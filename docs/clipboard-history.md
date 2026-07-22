# Clipboard history

Atuin can record text copied to the system clipboard and synchronize it using
the same account, encrypted record store, and daemon as shell history. Clipboard
entries are an independent data domain stored locally in `clipboard.db`; they
are never inserted into shell history.

Clipboard capture is disabled by default. Enable both the daemon and capture:

```toml
[daemon]
enabled = true
autostart = true

[clipboard]
enabled = true
```

Open interactive search with `atuin clipboard search --interactive` (or `-i`).
Clipboard search is also available from the normal `atuin search --interactive`
interface: press Ctrl-V to switch between shell history and clipboard history.
The active result list changes in place without opening another TUI, and each
domain keeps its own filter context while sharing the current query and UI
state.

Enter restores the selected clipboard entry and exits. Tab and numeric
selection shortcuts return the exact content to the shell without executing
it. Ctrl-Y copies without exiting, Delete soft-deletes and synchronizes a
deletion, and Escape exits without changing the clipboard or shell buffer.
Non-interactive `search`, `list`, `show`, `copy`, `delete`, `clear`, and
`status` commands are also available.

## Privacy warning

Enabling clipboard history can persist passwords, tokens, private messages,
and other sensitive text. The built-in secret patterns and configurable
`clipboard.exclude` regular expressions reduce accidental capture but cannot
recognize every secret. End-to-end encryption protects synchronized records in
transit and on the sync server; it does not protect `clipboard.db` from users or
processes that can access the local machine.

Only UTF-8 text is captured. Images, file lists, rich text, and source
application metadata are not captured by the initial implementation.
