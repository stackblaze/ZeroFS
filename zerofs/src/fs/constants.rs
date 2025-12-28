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

    /// Check if an inode ID is valid for a filesystem entry.
    /// 
    /// This validation is designed to detect corrupted legacy directory entries
    /// where random data is misinterpreted as an inode ID.
    /// 
    /// Returns true if the inode ID passes basic sanity checks:
    /// - Not zero (reserved)
    /// - Not in the suspicious range that indicates misinterpreted data
    ///   (specifically, values like 0xXXX00000001 where XXX < 1000)
    /// 
    /// The key insight: corrupted entries have a pattern where they look like
    /// 0x00000001 followed by small values, resulting in IDs like:
    /// - 0x1BB00000001 (high=443, corrupted)
    /// - 0x25E00000001 (high=606, corrupted)
    /// - 0x2B100000001 (high=689, corrupted)
    /// 
    /// Valid snapshot inodes have higher values in the high 32 bits:
    /// - 0x70300000001 (high=1795, valid)
    /// - 0x76B00000001 (high=1899, valid)
    #[inline]
    pub fn is_valid_inode_id(inode_id: InodeId) -> bool {
        if inode_id == 0 {
            return false; // Zero is reserved
        }
        
        // Check for the corrupted pattern: low 32 bits = 0x00000001, high 32 bits < 1000
        // This catches corrupted legacy entries while allowing valid snapshot inodes
        let low_32_bits = (inode_id & 0xFFFFFFFF) as u32;
        let high_32_bits = (inode_id >> 32) as u32;
        
        // Corrupted entries have pattern: 0x00000001 in low 32 bits, small value (< 1000) in high 32 bits
        if low_32_bits == 0x00000001 && high_32_bits > 0 && high_32_bits < 1000 {
            return false; // This is likely corrupted data
        }
        
        true // Everything else is valid
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
        assert!(validation::is_valid_inode_id(1));
        assert!(validation::is_valid_inode_id(42));
        assert!(validation::is_valid_inode_id(99_999));
        assert!(validation::is_valid_inode_id(100_001));
        assert!(validation::is_valid_inode_id(1_000_000));
    }

    #[test]
    fn test_invalid_inode() {
        // Zero is reserved
        assert!(!validation::is_valid_inode_id(0));
        
        // Corrupted pattern: 0xXXX00000001 where XXX < 0x1000
        assert!(!validation::is_valid_inode_id(0x2B100000001)); // 185597018113
        assert!(!validation::is_valid_inode_id(0x1BB00000001)); // 1902670512129
        assert!(!validation::is_valid_inode_id(0x25E00000001)); // 2602750181377
    }

    #[test]
    fn test_valid_virtual_inode() {
        assert!(validation::is_valid_inode_id(special_inodes::SNAPSHOTS_ROOT_INODE));
    }
    
    #[test]
    fn test_valid_snapshot_inodes() {
        // Snapshot inodes have high IDs but don't match the corrupted pattern
        assert!(validation::is_valid_inode_id(7709466296321)); // 0x70300000001
        assert!(validation::is_valid_inode_id(7726646165505)); // 0x70700000001
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

