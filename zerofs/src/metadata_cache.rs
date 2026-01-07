use crate::fs::inode::{Inode, InodeId};
use crate::fs::types::DirEntry;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::interval;
use tracing::{debug, info};

/// Cache key for directory entries
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirEntryKey {
    parent_id: InodeId,
    name: Vec<u8>,
}

/// Cache key for inode lookups
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct InodeKey {
    inode_id: InodeId,
}

/// Cache entry for directory entries
#[derive(Debug, Clone)]
enum DirEntryCacheValue {
    Found(DirEntry),
    NotFound, // Negative lookup cache
}

/// Cache entry for inodes
#[derive(Debug, Clone)]
enum InodeCacheValue {
    Found(Inode),
    NotFound, // Negative lookup cache
}

/// Metadata for cache entries
#[derive(Debug, Clone)]
struct CacheEntryMeta {
    created_at: Instant,
    access_count: u32,
    last_access: Instant,
}

/// High-performance metadata cache for ZeroFS
/// 
/// This cache reduces I/O wait by caching:
/// - Directory entry lookups (positive and negative)
/// - Inode lookups (positive and negative)
/// 
/// Key features:
/// - Negative lookup caching (file not found) to avoid repeated LSM tree queries
/// - LRU eviction for memory efficiency
/// - Automatic invalidation on modifications
/// - Access frequency tracking for hot data
/// 
/// This is separate from the writeback cache which handles chunk data.
/// The metadata cache handles filesystem structure, not file contents.
pub struct MetadataCache {
    /// Directory entry cache
    dir_entries: Arc<DashMap<DirEntryKey, (DirEntryCacheValue, CacheEntryMeta)>>,
    
    /// Inode cache
    inodes: Arc<DashMap<InodeKey, (InodeCacheValue, CacheEntryMeta)>>,
    
    /// Maximum number of directory entries to cache
    max_dir_entries: usize,
    
    /// Maximum number of inodes to cache
    max_inodes: usize,
    
    /// TTL for negative lookups (file not found)
    negative_lookup_ttl: Duration,
    
    /// Statistics
    stats: Arc<MetadataCacheStats>,
    
    /// Shutdown flag
    shutdown: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug, Default)]
pub struct MetadataCacheStats {
    pub dir_hits: AtomicU64,
    pub dir_misses: AtomicU64,
    pub dir_negative_hits: AtomicU64,
    pub inode_hits: AtomicU64,
    pub inode_misses: AtomicU64,
    pub inode_negative_hits: AtomicU64,
    pub evictions: AtomicU64,
    pub invalidations: AtomicU64,
}

impl MetadataCache {
    /// Create a new metadata cache
    pub fn new(max_dir_entries: usize, max_inodes: usize, negative_lookup_ttl_secs: u64) -> Arc<Self> {
        let cache = Arc::new(Self {
            dir_entries: Arc::new(DashMap::new()),
            inodes: Arc::new(DashMap::new()),
            max_dir_entries,
            max_inodes,
            negative_lookup_ttl: Duration::from_secs(negative_lookup_ttl_secs),
            stats: Arc::new(MetadataCacheStats::default()),
            shutdown: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        
        // Start background cleanup task
        let cache_clone = Arc::clone(&cache);
        tokio::spawn(async move {
            cache_clone.background_cleanup_task().await;
        });
        
        cache
    }
    
    /// Get a directory entry from cache
    /// Returns Some(Some(inode_id)) if found, Some(None) if cached as not found, None if not cached
    pub fn get_dir_entry(&self, parent_id: InodeId, name: &[u8]) -> Option<Option<InodeId>> {
        let key = DirEntryKey {
            parent_id,
            name: name.to_vec(),
        };
        
        if let Some(mut entry) = self.dir_entries.get_mut(&key) {
            let (value, meta) = entry.value_mut();
            meta.last_access = Instant::now();
            meta.access_count = meta.access_count.saturating_add(1);
            
            match value {
                DirEntryCacheValue::Found(entry) => {
                    self.stats.dir_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(Some(entry.fileid));
                }
                DirEntryCacheValue::NotFound => {
                    // Check if negative lookup is still valid
                    if meta.created_at.elapsed() < self.negative_lookup_ttl {
                        self.stats.dir_negative_hits.fetch_add(1, Ordering::Relaxed);
                        return Some(None);
                    } else {
                        // Negative lookup expired, remove it
                        drop(entry);
                        self.dir_entries.remove(&key);
                    }
                }
            }
        }
        
        self.stats.dir_misses.fetch_add(1, Ordering::Relaxed);
        None
    }
    
    /// Cache a directory entry (positive or negative)
    pub fn put_dir_entry(&self, parent_id: InodeId, name: &[u8], entry: Option<DirEntry>) {
        let key = DirEntryKey {
            parent_id,
            name: name.to_vec(),
        };
        
        // Ensure we have space
        if self.dir_entries.len() >= self.max_dir_entries {
            self.evict_dir_entries(self.max_dir_entries / 10);
        }
        
        let value = match entry {
            Some(e) => DirEntryCacheValue::Found(e),
            None => DirEntryCacheValue::NotFound,
        };
        
        let meta = CacheEntryMeta {
            created_at: Instant::now(),
            access_count: 1,
            last_access: Instant::now(),
        };
        
        self.dir_entries.insert(key, (value, meta));
    }
    
    /// Get an inode from cache
    pub fn get_inode(&self, inode_id: InodeId) -> Option<Option<Inode>> {
        let key = InodeKey { inode_id };
        
        if let Some(mut entry) = self.inodes.get_mut(&key) {
            let (value, meta) = entry.value_mut();
            meta.last_access = Instant::now();
            meta.access_count = meta.access_count.saturating_add(1);
            
            match value {
                InodeCacheValue::Found(inode) => {
                    self.stats.inode_hits.fetch_add(1, Ordering::Relaxed);
                    return Some(Some(inode.clone()));
                }
                InodeCacheValue::NotFound => {
                    // Check if negative lookup is still valid
                    if meta.created_at.elapsed() < self.negative_lookup_ttl {
                        self.stats.inode_negative_hits.fetch_add(1, Ordering::Relaxed);
                        return Some(None);
                    } else {
                        // Negative lookup expired, remove it
                        drop(entry);
                        self.inodes.remove(&key);
                    }
                }
            }
        }
        
        self.stats.inode_misses.fetch_add(1, Ordering::Relaxed);
        None
    }
    
    /// Cache an inode (positive or negative)
    pub fn put_inode(&self, inode_id: InodeId, inode: Option<Inode>) {
        let key = InodeKey { inode_id };
        
        // Ensure we have space
        if self.inodes.len() >= self.max_inodes {
            self.evict_inodes(self.max_inodes / 10);
        }
        
        let value = match inode {
            Some(i) => InodeCacheValue::Found(i),
            None => InodeCacheValue::NotFound,
        };
        
        let meta = CacheEntryMeta {
            created_at: Instant::now(),
            access_count: 1,
            last_access: Instant::now(),
        };
        
        self.inodes.insert(key, (value, meta));
    }
    
    /// Invalidate directory entry cache for a parent directory
    /// Called when directory entries are modified (create, delete, rename)
    pub fn invalidate_dir_entries(&self, parent_id: InodeId) {
        let keys_to_remove: Vec<DirEntryKey> = self
            .dir_entries
            .iter()
            .filter(|entry| entry.key().parent_id == parent_id)
            .map(|entry| entry.key().clone())
            .collect();
        
        for key in keys_to_remove {
            self.dir_entries.remove(&key);
        }
        
        self.stats.invalidations.fetch_add(1, Ordering::Relaxed);
        debug!("Invalidated directory entries for parent {}", parent_id);
    }
    
    /// Invalidate a specific directory entry
    pub fn invalidate_dir_entry(&self, parent_id: InodeId, name: &[u8]) {
        let key = DirEntryKey {
            parent_id,
            name: name.to_vec(),
        };
        self.dir_entries.remove(&key);
        self.stats.invalidations.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Invalidate inode cache
    pub fn invalidate_inode(&self, inode_id: InodeId) {
        let key = InodeKey { inode_id };
        self.inodes.remove(&key);
        self.stats.invalidations.fetch_add(1, Ordering::Relaxed);
    }
    
    /// Clear all caches (for testing or emergency)
    pub fn clear(&self) {
        self.dir_entries.clear();
        self.inodes.clear();
        info!("Metadata cache cleared");
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> &MetadataCacheStats {
        &self.stats
    }
    
    // Private helper methods
    
    fn evict_dir_entries(&self, count: usize) {
        // Collect entries with access info
        let mut entries: Vec<(DirEntryKey, Instant, u32)> = self
            .dir_entries
            .iter()
            .map(|entry| {
                let meta = &entry.value().1;
                (entry.key().clone(), meta.last_access, meta.access_count)
            })
            .collect();
        
        // Sort by LRU (least recently used first)
        entries.sort_by_key(|(_, last_access, _)| *last_access);
        
        // Evict oldest entries
        for (key, _, _) in entries.into_iter().take(count) {
            if self.dir_entries.remove(&key).is_some() {
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    
    fn evict_inodes(&self, count: usize) {
        // Collect entries with access info
        let mut entries: Vec<(InodeKey, Instant, u32)> = self
            .inodes
            .iter()
            .map(|entry| {
                let meta = &entry.value().1;
                (entry.key().clone(), meta.last_access, meta.access_count)
            })
            .collect();
        
        // Sort by LRU (least recently used first)
        entries.sort_by_key(|(_, last_access, _)| *last_access);
        
        // Evict oldest entries
        for (key, _, _) in entries.into_iter().take(count) {
            if self.inodes.remove(&key).is_some() {
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    
    async fn background_cleanup_task(&self) {
        let mut ticker = interval(Duration::from_secs(60)); // Run every minute
        
        loop {
            ticker.tick().await;
            
            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }
            
            // Remove expired negative lookups
            let now = Instant::now();
            let mut expired_dir = Vec::new();
            let mut expired_inode = Vec::new();
            
            for entry in self.dir_entries.iter() {
                if let DirEntryCacheValue::NotFound = entry.value().0 {
                    if entry.value().1.created_at.elapsed() >= self.negative_lookup_ttl {
                        expired_dir.push(entry.key().clone());
                    }
                }
            }
            
            for entry in self.inodes.iter() {
                if let InodeCacheValue::NotFound = entry.value().0 {
                    if entry.value().1.created_at.elapsed() >= self.negative_lookup_ttl {
                        expired_inode.push(entry.key().clone());
                    }
                }
            }
            
            for key in expired_dir {
                self.dir_entries.remove(&key);
            }
            
            for key in expired_inode {
                self.inodes.remove(&key);
            }
            
            // Evict if over capacity
            if self.dir_entries.len() > self.max_dir_entries {
                self.evict_dir_entries(self.max_dir_entries / 10);
            }
            
            if self.inodes.len() > self.max_inodes {
                self.evict_inodes(self.max_inodes / 10);
            }
        }
    }
}

impl Drop for MetadataCache {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

