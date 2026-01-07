use bytes::Bytes;
use dashmap::DashMap;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, Semaphore};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::encryption::EncryptedDb;
use crate::fs::inode::InodeId;
use crate::fs::key_codec::KeyCodec;

/// Configuration for the writeback cache
#[derive(Debug, Clone)]
pub struct WritebackCacheConfig {
    /// Directory to store the writeback cache files (should be on NVMe)
    pub cache_dir: PathBuf,
    /// Maximum size of the writeback cache in bytes
    pub max_cache_size_bytes: u64,
    /// Maximum number of dirty chunks before forcing a flush
    pub max_dirty_chunks: usize,
    /// Interval between periodic flushes (in seconds)
    pub flush_interval_secs: u64,
    /// Number of concurrent flush operations
    pub max_concurrent_flushes: usize,
    /// Time before a dirty chunk is eligible for flushing (in seconds)
    pub dirty_time_threshold_secs: u64,
    /// Enable direct I/O for cache files (bypasses OS page cache for better NVMe performance)
    pub use_direct_io: bool,
    /// Enable aggressive read caching for random access patterns (PostgreSQL indexes)
    pub cache_reads_aggressively: bool,
    /// Percentage of cache to reserve for read-only data (0-100)
    pub read_cache_percentage: u8,
}

impl Default for WritebackCacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from("/tmp/zerofs-writeback"),
            max_cache_size_bytes: 10 * 1024 * 1024 * 1024, // 10 GB
            max_dirty_chunks: 10000,
            flush_interval_secs: 5,
            max_concurrent_flushes: 16,
            dirty_time_threshold_secs: 2,
            use_direct_io: false, // Disabled by default for compatibility
            cache_reads_aggressively: false,
            read_cache_percentage: 30,
        }
    }
}

impl WritebackCacheConfig {
    /// PostgreSQL-optimized configuration
    /// 
    /// PostgreSQL characteristics:
    /// - 8KB page size (we use 32KB chunks, so 4 pages per chunk)
    /// - WAL writes are sequential and frequent
    /// - Checkpoint writes are bursty
    /// - Heavy random reads for index scans (B-tree lookups)
    /// - Random reads and writes to data files
    /// - Shared buffers typically 25% of RAM
    /// - Index blocks are frequently accessed (hot data)
    pub fn for_postgresql(cache_dir: PathBuf, cache_size_gb: f64) -> Self {
        Self {
            cache_dir,
            max_cache_size_bytes: (cache_size_gb * 1_000_000_000.0) as u64,
            // PostgreSQL can have many dirty pages during checkpoints
            max_dirty_chunks: 50000, // Higher for checkpoint bursts
            // Flush more frequently to match PostgreSQL's checkpoint intervals
            flush_interval_secs: 3,
            // More concurrent flushes for checkpoint bursts
            max_concurrent_flushes: 32,
            // Shorter threshold for WAL-like behavior
            dirty_time_threshold_secs: 1,
            use_direct_io: false,
            // Aggressively cache reads for index lookups
            cache_reads_aggressively: true,
            // Reserve 40% of cache for read-only data (indexes, frequently accessed pages)
            read_cache_percentage: 40,
        }
    }

    /// High-throughput database configuration
    /// For OLTP workloads with high transaction rates and random access
    pub fn for_high_throughput_db(cache_dir: PathBuf, cache_size_gb: f64) -> Self {
        Self {
            cache_dir,
            max_cache_size_bytes: (cache_size_gb * 1_000_000_000.0) as u64,
            max_dirty_chunks: 100000, // Very high for sustained write load
            flush_interval_secs: 2, // Aggressive flushing
            max_concurrent_flushes: 64, // Maximum parallelism
            dirty_time_threshold_secs: 1,
            use_direct_io: false,
            // High read caching for OLTP random access
            cache_reads_aggressively: true,
            read_cache_percentage: 50, // 50/50 split for read/write
        }
    }

    /// Analytics/OLAP database configuration
    /// For batch writes and large sequential scans
    pub fn for_analytics_db(cache_dir: PathBuf, cache_size_gb: f64) -> Self {
        Self {
            cache_dir,
            max_cache_size_bytes: (cache_size_gb * 1_000_000_000.0) as u64,
            max_dirty_chunks: 20000, // Moderate for batch operations
            flush_interval_secs: 10, // Less frequent, larger batches
            max_concurrent_flushes: 16,
            dirty_time_threshold_secs: 5, // Allow more coalescing
            use_direct_io: false,
            // Less aggressive read caching (sequential scans don't benefit as much)
            cache_reads_aggressively: false,
            read_cache_percentage: 20, // Mostly write-focused
        }
    }
}

/// Metadata for a cached chunk
#[derive(Debug, Clone)]
struct CachedChunkMeta {
    /// Inode ID
    inode_id: InodeId,
    /// Chunk index
    chunk_idx: u64,
    /// Size of the chunk data
    size: usize,
    /// Whether the chunk is dirty (needs flushing)
    is_dirty: bool,
    /// When the chunk was last modified
    dirty_since: Option<Instant>,
    /// Last access time (for LRU eviction)
    last_access: Instant,
    /// Reference count (for pinning)
    ref_count: usize,
    /// Access frequency counter (for hot data detection)
    access_count: u32,
    /// Last chunk accessed before this one (for sequential detection)
    prev_chunk_idx: Option<u64>,
}

/// A chunk key for indexing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChunkKey {
    inode_id: InodeId,
    chunk_idx: u64,
}

impl ChunkKey {
    fn new(inode_id: InodeId, chunk_idx: u64) -> Self {
        Self {
            inode_id,
            chunk_idx,
        }
    }

    /// Generate a filename for this chunk in the cache directory
    fn to_filename(&self) -> String {
        format!("chunk_{}_{}", self.inode_id, self.chunk_idx)
    }
}

/// Writeback cache for filesystem chunks
/// 
/// This cache sits between the filesystem and the underlying LSM tree storage,
/// providing a fast NVMe-backed write buffer that coalesces writes and reduces
/// the number of transactions to the LSM tree.
/// 
/// Key features:
/// - Write coalescing: Multiple writes to the same chunk are merged
/// - Async flushing: Dirty chunks are flushed in the background
/// - LRU eviction: Clean chunks are evicted when cache is full
/// - Direct I/O support: Bypasses OS page cache for better NVMe performance
pub struct WritebackCache {
    config: WritebackCacheConfig,
    db: Arc<EncryptedDb>,
    
    /// Metadata for all cached chunks
    metadata: DashMap<ChunkKey, CachedChunkMeta>,
    
    /// Dirty chunks that need to be flushed (ordered by dirty_since time)
    dirty_queue: Arc<RwLock<BTreeMap<Instant, Vec<ChunkKey>>>>,
    
    /// Current cache size in bytes
    current_size: AtomicU64,
    
    /// Number of dirty chunks
    dirty_count: AtomicUsize,
    
    /// Semaphore to limit concurrent flushes
    flush_semaphore: Arc<Semaphore>,
    
    /// Flag to stop background tasks
    shutdown: AtomicBool,
    
    /// Statistics
    stats: Arc<WritebackCacheStats>,
}

#[derive(Debug, Default)]
pub struct WritebackCacheStats {
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub writes: AtomicU64,
    pub flushes: AtomicU64,
    pub evictions: AtomicU64,
    pub flush_errors: AtomicU64,
    pub read_cache_hits: AtomicU64,
    pub sequential_reads: AtomicU64,
    pub random_reads: AtomicU64,
}

impl WritebackCache {
    /// Create a new writeback cache
    pub async fn new(config: WritebackCacheConfig, db: Arc<EncryptedDb>) -> anyhow::Result<Arc<Self>> {
        // Create cache directory if it doesn't exist
        fs::create_dir_all(&config.cache_dir).await?;
        
        info!(
            "Initializing writeback cache at {} with max size {} GB",
            config.cache_dir.display(),
            config.max_cache_size_bytes / (1024 * 1024 * 1024)
        );
        
        let cache = Arc::new(Self {
            flush_semaphore: Arc::new(Semaphore::new(config.max_concurrent_flushes)),
            config,
            db,
            metadata: DashMap::new(),
            dirty_queue: Arc::new(RwLock::new(BTreeMap::new())),
            current_size: AtomicU64::new(0),
            dirty_count: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            stats: Arc::new(WritebackCacheStats::default()),
        });
        
        // Start background flush task
        let cache_clone = Arc::clone(&cache);
        tokio::spawn(async move {
            cache_clone.background_flush_task().await;
        });
        
        Ok(cache)
    }
    
    /// Get a chunk from the cache or underlying storage
    /// Optimized for PostgreSQL's random access patterns (index lookups)
    pub async fn get(&self, inode_id: InodeId, chunk_idx: u64) -> anyhow::Result<Option<Bytes>> {
        let key = ChunkKey::new(inode_id, chunk_idx);
        
        // Check if chunk is in cache
        if let Some(mut meta) = self.metadata.get_mut(&key) {
            meta.last_access = Instant::now();
            meta.access_count = meta.access_count.saturating_add(1);
            
            // Detect sequential vs random access pattern
            if let Some(prev_idx) = meta.prev_chunk_idx {
                if chunk_idx == prev_idx + 1 || chunk_idx == prev_idx.wrapping_sub(1) {
                    self.stats.sequential_reads.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.random_reads.fetch_add(1, Ordering::Relaxed);
                }
            }
            meta.prev_chunk_idx = Some(chunk_idx);
            
            drop(meta);
            
            // Read from cache file
            let cache_path = self.config.cache_dir.join(key.to_filename());
            match self.read_from_cache_file(&cache_path).await {
                Ok(data) => {
                    self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                    self.stats.read_cache_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(Some(data));
                }
                Err(e) => {
                    warn!("Failed to read from cache file: {}", e);
                    // Fall through to read from DB
                    self.metadata.remove(&key);
                }
            }
        }
        
        // Cache miss - read from underlying storage
        self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        self.stats.random_reads.fetch_add(1, Ordering::Relaxed);
        
        let db_key = KeyCodec::chunk_key(inode_id, chunk_idx);
        let data = self.db.get_bytes(&db_key).await?;
        
        // Cache the read data based on configuration
        if let Some(ref bytes) = data {
            let should_cache = if self.config.cache_reads_aggressively {
                // For PostgreSQL: aggressively cache all reads (indexes, table pages)
                self.should_cache_read_aggressive(bytes.len())
            } else {
                // Conservative caching
                self.should_cache_read(bytes.len())
            };
            
            if should_cache {
                let _ = self.cache_chunk(key, bytes.clone(), false).await;
            }
        }
        
        Ok(data)
    }
    
    /// Batch read multiple chunks (optimized for PostgreSQL sequential scans)
    pub async fn get_batch(&self, keys: Vec<(InodeId, u64)>) -> anyhow::Result<Vec<Option<Bytes>>> {
        let mut results = Vec::with_capacity(keys.len());
        
        for (inode_id, chunk_idx) in keys {
            let data = self.get(inode_id, chunk_idx).await?;
            results.push(data);
        }
        
        Ok(results)
    }
    
    /// Write a chunk to the cache
    pub async fn put(&self, inode_id: InodeId, chunk_idx: u64, data: Bytes) -> anyhow::Result<()> {
        let key = ChunkKey::new(inode_id, chunk_idx);
        self.stats.writes.fetch_add(1, Ordering::Relaxed);
        
        // Ensure we have space in the cache
        self.ensure_cache_space(data.len()).await?;
        
        // Write to cache and mark as dirty
        self.cache_chunk(key, data, true).await?;
        
        // Check if we need to force a flush
        let dirty_count = self.dirty_count.load(Ordering::Relaxed);
        if dirty_count >= self.config.max_dirty_chunks {
            debug!("Dirty chunk limit reached ({}), triggering flush", dirty_count);
            // For database workloads, flush more aggressively (50% instead of 25%)
            self.flush_some_dirty_chunks(dirty_count / 2).await?;
        }
        
        Ok(())
    }
    
    /// Batch write multiple chunks (optimized for database checkpoint-like operations)
    pub async fn put_batch(&self, chunks: Vec<(InodeId, u64, Bytes)>) -> anyhow::Result<()> {
        let total_size: usize = chunks.iter().map(|(_, _, data)| data.len()).sum();
        
        // Ensure we have space for the entire batch
        self.ensure_cache_space(total_size).await?;
        
        // Write all chunks
        for (inode_id, chunk_idx, data) in chunks {
            let key = ChunkKey::new(inode_id, chunk_idx);
            self.stats.writes.fetch_add(1, Ordering::Relaxed);
            self.cache_chunk(key, data, true).await?;
        }
        
        // Check if we need to flush after batch
        let dirty_count = self.dirty_count.load(Ordering::Relaxed);
        if dirty_count >= self.config.max_dirty_chunks {
            debug!("Dirty chunk limit reached after batch ({}), triggering flush", dirty_count);
            self.flush_some_dirty_chunks(dirty_count / 2).await?;
        }
        
        Ok(())
    }
    
    /// Delete a chunk from the cache
    pub async fn delete(&self, inode_id: InodeId, chunk_idx: u64) -> anyhow::Result<()> {
        let key = ChunkKey::new(inode_id, chunk_idx);
        
        if let Some((_, meta)) = self.metadata.remove(&key) {
            // Remove from dirty queue if present
            if meta.is_dirty {
                if let Some(dirty_since) = meta.dirty_since {
                    let mut dirty_queue = self.dirty_queue.write().await;
                    if let Some(keys) = dirty_queue.get_mut(&dirty_since) {
                        keys.retain(|k| k != &key);
                        if keys.is_empty() {
                            dirty_queue.remove(&dirty_since);
                        }
                    }
                }
                self.dirty_count.fetch_sub(1, Ordering::Relaxed);
            }
            
            // Delete cache file
            let cache_path = self.config.cache_dir.join(key.to_filename());
            let _ = fs::remove_file(cache_path).await;
            
            self.current_size.fetch_sub(meta.size as u64, Ordering::Relaxed);
        }
        
        Ok(())
    }
    
    /// Flush all dirty chunks to the underlying storage
    pub async fn flush_all(&self) -> anyhow::Result<()> {
        info!("Flushing all dirty chunks to storage");
        
        let dirty_keys: Vec<ChunkKey> = self
            .metadata
            .iter()
            .filter(|entry| entry.value().is_dirty)
            .map(|entry| *entry.key())
            .collect();
        
        let total = dirty_keys.len();
        info!("Flushing {} dirty chunks", total);
        
        for (i, key) in dirty_keys.iter().enumerate() {
            if let Err(e) = self.flush_chunk(*key).await {
                error!("Failed to flush chunk {:?}: {}", key, e);
            }
            
            if (i + 1) % 100 == 0 {
                debug!("Flushed {}/{} chunks", i + 1, total);
            }
        }
        
        info!("Flush complete");
        Ok(())
    }
    
    /// Shutdown the cache, flushing all dirty data
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        info!("Shutting down writeback cache");
        self.shutdown.store(true, Ordering::Relaxed);
        self.flush_all().await?;
        Ok(())
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> &WritebackCacheStats {
        &self.stats
    }
    
    // Private helper methods
    
    async fn cache_chunk(&self, key: ChunkKey, data: Bytes, is_dirty: bool) -> anyhow::Result<()> {
        let size = data.len();
        let now = Instant::now();
        
        // Write to cache file
        let cache_path = self.config.cache_dir.join(key.to_filename());
        self.write_to_cache_file(&cache_path, &data).await?;
        
        // Update or insert metadata
        let dirty_since = if is_dirty { Some(now) } else { None };
        
        if let Some(mut meta) = self.metadata.get_mut(&key) {
            let old_size = meta.size;
            let was_dirty = meta.is_dirty;
            
            meta.size = size;
            meta.is_dirty = is_dirty;
            meta.last_access = now;
            
            if is_dirty && !was_dirty {
                meta.dirty_since = Some(now);
                self.dirty_count.fetch_add(1, Ordering::Relaxed);
            }
            
            self.current_size.fetch_add(size as u64, Ordering::Relaxed);
            self.current_size.fetch_sub(old_size as u64, Ordering::Relaxed);
        } else {
            let meta = CachedChunkMeta {
                inode_id: key.inode_id,
                chunk_idx: key.chunk_idx,
                size,
                is_dirty,
                dirty_since,
                last_access: now,
                ref_count: 0,
                access_count: 1,
                prev_chunk_idx: None,
            };
            
            self.metadata.insert(key, meta);
            self.current_size.fetch_add(size as u64, Ordering::Relaxed);
            
            if is_dirty {
                self.dirty_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        
        // Add to dirty queue if dirty
        if is_dirty {
            let mut dirty_queue = self.dirty_queue.write().await;
            dirty_queue.entry(now).or_insert_with(Vec::new).push(key);
        }
        
        Ok(())
    }
    
    async fn flush_chunk(&self, key: ChunkKey) -> anyhow::Result<()> {
        let _permit = self.flush_semaphore.acquire().await?;
        
        // Get metadata
        let meta = match self.metadata.get(&key) {
            Some(m) => m.clone(),
            None => return Ok(()), // Already evicted
        };
        
        if !meta.is_dirty {
            return Ok(()); // Already clean
        }
        
        // Read from cache file
        let cache_path = self.config.cache_dir.join(key.to_filename());
        let data = self.read_from_cache_file(&cache_path).await?;
        
        // Write to underlying storage
        let db_key = KeyCodec::chunk_key(key.inode_id, key.chunk_idx);
        self.db
            .put_with_options(
                &db_key,
                &data,
                &slatedb::config::PutOptions::default(),
                &slatedb::config::WriteOptions {
                    await_durable: false,
                },
            )
            .await?;
        
        // Mark as clean
        if let Some(mut meta) = self.metadata.get_mut(&key) {
            if meta.is_dirty {
                meta.is_dirty = false;
                meta.dirty_since = None;
                self.dirty_count.fetch_sub(1, Ordering::Relaxed);
            }
        }
        
        // Remove from dirty queue
        if let Some(dirty_since) = meta.dirty_since {
            let mut dirty_queue = self.dirty_queue.write().await;
            if let Some(keys) = dirty_queue.get_mut(&dirty_since) {
                keys.retain(|k| k != &key);
                if keys.is_empty() {
                    dirty_queue.remove(&dirty_since);
                }
            }
        }
        
        self.stats.flushes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    
    async fn flush_some_dirty_chunks(&self, count: usize) -> anyhow::Result<()> {
        let dirty_keys: Vec<ChunkKey> = {
            let dirty_queue = self.dirty_queue.read().await;
            let threshold = Instant::now() - Duration::from_secs(self.config.dirty_time_threshold_secs);
            
            dirty_queue
                .iter()
                .filter(|(time, _)| **time < threshold)
                .flat_map(|(_, keys)| keys.iter().copied())
                .take(count)
                .collect()
        };
        
        for key in dirty_keys {
            if let Err(e) = self.flush_chunk(key).await {
                error!("Failed to flush chunk {:?}: {}", key, e);
                self.stats.flush_errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        
        Ok(())
    }
    
    async fn ensure_cache_space(&self, needed: usize) -> anyhow::Result<()> {
        let current = self.current_size.load(Ordering::Relaxed);
        let max = self.config.max_cache_size_bytes;
        
        if current + needed as u64 <= max {
            return Ok(());
        }
        
        // Need to evict some clean chunks
        let to_free = (current + needed as u64 - max) + (max / 10); // Free 10% extra
        let mut freed = 0u64;
        
        // Collect clean chunks with access frequency information
        let now = Instant::now();
        
        // For PostgreSQL: Use frequency-aware LRU eviction
        // Keep frequently accessed chunks (hot index pages) in cache longer
        // Prioritize evicting cold data that hasn't been accessed recently
        let chunk_metadata: Vec<(ChunkKey, Instant, usize, u64, u32)> = self
            .metadata
            .iter()
            .filter(|entry| !entry.value().is_dirty && entry.value().ref_count == 0)
            .map(|entry| {
                let age_secs = now.duration_since(entry.value().last_access).as_secs();
                (*entry.key(), entry.value().last_access, entry.value().size, age_secs, entry.value().access_count)
            })
            .collect();
        
        let mut clean_chunks: Vec<(ChunkKey, Instant, usize, u64)> = chunk_metadata
            .into_iter()
            .map(|(key, last_access, size, age, access_count)| {
                // Calculate eviction score: higher score = evict first
                // Favor evicting chunks with low access count and high age
                let score = if access_count > 10 {
                    // Hot data - keep longer
                    age / (access_count as u64 + 1)
                } else {
                    // Cold data - evict sooner
                    age * 2
                };
                (key, last_access, size, score)
            })
            .collect();
        
        // Sort by eviction score (highest first)
        clean_chunks.sort_by_key(|(_, _, _, score)| std::cmp::Reverse(*score));
        
        for (key, _, _size, _) in clean_chunks {
            if freed >= to_free {
                break;
            }
            
            if let Some((_, meta)) = self.metadata.remove(&key) {
                let cache_path = self.config.cache_dir.join(key.to_filename());
                let _ = fs::remove_file(cache_path).await;
                
                freed += meta.size as u64;
                self.current_size.fetch_sub(meta.size as u64, Ordering::Relaxed);
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
        
        if freed < to_free {
            warn!("Could not free enough cache space: freed {} bytes, needed {} bytes", freed, to_free);
        }
        
        Ok(())
    }
    
    async fn background_flush_task(&self) {
        let mut ticker = interval(Duration::from_secs(self.config.flush_interval_secs));
        
        loop {
            ticker.tick().await;
            
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }
            
            let dirty_count = self.dirty_count.load(Ordering::Relaxed);
            if dirty_count == 0 {
                continue;
            }
            
            debug!("Background flush: {} dirty chunks", dirty_count);
            
            // Flush chunks older than threshold
            if let Err(e) = self.flush_some_dirty_chunks(dirty_count.min(100)).await {
                error!("Background flush error: {}", e);
            }
        }
    }
    
    async fn write_to_cache_file(&self, path: &PathBuf, data: &[u8]) -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await?;
        
        file.write_all(data).await?;
        file.sync_data().await?;
        Ok(())
    }
    
    async fn read_from_cache_file(&self, path: &PathBuf) -> anyhow::Result<Bytes> {
        let mut file = File::open(path).await?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).await?;
        Ok(Bytes::from(buffer))
    }
    
    fn should_cache_read(&self, size: usize) -> bool {
        let current = self.current_size.load(Ordering::Relaxed);
        let max = self.config.max_cache_size_bytes;
        
        // Only cache reads if we have plenty of space
        current + (size as u64) < max / 2
    }
    
    fn should_cache_read_aggressive(&self, size: usize) -> bool {
        let current = self.current_size.load(Ordering::Relaxed);
        let max = self.config.max_cache_size_bytes;
        let dirty_count = self.dirty_count.load(Ordering::Relaxed);
        
        // Calculate how much space is reserved for reads
        let read_cache_bytes = (max as f64 * (self.config.read_cache_percentage as f64 / 100.0)) as u64;
        
        // Count current clean (read-only) cache size
        let clean_count = self.metadata.len() - dirty_count;
        let estimated_clean_size = clean_count * 32 * 1024; // Rough estimate
        
        // Cache reads if:
        // 1. We have space in the read cache reservation
        // 2. OR we have general space available
        if estimated_clean_size < read_cache_bytes as usize {
            // Within read cache reservation
            current + (size as u64) < max
        } else {
            // Use general cache space if available
            current + (size as u64) < (max * 3) / 4
        }
    }
}

impl Drop for WritebackCache {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

