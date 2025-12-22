use crate::encryption::EncryptedDb;
use crate::fs::errors::FsError;
use crate::fs::inode::{DirectoryInode, Inode, InodeId};
use crate::fs::key_codec::{KeyCodec, ParsedKey};
use crate::fs::store::{DirectoryStore, InodeStore, DatasetStore};
use crate::fs::dataset::{Dataset, DatasetId};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use tracing::{debug, info};

fn get_current_time() -> (u64, u32) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    (now.as_secs(), now.subsec_nanos())
}

/// Inode ID for the /snapshots directory (reserved)
pub const SNAPSHOTS_ROOT_INODE: InodeId = 0xFFFFFFFF00000001;

/// Manager for creating and managing Copy-on-Write (COW) snapshots
pub struct SnapshotManager {
    db: Arc<EncryptedDb>,
    inode_store: InodeStore,
    dataset_store: DatasetStore,
    directory_store: DirectoryStore,
}

impl SnapshotManager {
    pub fn new(
        db: Arc<EncryptedDb>,
        inode_store: InodeStore,
        dataset_store: DatasetStore,
        directory_store: DirectoryStore,
    ) -> Self {
        Self {
            db,
            inode_store,
            dataset_store,
            directory_store,
        }
    }

    /// Ensure /snapshots directory exists, create it if it doesn't
    async fn ensure_snapshots_root_directory(&self, root_dir_inode: InodeId) -> Result<(), FsError> {
        // Check if /snapshots directory already exists
        if self.directory_store.exists(root_dir_inode, b"snapshots").await? {
            debug!("Snapshots root directory already exists");
            return Ok(());
        }

        info!("Creating /snapshots root directory");
        let (now_sec, _) = get_current_time();
        
        // Create the snapshots directory inode
        let snapshots_dir = Inode::Directory(DirectoryInode {
            mtime: now_sec,
            mtime_nsec: 0,
            ctime: now_sec,
            ctime_nsec: 0,
            atime: now_sec,
            atime_nsec: 0,
            mode: 0o755,
            uid: 0,
            gid: 0,
            entry_count: 0,
            parent: root_dir_inode, // Parent is root
            name: Some(b"snapshots".to_vec()),
            nlink: 2,
        });

        // Save the snapshots directory inode (directly to DB, not in transaction)
        let serialized = bincode::serialize(&snapshots_dir).map_err(|_| FsError::IoError)?;
        let key = KeyCodec::inode_key(SNAPSHOTS_ROOT_INODE);
        self.db.put_with_options(
            &key,
            &serialized,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        // Add entry in root directory pointing to /snapshots
        let cookie_key = KeyCodec::dir_cookie_counter_key(root_dir_inode);
        let cookie: u64 = match self.db.get_bytes(&cookie_key).await {
            Ok(Some(val)) => {
                let bytes: [u8; 8] = val.as_ref().try_into().map_err(|_| FsError::IoError)?;
                u64::from_be_bytes(bytes)
            }
            _ => crate::fs::store::directory::COOKIE_FIRST_ENTRY,
        };
        
        let new_cookie = cookie + 1;
        self.db.put_with_options(
            &cookie_key,
            &new_cookie.to_be_bytes(),
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        // Add directory entry
        let entry_key = KeyCodec::dir_entry_key(root_dir_inode, b"snapshots");
        self.db.put_with_options(
            &entry_key,
            &SNAPSHOTS_ROOT_INODE.to_be_bytes(),
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        let scan_key = KeyCodec::dir_scan_key(root_dir_inode, cookie);
        let scan_value = KeyCodec::encode_dir_scan_value(SNAPSHOTS_ROOT_INODE, b"snapshots");
        self.db.put_with_options(
            &scan_key,
            &scan_value,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        info!("Created /snapshots directory at root");
        Ok(())
    }

    /// Create a directory entry for a snapshot in /snapshots/
    async fn create_snapshot_directory(
        &self,
        snapshot_name: &str,
        snapshot_root_inode: InodeId,
        created_at: u64,
    ) -> Result<(), FsError> {
        debug!("Creating snapshot directory entry: /snapshots/{}", snapshot_name);

        // The snapshot already has a root inode, we just need to add it to /snapshots directory
        // First, update the snapshot root's parent to point to snapshots directory
        let mut snapshot_root = self.inode_store.get(snapshot_root_inode).await?;
        
        if let Inode::Directory(dir) = &mut snapshot_root {
            dir.parent = SNAPSHOTS_ROOT_INODE;
            dir.name = Some(snapshot_name.as_bytes().to_vec());
        } else {
            return Err(FsError::NotDirectory);
        }

        // Save updated snapshot root
        let serialized = bincode::serialize(&snapshot_root).map_err(|_| FsError::IoError)?;
        let key = KeyCodec::inode_key(snapshot_root_inode);
        self.db.put_with_options(
            &key,
            &serialized,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        // Add directory entry in /snapshots/ pointing to the snapshot root
        let cookie_key = KeyCodec::dir_cookie_counter_key(SNAPSHOTS_ROOT_INODE);
        let cookie: u64 = match self.db.get_bytes(&cookie_key).await {
            Ok(Some(val)) => {
                let bytes: [u8; 8] = val.as_ref().try_into().map_err(|_| FsError::IoError)?;
                u64::from_be_bytes(bytes)
            }
            _ => crate::fs::store::directory::COOKIE_FIRST_ENTRY,
        };
        
        let new_cookie = cookie + 1;
        self.db.put_with_options(
            &cookie_key,
            &new_cookie.to_be_bytes(),
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        let entry_key = KeyCodec::dir_entry_key(SNAPSHOTS_ROOT_INODE, snapshot_name.as_bytes());
        self.db.put_with_options(
            &entry_key,
            &snapshot_root_inode.to_be_bytes(),
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        let scan_key = KeyCodec::dir_scan_key(SNAPSHOTS_ROOT_INODE, cookie);
        let scan_value = KeyCodec::encode_dir_scan_value(snapshot_root_inode, snapshot_name.as_bytes());
        self.db.put_with_options(
            &scan_key,
            &scan_value,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        // Update /snapshots directory metadata
        let mut snapshots_dir_inode = self.inode_store.get(SNAPSHOTS_ROOT_INODE).await?;
        if let Inode::Directory(dir) = &mut snapshots_dir_inode {
            dir.entry_count += 1;
            dir.mtime = created_at;
            dir.mtime_nsec = 0;
            dir.ctime = created_at;
            dir.ctime_nsec = 0;
        }
        let serialized = bincode::serialize(&snapshots_dir_inode).map_err(|_| FsError::IoError)?;
        let key = KeyCodec::inode_key(SNAPSHOTS_ROOT_INODE);
        self.db.put_with_options(
            &key,
            &serialized,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        info!("Created snapshot directory: /snapshots/{}", snapshot_name);
        Ok(())
    }

    /// Allocate a new inode ID
    pub fn allocate_inode(&self) -> InodeId {
        self.inode_store.allocate()
    }

    /// Create a new dataset
    pub async fn create_dataset(
        &self,
        name: String,
        root_inode: InodeId,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Dataset, FsError> {
        self.dataset_store.create_dataset(name, root_inode, created_at, is_readonly).await
    }

    /// List all datasets
    pub async fn list_datasets(&self) -> Vec<Dataset> {
        self.dataset_store.list_datasets().await
    }

    /// Get dataset by name
    pub async fn get_dataset_by_name(&self, name: &str) -> Option<Dataset> {
        self.dataset_store.get_by_name(name).await
    }

    /// Delete dataset by name
    pub async fn delete_dataset(&self, name: &str) -> Result<(), FsError> {
        // Get the dataset by name
        let dataset = self.dataset_store.get_by_name(name).await
            .ok_or(FsError::NotFound)?;
        
        self.dataset_store.delete_dataset(dataset.id).await?;
        Ok(())
    }

    /// Set default dataset by name
    pub async fn set_default_dataset(&self, name: &str) -> Result<(), FsError> {
        let dataset = self.dataset_store.get_by_name(name).await
            .ok_or(FsError::NotFound)?;
        
        self.dataset_store.set_default(dataset.id).await
    }

    /// Get default dataset ID
    pub async fn get_default_dataset(&self) -> DatasetId {
        self.dataset_store.get_default().await
    }

    /// Create a snapshot by dataset name
    pub async fn create_snapshot_by_name(
        &self,
        source_name: &str,
        snapshot_name: String,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Dataset, FsError> {
        let source = self.dataset_store.get_by_name(source_name).await
            .ok_or(FsError::NotFound)?;
        
        self.create_snapshot(source.id, snapshot_name, created_at, is_readonly).await
    }

    /// Delete snapshot by name
    pub async fn delete_snapshot_by_name(&self, name: &str) -> Result<(), FsError> {
        let snapshot = self.dataset_store.get_by_name(name).await
            .ok_or(FsError::NotFound)?;
        
        if !snapshot.is_snapshot {
            return Err(FsError::InvalidArgument);
        }
        
        self.delete_snapshot(snapshot.id).await
    }

    /// Create a snapshot of a dataset
    /// This creates a COW snapshot by cloning the root directory inode
    /// The actual data chunks are shared until modified (copy-on-write)
    /// Also creates a real directory entry at /snapshots/<name>/ for NFS access
    pub async fn create_snapshot(
        &self,
        source_id: DatasetId,
        snapshot_name: String,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Dataset, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        // Get the source dataset
        let source = self.dataset_store.get_by_id(source_id).await
            .ok_or(FsError::NotFound)?;

        // Clone the root inode of the source dataset
        let source_root_inode = self.inode_store.get(source.root_inode).await?;
        
        // Create a new inode for the snapshot root
        let snapshot_root_id = self.inode_store.allocate();
        
        // Clone the directory inode
        let snapshot_root = match source_root_inode {
            Inode::Directory(dir) => {
                // Clone the directory metadata
                Inode::Directory(DirectoryInode {
                    mtime: dir.mtime,
                    mtime_nsec: dir.mtime_nsec,
                    ctime: created_at,
                    ctime_nsec: 0,
                    atime: dir.atime,
                    atime_nsec: dir.atime_nsec,
                    mode: dir.mode,
                    uid: dir.uid,
                    gid: dir.gid,
                    entry_count: dir.entry_count,
                    parent: snapshot_root_id, // Will be updated later to point to /snapshots
                    name: None, // Will be updated later
                    nlink: dir.nlink,
                })
            }
            _ => return Err(FsError::NotDirectory),
        };

        // Ensure /snapshots directory exists (create in actual root, inode 0)
        self.ensure_snapshots_root_directory(0).await?;

        // Save the cloned root inode
        let serialized = bincode::serialize(&snapshot_root).map_err(|_| FsError::IoError)?;
        let key = KeyCodec::inode_key(snapshot_root_id);
        self.db.put_with_options(
            &key,
            &serialized,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|_| FsError::IoError)?;

        // Create the snapshot metadata in dataset store
        let snapshot = self.dataset_store
            .create_snapshot(source_id, snapshot_name.clone(), snapshot_root_id, created_at, is_readonly)
            .await?;

        // Clone directory entries (COW - they reference the same inodes)
        self.clone_directory_entries(source.root_inode, snapshot_root_id).await?;
        
        // Flush to ensure all entries are persisted
        self.db.flush().await.map_err(|_| FsError::IoError)?;

        // Create real directory entry for the snapshot in /snapshots/
        self.create_snapshot_directory(&snapshot_name, snapshot_root_id, created_at).await?;

        info!("Snapshot '{}' created with real directory at /snapshots/{}", snapshot_name, snapshot_name);
        Ok(snapshot)
    }

    /// Clone directory entries from source to destination
    /// This performs a shallow copy - directory entries point to the same inodes
    /// Subdirectories share their inode IDs and directory entries (true COW)
    /// Actual COW happens when those inodes are modified
    async fn clone_directory_entries(
        &self,
        source_dir_id: InodeId,
        dest_dir_id: InodeId,
    ) -> Result<(), FsError> {
        // Get all entries from source directory using range scan
        // This properly handles non-sequential cookies (e.g., after deletions)
        let mut entries = vec![];
        
        let start_key = Bytes::from(KeyCodec::dir_scan_prefix(source_dir_id));
        let end_key = KeyCodec::dir_scan_end_key(source_dir_id);
        
        let mut iter = self
            .db
            .scan(start_key..end_key)
            .await
            .map_err(|_| FsError::IoError)?;
        
        // Iterate through all entries in the directory
        let mut count = 0;
        const MAX_ENTRIES: usize = 100000; // Safety limit
        
        while let Some(result) = iter.next().await {
            if count >= MAX_ENTRIES {
                tracing::error!("Directory {} has more than {} entries - aborting clone", source_dir_id, MAX_ENTRIES);
                return Err(FsError::IoError);
            }
            
            let (key, value) = result.map_err(|_| FsError::IoError)?;
            
            // Parse cookie from key
            let cookie = match KeyCodec::parse_key(&key) {
                ParsedKey::DirScan { cookie } => cookie,
                _ => {
                    tracing::warn!("Unexpected key type in directory scan for inode {}", source_dir_id);
                    continue;
                }
            };
            
            // Decode entry value to get (inode_id, name)
            let (inode_id, name) = KeyCodec::decode_dir_scan_value(&value)?;
            entries.push((name, inode_id, cookie));
            count += 1;
            
            if count % 100 == 0 {
                tracing::debug!("Scanned {} entries from directory {}", count, source_dir_id);
            }
        }

        // Write cloned entries to destination directory
        let num_entries = entries.len();
        tracing::info!("Cloning {} entries from source inode {} to dest inode {}", num_entries, source_dir_id, dest_dir_id);
        for (name, inode_id, cookie) in entries {
            let name_str = String::from_utf8_lossy(&name);
            tracing::debug!("Cloning entry '{}' (inode {}) from {} to {}", name_str, inode_id, source_dir_id, dest_dir_id);
            
            // Create dir_entry key for destination
            let entry_key = KeyCodec::dir_entry_key(dest_dir_id, &name);
            let entry_value = KeyCodec::encode_dir_entry(inode_id, cookie);
            tracing::info!("Writing entry '{}' to dest inode {}: key={:?}, inode_id={}", name_str, dest_dir_id, entry_key, inode_id);
            self.db.put_with_options(
                &entry_key,
                &entry_value,
                &slatedb::config::PutOptions::default(),
                &slatedb::config::WriteOptions { await_durable: false }
            )
            .await
            .map_err(|_| FsError::IoError)?;

            // Create dir_scan key for destination
            let scan_key = KeyCodec::dir_scan_key(dest_dir_id, cookie);
            let scan_value = KeyCodec::encode_dir_scan_value(inode_id, &name);
            self.db.put_with_options(
                &scan_key,
                &scan_value,
                &slatedb::config::PutOptions::default(),
                &slatedb::config::WriteOptions { await_durable: false }
            )
            .await
            .map_err(|_| FsError::IoError)?;
            
            // Verify the entry was written and is readable
            let verify = self.db.get_bytes(&entry_key).await.map_err(|_| FsError::IoError)?;
            if verify.is_none() {
                tracing::error!("Failed to verify directory entry '{}' was written to inode {}", name_str, dest_dir_id);
                return Err(FsError::IoError);
            }
            tracing::debug!("Verified entry '{}' is readable in inode {}", name_str, dest_dir_id);

            // Increment nlink on the referenced inode (COW - same inode is now referenced by snapshot)
            let inode = self.inode_store.get(inode_id).await?;
            let updated_inode = self.increment_nlink(inode)?;
            let serialized = bincode::serialize(&updated_inode).map_err(|_| FsError::IoError)?;
            let inode_key = KeyCodec::inode_key(inode_id);
            self.db.put_with_options(
                &inode_key,
                &serialized,
                &slatedb::config::PutOptions::default(),
                &slatedb::config::WriteOptions { await_durable: false }
            )
            .await
            .map_err(|_| FsError::IoError)?;
        }

        // Clone the cookie counter
        let source_counter_key = KeyCodec::dir_cookie_counter_key(source_dir_id);
        if let Some(counter_data) = self.db.get_bytes(&source_counter_key).await.map_err(|_| FsError::IoError)? {
            let dest_counter_key = KeyCodec::dir_cookie_counter_key(dest_dir_id);
            self.db.put_with_options(
                &dest_counter_key,
                &counter_data,
                &slatedb::config::PutOptions::default(),
                &slatedb::config::WriteOptions { await_durable: false }
            )
            .await
            .map_err(|_| FsError::IoError)?;
        }
        
        tracing::info!("Successfully cloned and verified all {} entries from inode {} to inode {}", num_entries, source_dir_id, dest_dir_id);

        Ok(())
    }

    /// Increment nlink count on an inode
    fn increment_nlink(&self, inode: Inode) -> Result<Inode, FsError> {
        match inode {
            Inode::File(mut f) => {
                f.nlink = f.nlink.saturating_add(1);
                Ok(Inode::File(f))
            }
            Inode::Directory(mut d) => {
                d.nlink = d.nlink.saturating_add(1);
                Ok(Inode::Directory(d))
            }
            Inode::Symlink(mut s) => {
                s.nlink = s.nlink.saturating_add(1);
                Ok(Inode::Symlink(s))
            }
            Inode::Fifo(mut s) => {
                s.nlink = s.nlink.saturating_add(1);
                Ok(Inode::Fifo(s))
            }
            Inode::Socket(mut s) => {
                s.nlink = s.nlink.saturating_add(1);
                Ok(Inode::Socket(s))
            }
            Inode::CharDevice(mut s) => {
                s.nlink = s.nlink.saturating_add(1);
                Ok(Inode::CharDevice(s))
            }
            Inode::BlockDevice(mut s) => {
                s.nlink = s.nlink.saturating_add(1);
                Ok(Inode::BlockDevice(s))
            }
        }
    }

    /// Delete a snapshot
    /// This decrements reference counts on all inodes in the snapshot
    pub async fn delete_snapshot(&self, snapshot_id: DatasetId) -> Result<(), FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        // Get the snapshot
        let snapshot = self.dataset_store.get_by_id(snapshot_id).await
            .ok_or(FsError::NotFound)?;

        if !snapshot.is_snapshot {
            return Err(FsError::InvalidArgument);
        }

        // TODO: Implement recursive deletion of snapshot tree
        // For now, just remove it from the registry
        self.dataset_store.delete_dataset(snapshot_id).await?;

        Ok(())
    }

    /// List all snapshots
    pub async fn list_snapshots(&self) -> Vec<Dataset> {
        self.dataset_store.list_snapshots().await
    }

    /// Get snapshot info
    pub async fn get_snapshot(&self, snapshot_id: DatasetId) -> Option<Dataset> {
        self.dataset_store.get_by_id(snapshot_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ZeroFS;

    #[tokio::test]
    async fn test_snapshot_creation() {
        let encryption_key = [0u8; 32];
        let fs = ZeroFS::new_in_memory_with_encryption(encryption_key)
            .await
            .unwrap();

        let snapshot_manager = SnapshotManager::new(
            fs.db.clone(),
            fs.inode_store.clone(),
            fs.dataset_store.clone(),
        );

        // Create a snapshot of the root dataset
        let snapshot = snapshot_manager
            .create_snapshot(0, "test-snapshot".to_string(), 5000)
            .await
            .unwrap();

        assert_eq!(snapshot.name, "test-snapshot");
        assert!(snapshot.is_snapshot);
        assert!(snapshot.is_readonly);
        assert_eq!(snapshot.parent_id, Some(0));

        // Verify the snapshot appears in the list
        let snapshots = snapshot_manager.list_snapshots().await;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].name, "test-snapshot");
    }
}

