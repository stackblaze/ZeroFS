use crate::fs::dataset::Dataset;
/// Virtual filesystem layer for exposing snapshots as subdirectories
/// This makes snapshots accessible at /.snapshots/<snapshot-name>/
use crate::fs::errors::FsError;
use crate::fs::inode::{DirectoryInode, Inode, InodeId};
use crate::fs::store::DatasetStore;

/// Special inode ID for the .snapshots directory
/// We use a very high ID that won't conflict with regular inodes
pub const SNAPSHOTS_DIR_INODE: InodeId = u64::MAX - 1000;

/// Base inode ID for virtual snapshot directory entries
/// Each snapshot gets SNAPSHOT_BASE_INODE + snapshot_id
pub const SNAPSHOT_BASE_INODE: InodeId = u64::MAX - 1_000_000;

#[derive(Clone)]
pub struct SnapshotVfs {
    dataset_store: DatasetStore,
}

impl SnapshotVfs {
    pub fn new(dataset_store: DatasetStore) -> Self {
        Self { dataset_store }
    }

    /// Check if this is the .snapshots directory inode
    pub fn is_snapshots_dir(inode_id: InodeId) -> bool {
        inode_id == SNAPSHOTS_DIR_INODE
    }

    /// Check if this is a virtual snapshot directory inode
    pub fn is_snapshot_dir(inode_id: InodeId) -> bool {
        inode_id >= SNAPSHOT_BASE_INODE && inode_id < SNAPSHOTS_DIR_INODE
    }

    /// Get snapshot ID from virtual inode ID
    pub fn snapshot_id_from_inode(inode_id: InodeId) -> Option<u64> {
        if Self::is_snapshot_dir(inode_id) {
            Some(inode_id - SNAPSHOT_BASE_INODE)
        } else {
            None
        }
    }

    /// Get virtual inode ID for a snapshot
    pub fn inode_for_snapshot(snapshot_id: u64) -> InodeId {
        SNAPSHOT_BASE_INODE + snapshot_id
    }

    /// Check if filename is ".snapshots"
    pub fn is_snapshots_name(name: &[u8]) -> bool {
        name == b".snapshots"
    }

    /// Look up an entry in the .snapshots directory
    pub async fn lookup_in_snapshots(&self, name: &[u8]) -> Result<InodeId, FsError> {
        let name_str = std::str::from_utf8(name).map_err(|_| FsError::InvalidArgument)?;

        // Find the snapshot by name
        let snapshot = self
            .dataset_store
            .get_by_name(name_str)
            .await
            .ok_or(FsError::NotFound)?;

        // Only allow access to actual snapshots, not regular datasets
        if !snapshot.is_snapshot {
            return Err(FsError::NotFound);
        }

        // Return the virtual inode for this snapshot directory
        Ok(Self::inode_for_snapshot(snapshot.id))
    }

    /// Get the inode for .snapshots directory (virtual)
    pub fn get_snapshots_dir_inode(&self, created_at: u64) -> Inode {
        Inode::Directory(DirectoryInode {
            mtime: created_at,
            mtime_nsec: 0,
            ctime: created_at,
            ctime_nsec: 0,
            atime: created_at,
            atime_nsec: 0,
            mode: 0o555, // Read-only, world-readable
            uid: 0,
            gid: 0,
            entry_count: 0, // Will be computed dynamically
            parent: 0,      // Root directory
            name: Some(b".snapshots".to_vec()),
            nlink: 2,
        })
    }

    /// Get the virtual inode for a snapshot directory
    pub async fn get_snapshot_dir_inode(&self, snapshot_id: u64) -> Result<Inode, FsError> {
        let snapshot = self
            .dataset_store
            .get_by_id(snapshot_id)
            .await
            .ok_or(FsError::NotFound)?;

        if !snapshot.is_snapshot {
            return Err(FsError::InvalidArgument);
        }

        // Create a virtual directory inode that points to the snapshot's root
        // Use read-write permissions if snapshot is not readonly (like btrfs)
        let mode = if snapshot.is_readonly { 0o555 } else { 0o755 };
        Ok(Inode::Directory(DirectoryInode {
            mtime: snapshot.created_at,
            mtime_nsec: 0,
            ctime: snapshot.created_at,
            ctime_nsec: 0,
            atime: snapshot.created_at,
            atime_nsec: 0,
            mode,
            uid: 0,
            gid: 0,
            entry_count: 0, // From actual snapshot root
            parent: SNAPSHOTS_DIR_INODE,
            name: Some(snapshot.name.as_bytes().to_vec()),
            nlink: 2,
        }))
    }

    /// Get the actual root inode ID for a snapshot
    pub async fn get_snapshot_root_inode(&self, snapshot_id: u64) -> Result<InodeId, FsError> {
        let snapshot = self
            .dataset_store
            .get_by_id(snapshot_id)
            .await
            .ok_or(FsError::NotFound)?;

        if !snapshot.is_snapshot {
            return Err(FsError::InvalidArgument);
        }

        Ok(snapshot.root_inode)
    }

    /// List all snapshots for readdir on .snapshots
    pub async fn list_snapshots(&self) -> Vec<Dataset> {
        self.dataset_store.list_snapshots().await
    }

    /// Check if an inode should be treated as read-only (snapshot content)
    pub async fn is_readonly_context(&self, inode_id: InodeId) -> bool {
        // Virtual snapshot directories are always read-only
        if Self::is_snapshots_dir(inode_id) || Self::is_snapshot_dir(inode_id) {
            return true;
        }

        // TODO: Track which inodes belong to snapshots for full read-only enforcement
        // For now, only the virtual directories are marked read-only
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_inode_ids() {
        assert!(SnapshotVfs::is_snapshots_dir(SNAPSHOTS_DIR_INODE));
        assert!(!SnapshotVfs::is_snapshots_dir(0));
        assert!(!SnapshotVfs::is_snapshots_dir(100));

        let snap_inode = SnapshotVfs::inode_for_snapshot(5);
        assert!(SnapshotVfs::is_snapshot_dir(snap_inode));
        assert_eq!(SnapshotVfs::snapshot_id_from_inode(snap_inode), Some(5));

        assert!(SnapshotVfs::is_snapshots_name(b".snapshots"));
        assert!(!SnapshotVfs::is_snapshots_name(b"snapshots"));
        assert!(!SnapshotVfs::is_snapshots_name(b".snapshot"));
    }
}
