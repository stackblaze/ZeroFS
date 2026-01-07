use crate::encryption::{EncryptedDb, EncryptedTransaction};
use crate::fs::errors::FsError;
use crate::fs::inode::{Inode, InodeId};
use crate::fs::key_codec::KeyCodec;
use crate::metadata_cache::MetadataCache;
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_HARDLINKS_PER_INODE: u32 = u32::MAX;

#[derive(Clone)]
pub struct InodeStore {
    db: Arc<EncryptedDb>,
    next_id: Arc<AtomicU64>,
    metadata_cache: Option<Arc<MetadataCache>>,
}

impl InodeStore {
    pub fn new(db: Arc<EncryptedDb>, initial_next_id: u64) -> Self {
        Self {
            db,
            next_id: Arc::new(AtomicU64::new(initial_next_id)),
            metadata_cache: None,
        }
    }

    pub fn new_with_cache(
        db: Arc<EncryptedDb>,
        initial_next_id: u64,
        metadata_cache: Arc<MetadataCache>,
    ) -> Self {
        Self {
            db,
            next_id: Arc::new(AtomicU64::new(initial_next_id)),
            metadata_cache: Some(metadata_cache),
        }
    }

    pub fn allocate(&self) -> InodeId {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.load(Ordering::SeqCst)
    }

    pub async fn get(&self, id: InodeId) -> Result<Inode, FsError> {
        // Check metadata cache first
        if let Some(ref cache) = self.metadata_cache {
            if let Some(cached_inode) = cache.get_inode(id) {
                return match cached_inode {
                    Some(inode) => Ok(inode),
                    None => Err(FsError::NotFound), // Cached negative lookup
                };
            }
        }

        let key = KeyCodec::inode_key(id);

        let data = self
            .db
            .get_bytes(&key)
            .await
            .map_err(|e| {
                tracing::error!(
                    "InodeStore::get({}): database get_bytes failed: {:?}",
                    id,
                    e
                );
                FsError::IoError
            })?;

        match data {
            Some(data) => {
                let inode: Inode = bincode::deserialize(&data).map_err(|e| {
                    tracing::warn!(
                        "InodeStore::get({}): failed to deserialize inode data (len={}): {:?}.",
                        id,
                        data.len(),
                        e
                    );
                    FsError::InvalidData
                })?;
                
                // Cache positive lookup
                if let Some(ref cache) = self.metadata_cache {
                    cache.put_inode(id, Some(inode.clone()));
                }
                
                Ok(inode)
            }
            None => {
                // Cache negative lookup
                if let Some(ref cache) = self.metadata_cache {
                    cache.put_inode(id, None);
                }
                tracing::warn!(
                    "InodeStore::get({}): inode key not found in database (key={:?}).",
                    id,
                    key
                );
                Err(FsError::NotFound)
            }
        }
    }

    pub fn save(
        &self,
        txn: &mut EncryptedTransaction,
        id: InodeId,
        inode: &Inode,
    ) -> Result<(), Box<bincode::ErrorKind>> {
        let key = KeyCodec::inode_key(id);
        let data = bincode::serialize(inode)?;
        txn.put_bytes(&key, Bytes::from(data));
        
        // Update cache with new inode data
        if let Some(ref cache) = self.metadata_cache {
            cache.put_inode(id, Some(inode.clone()));
        }
        
        Ok(())
    }

    pub fn delete(&self, txn: &mut EncryptedTransaction, id: InodeId) {
        let key = KeyCodec::inode_key(id);
        txn.delete_bytes(&key);
        
        // Invalidate cache
        if let Some(ref cache) = self.metadata_cache {
            cache.invalidate_inode(id);
        }
    }

    pub fn save_counter(&self, txn: &mut EncryptedTransaction) {
        let key = KeyCodec::system_counter_key();
        let next_id = self.next_id.load(Ordering::SeqCst);
        txn.put_bytes(&key, KeyCodec::encode_counter(next_id));
    }
}
