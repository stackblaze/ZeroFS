//! Filesystem constants and limits
//!
//! This module defines constants used throughout the ZeroFS filesystem implementation,
//! including validation limits, timeouts, and special inode IDs.

use crate::fs::inode::InodeId;
use std::time::Duration;

/// Validation constants for filesystem entries
pub mod validation {
    use super::InodeId;

    /// Maximum inode ID for normal filesystem entries.
    /// 
    /// Normal filesystem inodes (files, directories) use IDs from 0 to this value.
    /// Virtual inodes (snapshots, special directories) use higher IDs.
    /// 
    /// This limit helps detect corrupted legacy directory entries where random
    /// data is misinterpreted as an inode ID.
    pub const MAX_NORMAL_INODE_ID: InodeId = 100_000;

    /// Maximum filename length in bytes.
    /// 
    /// Filenames longer than this are rejected as invalid or corrupted.
    /// This is a reasonable limit that prevents memory issues while supporting
    /// most real-world filenames.
    pub const MAX_FILENAME_LENGTH: usize = 256;

    /// Minimum filename length in bytes.
    /// 
    /// Empty filenames are not allowed.
    pub const MIN_FILENAME_LENGTH: usize = 1;

    /// Check if an inode ID is valid for a normal filesystem entry.
    /// 
    /// Returns true if the inode ID is either:
    /// - A normal inode (< MAX_NORMAL_INODE_ID)
    /// - A known virtual inode (e.g., SNAPSHOTS_ROOT_INODE)
    #[inline]
    pub fn is_valid_inode_id(inode_id: InodeId) -> bool {
        inode_id < MAX_NORMAL_INODE_ID || is_virtual_inode(inode_id)
    }

    /// Check if an inode ID is a known virtual inode.
    #[inline]
    pub fn is_virtual_inode(inode_id: InodeId) -> bool {
        inode_id == super::special_inodes::SNAPSHOTS_ROOT_INODE
    }

    /// Check if a filename is valid.
    /// 
    /// Returns true if the filename:
    /// - Is not empty
    /// - Is not longer than MAX_FILENAME_LENGTH
    #[inline]
    pub fn is_valid_filename(name: &[u8]) -> bool {
        let len = name.len();
        len >= MIN_FILENAME_LENGTH && len <= MAX_FILENAME_LENGTH
    }
}

/// Timeout constants for asynchronous operations
pub mod timeouts {
    use super::Duration;

    /// Timeout for database flush operations during snapshot creation.
    /// 
    /// This is a longer timeout because:
    /// - Snapshots may involve flushing large amounts of data
    /// - Network latency to cloud storage (S3, etc.)
    /// - Better to wait than to fail and lose data
    pub const DB_FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

    /// Timeout for writeback cache flush operations.
    /// 
    /// This is shorter because the writeback cache is typically smaller
    /// and local to the server.
    pub const CACHE_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);
}

/// Special inode IDs reserved for virtual filesystem entries
pub mod special_inodes {
    use super::InodeId;

    /// Root inode for the `/snapshots` virtual directory.
    /// 
    /// This is a very high inode ID (0xFFFFFFFF00000001) that won't conflict
    /// with normal filesystem inodes. The `/snapshots` directory is virtual
    /// and provides read-only access to all snapshots.
    pub const SNAPSHOTS_ROOT_INODE: InodeId = 0xFFFFFFFF00000001;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_normal_inode() {
        assert!(validation::is_valid_inode_id(0));
        assert!(validation::is_valid_inode_id(42));
        assert!(validation::is_valid_inode_id(99_999));
    }

    #[test]
    fn test_invalid_inode() {
        assert!(!validation::is_valid_inode_id(100_001));
        assert!(!validation::is_valid_inode_id(0x2B100000001));
    }

    #[test]
    fn test_valid_virtual_inode() {
        assert!(validation::is_valid_inode_id(special_inodes::SNAPSHOTS_ROOT_INODE));
    }

    #[test]
    fn test_valid_filename() {
        assert!(validation::is_valid_filename(b"test.txt"));
        assert!(validation::is_valid_filename(b"a"));
        assert!(validation::is_valid_filename(&vec![b'x'; 255]));
    }

    #[test]
    fn test_invalid_filename() {
        assert!(!validation::is_valid_filename(b""));
        assert!(!validation::is_valid_filename(&vec![b'x'; 257]));
    }
}

