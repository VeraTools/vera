//! Best-effort persistence of raw API embeddings across failed indexing runs.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::embedding::EmbeddingProvider;

use super::pipeline::{INDEX_RESUME_SUFFIX, sibling_index_dir};

pub(crate) type EmbeddingKey = [u8; 32];

#[derive(Debug, thiserror::Error)]
pub(crate) enum CheckpointError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("embedding checkpoint lock poisoned")]
    Poisoned,
    #[error("invalid embedding checkpoint vector bytes")]
    InvalidVector,
}

#[derive(Clone)]
pub(crate) struct EmbeddingCheckpoint(Arc<CheckpointState>);

struct CheckpointState {
    connection: Mutex<Connection>,
    disabled: AtomicBool,
    reused: AtomicUsize,
}

impl EmbeddingCheckpoint {
    pub(crate) fn key(text: &str) -> EmbeddingKey {
        Sha256::digest(text.as_bytes()).into()
    }

    pub(crate) fn open(idx_dir: &Path, identity: &str) -> Result<Self, CheckpointError> {
        let dir = sibling_index_dir(idx_dir, INDEX_RESUME_SUFFIX);
        std::fs::create_dir_all(&dir)?;
        let mut connection = Connection::open(dir.join("embeddings.db"))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS vectors (
                 key BLOB PRIMARY KEY, vector BLOB NOT NULL
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS identity (
                 id INTEGER PRIMARY KEY CHECK (id = 1), value TEXT NOT NULL
             );",
        )?;
        let transaction = connection.transaction()?;
        let stored: Option<String> = transaction
            .query_row("SELECT value FROM identity WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        if stored.as_deref() != Some(identity) {
            transaction.execute("DELETE FROM vectors", [])?;
            transaction.execute(
                "INSERT OR REPLACE INTO identity (id, value) VALUES (1, ?1)",
                [identity],
            )?;
        }
        transaction.commit()?;
        Ok(Self(Arc::new(CheckpointState {
            connection: Mutex::new(connection),
            disabled: AtomicBool::new(false),
            reused: AtomicUsize::new(0),
        })))
    }

    pub(crate) fn for_provider(
        idx_dir: &Path,
        provider: &impl EmbeddingProvider,
        model_name: &str,
    ) -> Option<Self> {
        if !provider.checkpoints_embeddings() {
            return None;
        }
        let identity = format!("{model_name}\n{}", provider.document_prefix_identity());
        match Self::open(idx_dir, &identity) {
            Ok(checkpoint) => Some(checkpoint),
            Err(error) => {
                tracing::warn!(%error, "embedding checkpoint unavailable; continuing without saved embeddings");
                None
            }
        }
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, CheckpointError> {
        self.0
            .connection
            .lock()
            .map_err(|_| CheckpointError::Poisoned)
    }

    fn disable(&self, error: impl std::fmt::Display) {
        if !self.0.disabled.swap(true, Ordering::Relaxed) {
            tracing::warn!(%error, "embedding checkpoint failed; continuing without saved embeddings");
        }
    }

    pub(crate) fn lookup(&self, keys: &[EmbeddingKey]) -> Vec<Option<Vec<f32>>> {
        let lookup = || -> Result<Vec<Option<Vec<f32>>>, CheckpointError> {
            let connection = self.connection()?;
            let mut statement = connection.prepare("SELECT vector FROM vectors WHERE key = ?1")?;
            keys.iter()
                .map(|key| {
                    let bytes: Option<Vec<u8>> = statement
                        .query_row([key.as_slice()], |row| row.get(0))
                        .optional()?;
                    bytes
                        .map(|bytes| {
                            let (values, remainder) = bytes.as_chunks::<4>();
                            if !remainder.is_empty() {
                                return Err(CheckpointError::InvalidVector);
                            }
                            Ok(values
                                .iter()
                                .map(|bytes| f32::from_le_bytes(*bytes))
                                .collect())
                        })
                        .transpose()
                })
                .collect()
        };
        if !self.0.disabled.load(Ordering::Relaxed) {
            match lookup() {
                Ok(vectors) => {
                    self.0.reused.fetch_add(
                        vectors.iter().filter(|v| v.is_some()).count(),
                        Ordering::Relaxed,
                    );
                    return vectors;
                }
                Err(error) => self.disable(error),
            }
        }
        vec![None; keys.len()]
    }

    /// One transaction per completed batch, off the async executor.
    pub(crate) async fn store(&self, vectors: &[(EmbeddingKey, &[f32])]) {
        if vectors.is_empty() || self.0.disabled.load(Ordering::Relaxed) {
            return;
        }
        let vectors: Vec<_> = vectors
            .iter()
            .map(|(key, vector)| (*key, vector.to_vec()))
            .collect();
        let checkpoint = self.clone();
        let result = tokio::task::spawn_blocking(move || checkpoint.store_blocking(&vectors)).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.disable(error),
            Err(error) => self.disable(error),
        }
    }

    fn store_blocking(&self, vectors: &[(EmbeddingKey, Vec<f32>)]) -> Result<(), CheckpointError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        {
            let mut statement = transaction
                .prepare("INSERT OR IGNORE INTO vectors (key, vector) VALUES (?1, ?2)")?;
            for (key, vector) in vectors {
                let bytes: Vec<u8> = vector
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect();
                statement.execute(params![key.as_slice(), bytes])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn reused_count(&self) -> usize {
        self.0.reused.load(Ordering::Relaxed)
    }

    pub(crate) fn saved_count(&self) -> usize {
        match self.connection().and_then(|connection| {
            Ok(connection
                .query_row("SELECT COUNT(*) FROM vectors", [], |row| {
                    row.get::<_, i64>(0)
                })?
                .try_into()
                .unwrap_or(0))
        }) {
            Ok(count) => count,
            Err(error) => {
                self.disable(error);
                0
            }
        }
    }

    pub(crate) fn failure_context(checkpoint: Option<&Self>, command: &str) -> String {
        let count = checkpoint.map_or(0, Self::saved_count);
        if count == 0 {
            "embedding generation failed".to_string()
        } else {
            format!(
                "embedding generation failed ({count} embeddings saved; rerun `vera {command}` to resume)"
            )
        }
    }

    /// Call only after dropping the checkpoint, so Windows can remove SQLite files.
    pub(crate) fn remove(idx_dir: &Path) {
        if let Err(error) = std::fs::remove_dir_all(sibling_index_dir(idx_dir, INDEX_RESUME_SUFFIX))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "failed to remove completed embedding checkpoint");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn checkpoint_reopens_bit_identical_vectors_without_duplicates() {
        let root = tempdir().unwrap();
        let idx_dir = root.path().join(".vera");
        let key = EmbeddingCheckpoint::key("exact prepared text");
        let vector = vec![f32::from_bits(0x7fc01234), -0.0, f32::MIN_POSITIVE, -1.25];
        let checkpoint = EmbeddingCheckpoint::open(&idx_dir, "model\nprefix").unwrap();
        checkpoint.store(&[(key, &vector)]).await;
        checkpoint.store(&[(key, &[9.0])]).await;
        assert_eq!(checkpoint.saved_count(), 1);
        drop(checkpoint);

        let reopened = EmbeddingCheckpoint::open(&idx_dir, "model\nprefix").unwrap();
        let hits = reopened.lookup(&[key, EmbeddingCheckpoint::key("miss")]);
        assert_eq!(
            hits[0]
                .as_ref()
                .unwrap()
                .iter()
                .map(|v| v.to_bits())
                .collect::<Vec<_>>(),
            vector.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert!(hits[1].is_none());
        assert_eq!(reopened.reused_count(), 1);
        assert_eq!(reopened.saved_count(), 1);
    }

    #[tokio::test]
    async fn checkpoint_identity_changes_reset_saved_vectors() {
        let root = tempdir().unwrap();
        let idx_dir = root.path().join(".vera");
        let key = EmbeddingCheckpoint::key("text");
        for identity in [
            "model\nprefix",
            "other-model\nprefix",
            "other-model\nother-prefix",
        ] {
            let checkpoint = EmbeddingCheckpoint::open(&idx_dir, identity).unwrap();
            assert_eq!(checkpoint.saved_count(), 0);
            checkpoint.store(&[(key, &[1.0, 2.0])]).await;
            assert_eq!(checkpoint.saved_count(), 1);
        }
    }

    #[tokio::test]
    async fn checkpoint_lookup_and_store_failures_disable_only_the_optimization() {
        let root = tempdir().unwrap();
        let idx_dir = root.path().join(".vera");
        let key = EmbeddingCheckpoint::key("text");
        for fail_lookup in [true, false] {
            let checkpoint = EmbeddingCheckpoint::open(&idx_dir, "model\nprefix").unwrap();
            checkpoint
                .connection()
                .unwrap()
                .execute_batch("DROP TABLE vectors")
                .unwrap();
            if fail_lookup {
                assert_eq!(checkpoint.lookup(&[key]), vec![None]);
            } else {
                checkpoint.store(&[(key, &[1.0])]).await;
            }
            assert!(checkpoint.0.disabled.load(Ordering::Relaxed));
            assert_eq!(checkpoint.lookup(&[key]), vec![None]);
            checkpoint.store(&[(key, &[1.0])]).await;
            assert_eq!(checkpoint.saved_count(), 0);
        }
    }
}
