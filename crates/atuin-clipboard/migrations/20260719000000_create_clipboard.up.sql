CREATE TABLE clipboard_entries (
    id TEXT PRIMARY KEY NOT NULL,
    timestamp INTEGER NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    hostname TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    deleted_at INTEGER
);

CREATE INDEX clipboard_entries_timestamp_idx
    ON clipboard_entries(timestamp DESC);
CREATE INDEX clipboard_entries_hash_idx
    ON clipboard_entries(content_hash);
