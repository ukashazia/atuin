use atuin_client::record::{encryption::PASETO_V4, sqlite_store::SqliteStore, store::Store};
use atuin_common::record::{DecryptedData, Host, HostId, Record, RecordId, RecordIdx};
use eyre::{Result, bail, ensure, eyre};
use rmp::decode::Bytes;

use crate::{ClipboardDatabase, ClipboardEntry, ClipboardId};

pub const CLIPBOARD_TAG: &str = "clipboard";
pub const CLIPBOARD_VERSION: &str = "v0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClipboardRecord {
    Create(ClipboardEntry),
    Delete(ClipboardId),
}

impl ClipboardRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = Vec::new();
        encode::write_array_len(&mut output, 2)?;
        match self {
            Self::Create(entry) => {
                encode::write_u8(&mut output, 0)?;
                encode::write_array_len(&mut output, 8)?;
                encode::write_str(&mut output, &entry.id.0)?;
                let timestamp: i64 = entry.timestamp.unix_timestamp_nanos().try_into()?;
                encode::write_i64(&mut output, timestamp)?;
                encode::write_str(&mut output, &entry.content)?;
                encode::write_str(&mut output, &entry.content_hash)?;
                encode::write_str(&mut output, &entry.hostname)?;
                encode::write_str(&mut output, &entry.mime_type)?;
                encode::write_bool(&mut output, entry.deleted_at.is_some())?;
                let deleted_at = entry
                    .deleted_at
                    .map(|timestamp| timestamp.unix_timestamp_nanos().try_into())
                    .transpose()?
                    .unwrap_or_default();
                encode::write_i64(&mut output, deleted_at)?;
            }
            Self::Delete(id) => {
                encode::write_u8(&mut output, 1)?;
                encode::write_array_len(&mut output, 1)?;
                encode::write_str(&mut output, &id.0)?;
            }
        }
        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

        fn report<E: std::fmt::Debug>(error: E) -> eyre::Report {
            eyre!("{error:?}")
        }

        ensure!(
            version == CLIPBOARD_VERSION,
            "unknown clipboard version {version:?}"
        );
        let mut bytes = Bytes::new(&data.0);
        let fields = decode::read_array_len(&mut bytes).map_err(report)?;
        ensure!(
            fields >= 2,
            "clipboard record must contain at least two fields"
        );
        let record_type = decode::read_u8(&mut bytes).map_err(report)?;
        let payload_fields = decode::read_array_len(&mut bytes).map_err(report)?;
        let remaining = bytes.remaining_slice();

        match record_type {
            0 => {
                ensure!(
                    payload_fields >= 8,
                    "clipboard create record is missing fields"
                );
                let (id, remaining) = decode::read_str_from_slice(remaining).map_err(report)?;
                let mut bytes = Bytes::new(remaining);
                let timestamp = decode::read_i64(&mut bytes).map_err(report)?;
                let (content, remaining) =
                    decode::read_str_from_slice(bytes.remaining_slice()).map_err(report)?;
                let (content_hash, remaining) =
                    decode::read_str_from_slice(remaining).map_err(report)?;
                let (hostname, remaining) =
                    decode::read_str_from_slice(remaining).map_err(report)?;
                let (mime_type, remaining) =
                    decode::read_str_from_slice(remaining).map_err(report)?;
                let mut bytes = Bytes::new(remaining);
                let has_deleted_at = decode::read_bool(&mut bytes).map_err(report)?;
                let deleted_at = decode::read_i64(&mut bytes).map_err(report)?;

                Ok(Self::Create(ClipboardEntry {
                    id: id.parse()?,
                    timestamp: time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(
                        timestamp,
                    ))?,
                    content: content.to_owned(),
                    content_hash: content_hash.to_owned(),
                    hostname: hostname.to_owned(),
                    mime_type: mime_type.to_owned(),
                    deleted_at: has_deleted_at
                        .then(|| {
                            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(deleted_at))
                        })
                        .transpose()?,
                }))
            }
            1 => {
                ensure!(
                    payload_fields >= 1,
                    "clipboard delete record is missing its ID"
                );
                let (id, _) = decode::read_str_from_slice(remaining).map_err(report)?;
                Ok(Self::Delete(id.parse()?))
            }
            other => bail!("unknown clipboard record type {other}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaterializedEvent {
    Created(ClipboardId),
    Deleted(ClipboardId),
}

#[derive(Clone, Debug)]
pub struct ClipboardStore {
    pub store: SqliteStore,
    pub host_id: HostId,
    pub encryption_key: [u8; 32],
}

impl ClipboardStore {
    pub fn new(store: SqliteStore, host_id: HostId, encryption_key: [u8; 32]) -> Self {
        Self {
            store,
            host_id,
            encryption_key,
        }
    }

    async fn push_record(&self, clipboard: ClipboardRecord) -> Result<(RecordId, RecordIdx)> {
        let data = clipboard.serialize()?;
        let idx = self
            .store
            .last(self.host_id, CLIPBOARD_TAG)
            .await?
            .map_or(0, |record| record.idx + 1);
        let record = Record::builder()
            .host(Host::new(self.host_id))
            .version(CLIPBOARD_VERSION.to_owned())
            .tag(CLIPBOARD_TAG.to_owned())
            .idx(idx)
            .data(data)
            .build();
        let id = record.id;
        self.store
            .push(&record.encrypt::<PASETO_V4>(&self.encryption_key))
            .await?;
        Ok((id, idx))
    }

    pub async fn push(&self, entry: ClipboardEntry) -> Result<(RecordId, RecordIdx)> {
        self.push_record(ClipboardRecord::Create(entry)).await
    }

    pub async fn delete(&self, id: ClipboardId) -> Result<(RecordId, RecordIdx)> {
        self.push_record(ClipboardRecord::Delete(id)).await
    }

    fn decode_record(
        &self,
        record: atuin_common::record::Record<atuin_common::record::EncryptedData>,
    ) -> Result<ClipboardRecord> {
        let version = record.version.clone();
        ensure!(
            version == CLIPBOARD_VERSION,
            "unknown clipboard version {version:?}"
        );
        let decrypted = record.decrypt::<PASETO_V4>(&self.encryption_key)?;
        ClipboardRecord::deserialize(&decrypted.data, &version)
    }

    pub async fn build(&self, database: &ClipboardDatabase) -> Result<()> {
        let records = self.store.all_tagged(CLIPBOARD_TAG).await?;
        let mut creates = Vec::new();
        let mut deletes = Vec::new();
        for record in records {
            let record_id = record.id;
            match self.decode_record(record) {
                Ok(ClipboardRecord::Create(entry)) => creates.push(entry),
                Ok(ClipboardRecord::Delete(id)) => deletes.push(id),
                Err(error) => tracing::warn!(
                    record_id = %record_id.0,
                    %error,
                    "failed to decode clipboard record; skipping"
                ),
            }
        }
        for entry in creates {
            database.materialize(&entry).await?;
        }
        for id in deletes {
            database.apply_remote_deletion(&id).await?;
        }
        Ok(())
    }

    pub async fn incremental_build(
        &self,
        database: &ClipboardDatabase,
        downloaded_record_ids: &[RecordId],
    ) -> Result<Vec<MaterializedEvent>> {
        let mut events = Vec::new();
        for id in downloaded_record_ids {
            let Ok(record) = self.store.get(*id).await else {
                continue;
            };
            if record.tag != CLIPBOARD_TAG {
                continue;
            }
            let record_id = record.id;
            let decoded = match self.decode_record(record) {
                Ok(decoded) => decoded,
                Err(error) => {
                    tracing::warn!(
                        record_id = %record_id.0,
                        %error,
                        "failed to decode clipboard record; skipping"
                    );
                    continue;
                }
            };
            match decoded {
                ClipboardRecord::Create(entry) => {
                    let entry_id = entry.id.clone();
                    database.materialize(&entry).await?;
                    events.push(MaterializedEvent::Created(entry_id));
                }
                ClipboardRecord::Delete(entry_id) => {
                    database.apply_remote_deletion(&entry_id).await?;
                    events.push(MaterializedEvent::Deleted(entry_id));
                }
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::utils::uuid_v7;

    use super::*;

    fn sample(content: &str) -> ClipboardEntry {
        ClipboardEntry::new(content.to_owned(), "host".to_owned())
    }

    #[test]
    fn create_and_delete_round_trip() {
        let create = ClipboardRecord::Create(sample("line one\n世界"));
        assert_eq!(
            ClipboardRecord::deserialize(&create.serialize().unwrap(), CLIPBOARD_VERSION).unwrap(),
            create
        );
        let delete = ClipboardRecord::Delete(ClipboardId::new());
        assert_eq!(
            ClipboardRecord::deserialize(&delete.serialize().unwrap(), CLIPBOARD_VERSION).unwrap(),
            delete
        );
    }

    #[test]
    fn unknown_version_and_type_are_rejected() {
        let record = ClipboardRecord::Create(sample("x")).serialize().unwrap();
        assert!(ClipboardRecord::deserialize(&record, "v9").is_err());
        assert!(
            ClipboardRecord::deserialize(&DecryptedData(vec![0x92, 0x09, 0x90]), "v0").is_err()
        );
    }

    #[test]
    fn future_array_fields_are_ignored() {
        let mut bytes = ClipboardRecord::Create(sample("future"))
            .serialize()
            .unwrap()
            .0;
        // The create payload starts with fixarray(8); advertise one optional field and append nil.
        let payload_marker = bytes.iter().position(|byte| *byte == 0x98).unwrap();
        bytes[payload_marker] = 0x99;
        bytes.push(0xc0);
        assert!(ClipboardRecord::deserialize(&DecryptedData(bytes), "v0").is_ok());
    }

    #[test]
    fn empty_and_large_content_round_trip() {
        for content in [String::new(), "界".repeat(200_000)] {
            let record = ClipboardRecord::Create(sample(&content));
            assert_eq!(
                ClipboardRecord::deserialize(&record.serialize().unwrap(), CLIPBOARD_VERSION)
                    .unwrap(),
                record
            );
        }
    }

    #[tokio::test]
    async fn create_delete_materialize_and_ignore_other_tags() {
        let record_store = SqliteStore::new("sqlite::memory:", 1.0).await.unwrap();
        let database = ClipboardDatabase::new("sqlite::memory:", 1.0)
            .await
            .unwrap();
        let store = ClipboardStore::new(record_store, HostId(uuid_v7()), [7; 32]);
        let entry = sample("sync me");
        let (create_id, _) = store.push(entry.clone()).await.unwrap();
        let events = store
            .incremental_build(&database, &[create_id])
            .await
            .unwrap();
        assert_eq!(events, [MaterializedEvent::Created(entry.id.clone())]);
        assert_eq!(database.load(&entry.id).await.unwrap(), Some(entry.clone()));

        let (delete_id, _) = store.delete(entry.id.clone()).await.unwrap();
        store
            .incremental_build(&database, &[delete_id])
            .await
            .unwrap();
        assert_eq!(database.count_active().await.unwrap(), 0);

        let other = Record::builder()
            .host(Host::new(store.host_id))
            .version("v0".to_owned())
            .tag("other".to_owned())
            .idx(0)
            .data(DecryptedData(vec![]))
            .build();
        let other_id = other.id;
        store
            .store
            .push(&other.encrypt::<PASETO_V4>(&store.encryption_key))
            .await
            .unwrap();
        assert!(
            store
                .incremental_build(&database, &[other_id])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn malformed_record_does_not_block_valid_record() {
        let record_store = SqliteStore::new("sqlite::memory:", 1.0).await.unwrap();
        let database = ClipboardDatabase::new("sqlite::memory:", 1.0)
            .await
            .unwrap();
        let store = ClipboardStore::new(record_store, HostId(uuid_v7()), [3; 32]);
        let malformed = Record::builder()
            .host(Host::new(store.host_id))
            .version(CLIPBOARD_VERSION.to_owned())
            .tag(CLIPBOARD_TAG.to_owned())
            .idx(0)
            .data(DecryptedData(vec![0xc0]))
            .build();
        let malformed_id = malformed.id;
        store
            .store
            .push(&malformed.encrypt::<PASETO_V4>(&store.encryption_key))
            .await
            .unwrap();
        let valid = sample("still materialized");
        let (valid_id, _) = store.push(valid.clone()).await.unwrap();
        let events = store
            .incremental_build(&database, &[malformed_id, valid_id])
            .await
            .unwrap();
        assert_eq!(events, [MaterializedEvent::Created(valid.id)]);
    }
}
