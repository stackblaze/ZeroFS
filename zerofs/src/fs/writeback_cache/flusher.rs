use super::{FlushSignal, WritebackCache};
use crate::encryption::EncryptedDb;
use crate::task::spawn_named;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

pub struct WritebackFlusher {
    cache: Arc<WritebackCache>,
    db: Arc<EncryptedDb>,
    flush_interval: Duration,
    flush_threshold_bytes: u64,
}

impl WritebackFlusher {
    pub fn new(
        cache: Arc<WritebackCache>,
        db: Arc<EncryptedDb>,
        flush_interval_secs: u64,
        flush_threshold_percent: u8,
        max_bytes: u64,
    ) -> Self {
        let flush_threshold_bytes =
            (max_bytes as f64 * (flush_threshold_percent as f64 / 100.0)) as u64;

        Self {
            cache,
            db,
            flush_interval: Duration::from_secs(flush_interval_secs),
            flush_threshold_bytes,
        }
    }

    pub fn spawn(
        self,
        mut flush_rx: mpsc::UnboundedReceiver<FlushSignal>,
        shutdown: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        spawn_named("writeback-flusher", async move {
            info!(
                "Writeback flusher started: interval={}s, threshold={}MB",
                self.flush_interval.as_secs(),
                self.flush_threshold_bytes / 1_000_000
            );

            let mut interval = tokio::time::interval(self.flush_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        debug!("Time-triggered flush");
                        if let Err(e) = self.flush("time-triggered").await {
                            error!("Time-triggered flush failed: {}", e);
                        }
                    }
                    signal = flush_rx.recv() => {
                        match signal {
                            Some(FlushSignal::SizeTriggered) => {
                                debug!("Size-triggered flush");
                                if let Err(e) = self.flush("size-triggered").await {
                                    error!("Size-triggered flush failed: {}", e);
                                }
                            }
                            Some(FlushSignal::Manual) => {
                                debug!("Manual flush requested");
                                if let Err(e) = self.flush("manual").await {
                                    error!("Manual flush failed: {}", e);
                                }
                            }
                            Some(FlushSignal::TimeTriggered) => {
                                // Already handled by interval.tick()
                            }
                            None => {
                                info!("Flush channel closed, stopping flusher");
                                break;
                            }
                        }
                    }
                    _ = shutdown.cancelled() => {
                        info!("Shutdown signal received, performing final flush");
                        if let Err(e) = self.flush("shutdown").await {
                            error!("Shutdown flush failed: {}", e);
                        }
                        break;
                    }
                }

                // Check if size threshold exceeded (in addition to explicit signals)
                let pending_bytes = self.cache.stats().pending_bytes() as u64;
                if pending_bytes > self.flush_threshold_bytes {
                    debug!(
                        "Size threshold exceeded: {} > {} bytes",
                        pending_bytes, self.flush_threshold_bytes
                    );
                    if let Err(e) = self.flush("threshold-check").await {
                        error!("Threshold-triggered flush failed: {}", e);
                    }
                }
            }

            info!("Writeback flusher stopped");
        })
    }

    async fn flush(&self, trigger: &str) -> Result<(), String> {
        let stats = self.cache.stats();
        let pending_count = stats.pending_count();
        let pending_bytes = stats.pending_bytes();

        if pending_count == 0 {
            debug!("No pending transactions to flush ({})", trigger);
            return Ok(());
        }

        debug!(
            "Flushing {} transactions ({} bytes) [{}]",
            pending_count, pending_bytes, trigger
        );

        self.cache
            .flush_to_backend(&self.db)
            .await
            .map_err(|e| format!("Flush failed: {:?}", e))?;

        info!(
            "Flush complete [{}]: {} transactions, {} bytes",
            trigger, pending_count, pending_bytes
        );

        Ok(())
    }
}
