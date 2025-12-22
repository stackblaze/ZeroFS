mod flusher;

pub use flusher::WritebackFlusher;

use crate::encryption::EncryptedDb;
use crate::fs::errors::FsError;
use anyhow::Result;
use dashmap::DashMap;
use slatedb::WriteBatch;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

pub type TxnId = u64;

#[derive(Clone)]
pub struct CachedBatch {
    pub id: TxnId,
    pub batch: WriteBatch,
    pub size_bytes: usize,
}

#[derive(Clone)]
pub struct WritebackStats {
    pub total_writes: Arc<AtomicU64>,
    pub total_bytes_written: Arc<AtomicU64>,
    pub total_flushes: Arc<AtomicU64>,
    pub pending_bytes: Arc<AtomicUsize>,
    pub pending_count: Arc<AtomicUsize>,
}

impl WritebackStats {
    fn new() -> Self {
        Self {
            total_writes: Arc::new(AtomicU64::new(0)),
            total_bytes_written: Arc::new(AtomicU64::new(0)),
            total_flushes: Arc::new(AtomicU64::new(0)),
            pending_bytes: Arc::new(AtomicUsize::new(0)),
            pending_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn record_write(&self, bytes: usize) {
        self.total_writes.fetch_add(1, Ordering::Relaxed);
        self.total_bytes_written
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.pending_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.pending_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_flush(&self, txn_count: usize, bytes: usize) {
        self.total_flushes.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes.fetch_sub(bytes, Ordering::Relaxed);
        self.pending_count.fetch_sub(txn_count, Ordering::Relaxed);
    }

    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes.load(Ordering::Relaxed)
    }

    pub fn pending_count(&self) -> usize {
        self.pending_count.load(Ordering::Relaxed)
    }
}

pub struct WritebackCache {
    pending_batches: Arc<DashMap<TxnId, CachedBatch>>,
    next_txn_id: AtomicU64,
    stats: Arc<WritebackStats>,
    max_bytes: u64,
    flush_tx: mpsc::UnboundedSender<FlushSignal>,
}

#[derive(Debug, Clone)]
pub enum FlushSignal {
    TimeTriggered,
    SizeTriggered,
    Manual,
}

impl WritebackCache {
    pub fn new(max_bytes: u64) -> Result<(Self, mpsc::UnboundedReceiver<FlushSignal>)> {
        let stats = Arc::new(WritebackStats::new());
        let (flush_tx, flush_rx) = mpsc::unbounded_channel();

        Ok((
            Self {
                pending_batches: Arc::new(DashMap::new()),
                next_txn_id: AtomicU64::new(1),
                stats,
                max_bytes,
                flush_tx,
            },
            flush_rx,
        ))
    }

    pub async fn write(&self, batch: WriteBatch) -> Result<TxnId, FsError> {
        let txn_id = self.next_txn_id.fetch_add(1, Ordering::SeqCst);

        // Estimate batch size - use approximate size
        // Since we can't access WriteBatch internals, estimate based on common patterns
        let size_bytes = 1000; // Conservative estimate for a batch

        let cached = CachedBatch {
            id: txn_id,
            batch,
            size_bytes,
        };

        // Track in memory
        self.pending_batches.insert(txn_id, cached);
        self.stats.record_write(size_bytes);

        debug!(
            "Cached transaction {} (~{} bytes) in writeback cache, pending: {} txns, {} bytes",
            txn_id,
            size_bytes,
            self.stats.pending_count(),
            self.stats.pending_bytes()
        );

        // Check if we should trigger a flush
        if self.stats.pending_bytes() as u64 > self.max_bytes {
            let _ = self.flush_tx.send(FlushSignal::SizeTriggered);
        }

        Ok(txn_id)
    }

    pub async fn flush_to_backend(&self, db: &EncryptedDb) -> Result<(), FsError> {
        let pending: Vec<_> = self
            .pending_batches
            .iter()
            .map(|entry| entry.value().clone())
            .collect();

        if pending.is_empty() {
            debug!("No pending batches to flush");
            return Ok(());
        }

        info!(
            "Flushing {} pending batches ({} bytes) to backend",
            pending.len(),
            self.stats.pending_bytes()
        );

        let mut total_bytes = 0;
        let mut flushed_ids = Vec::new();

        for cached in pending {
            // Write batch to SlateDB
            db.write_raw_batch(
                cached.batch,
                Vec::new(), // pending_operations - empty for writeback cache
                Vec::new(), // deleted_keys - empty for writeback cache
                &slatedb::config::WriteOptions {
                    await_durable: false,
                },
            )
            .await
            .map_err(|e| {
                error!(
                    "Failed to flush transaction {} to backend: {}",
                    cached.id, e
                );
                FsError::IoError
            })?;

            total_bytes += cached.size_bytes;
            flushed_ids.push(cached.id);
        }

        // Remove from pending tracking
        for txn_id in &flushed_ids {
            self.pending_batches.remove(txn_id);
        }

        self.stats.record_flush(flushed_ids.len(), total_bytes);

        info!(
            "Successfully flushed {} batches ({} bytes), remaining: {} batches, {} bytes",
            flushed_ids.len(),
            total_bytes,
            self.stats.pending_count(),
            self.stats.pending_bytes()
        );

        Ok(())
    }

    pub fn stats(&self) -> Arc<WritebackStats> {
        Arc::clone(&self.stats)
    }

    pub fn trigger_flush(&self) {
        let _ = self.flush_tx.send(FlushSignal::Manual);
    }
}
