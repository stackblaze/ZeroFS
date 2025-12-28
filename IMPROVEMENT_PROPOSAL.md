# ZeroFS Snapshot/Restore Implementation - Improvement Proposals

## Current Status
✅ **Working**: All tests pass, directory restore works correctly
⚠️ **Can be improved**: Code has duplication and complexity

---

## 1. Eliminate Code Duplication (HIGH PRIORITY)

### Problem
`clone_directory_deep()` (snapshot_manager.rs) and `clone_directory_recursive()` (rpc/server.rs) are ~90% identical:
- Both recursively clone directory entries
- Both skip corrupted entries
- Both use same transaction pattern
- ~150 lines of duplicated code

### Solution
Extract shared logic into a helper struct:

```rust
// zerofs/src/fs/directory_cloner.rs (NEW FILE)
pub struct DirectoryCloner {
    db: Arc<EncryptedDb>,
    inode_store: InodeStore,
    directory_store: DirectoryStore,
}

impl DirectoryCloner {
    pub async fn clone_recursive(
        &self,
        source_dir_inode: InodeId,
        dest_dir_inode: InodeId,
        options: CloneOptions,
    ) -> Result<CloneStats, FsError> {
        // Unified implementation used by both snapshot and restore
    }
}

pub struct CloneOptions {
    pub skip_corrupted: bool,
    pub max_valid_inode_id: InodeId,
    pub await_durable: bool,
}

pub struct CloneStats {
    pub entries_cloned: usize,
    pub entries_skipped: usize,
    pub directories_processed: usize,
}
```

**Benefits**:
- Single source of truth
- Easier to test
- Easier to maintain
- Better error handling consistency

**Effort**: Medium (2-3 hours)

---

## 2. Centralize Validation (MEDIUM PRIORITY)

### Problem
Inode ID validation happens in 3 places:
1. `directory.rs::decode_dir_scan_value()` - during read
2. `snapshot_manager.rs::clone_directory_deep()` - during snapshot
3. `rpc/server.rs::clone_directory_recursive()` - during restore

### Solution
Keep validation ONLY in `decode_dir_scan_value()`:

```rust
// zerofs/src/fs/store/directory.rs

// Define constants
pub const MAX_VALID_INODE_ID: InodeId = 100_000;
pub const MAX_FILENAME_LENGTH: usize = 256;

fn decode_dir_scan_value(data: &[u8]) -> Result<(Vec<u8>, DirScanValue), FsError> {
    // Try new format first...
    
    // Legacy format with strict validation
    if data.len() >= 8 {
        let inode_id = u64::from_le_bytes(data[..8].try_into().unwrap());
        let name = data[8..].to_vec();
        
        // Validate using constants
        if !is_valid_inode_id(inode_id) || !is_valid_filename(&name) {
            return Err(FsError::InvalidData);
        }
        
        // ... rest of logic
    }
}

fn is_valid_inode_id(inode_id: InodeId) -> bool {
    inode_id < MAX_VALID_INODE_ID || 
    inode_id == SNAPSHOTS_ROOT_INODE
}

fn is_valid_filename(name: &[u8]) -> bool {
    !name.is_empty() && 
    name.len() < MAX_FILENAME_LENGTH
}
```

**Benefits**:
- Single validation point
- Corrupted entries never enter the system
- Remove redundant checks elsewhere
- Easier to adjust validation rules

**Effort**: Low (1 hour)

---

## 3. Simplify Flush Strategy (MEDIUM PRIORITY)

### Problem
Complex flush logic with multiple timeouts:
```rust
// Pre-snapshot: 10s timeout, can fail silently
match tokio::time::timeout(Duration::from_secs(10), self.db.flush()).await { ... }

// Post-snapshot: 30s timeout, can fail silently
match tokio::time::timeout(Duration::from_secs(30), self.db.flush()).await { ... }
```

### Solution A: Single Flush Point (Recommended)
```rust
// Only flush AFTER snapshot creation, with proper error handling
pub async fn create_snapshot(...) -> Result<Dataset, FsError> {
    // Flush writeback cache (required)
    if let Some(ref cache) = self.writeback_cache {
        cache.flush_now(self.db.as_ref()).await?;
    }
    
    // Clone directory entries (non-durable writes for speed)
    self.clone_directory_deep(...).await?;
    
    // Single flush at end (with timeout but proper error)
    tokio::time::timeout(
        Duration::from_secs(30),
        self.db.flush()
    )
    .await
    .map_err(|_| FsError::Timeout)?
    .map_err(|_| FsError::IoError)?;
    
    Ok(snapshot)
}
```

### Solution B: Make Flush Optional
```rust
pub struct SnapshotOptions {
    pub await_durable: bool,  // If false, return immediately after clone
}

// For interactive use: await_durable = true
// For batch operations: await_durable = false, periodic flush
```

**Benefits**:
- Clearer error handling
- Predictable behavior
- No silent failures
- Option for async snapshots

**Effort**: Low (1-2 hours)

---

## 4. Extract Magic Numbers to Constants (LOW PRIORITY)

### Problem
Hardcoded values throughout:
- `100_000` - max valid inode ID
- `256` - max filename length
- `10`, `30` - flush timeouts
- `0xFFFFFFFF00000001` - snapshots root inode

### Solution
```rust
// zerofs/src/fs/constants.rs (NEW FILE)
pub mod validation {
    use crate::fs::inode::InodeId;
    
    /// Maximum inode ID for normal filesystem entries
    /// Virtual inodes (snapshots, etc.) use higher IDs
    pub const MAX_NORMAL_INODE_ID: InodeId = 100_000;
    
    /// Maximum filename length (bytes)
    pub const MAX_FILENAME_LENGTH: usize = 256;
    
    /// Minimum filename length (bytes)
    pub const MIN_FILENAME_LENGTH: usize = 1;
}

pub mod timeouts {
    use std::time::Duration;
    
    /// Timeout for database flush operations
    pub const DB_FLUSH_TIMEOUT: Duration = Duration::from_secs(30);
    
    /// Timeout for writeback cache flush
    pub const CACHE_FLUSH_TIMEOUT: Duration = Duration::from_secs(10);
}

pub mod special_inodes {
    use crate::fs::inode::InodeId;
    
    /// Root inode for the /snapshots virtual directory
    pub const SNAPSHOTS_ROOT_INODE: InodeId = 0xFFFFFFFF00000001;
}
```

**Benefits**:
- Self-documenting code
- Easy to tune
- Consistent across codebase

**Effort**: Low (30 minutes)

---

## 5. Improve Error Messages (LOW PRIORITY)

### Problem
Generic error messages make debugging hard:
```rust
.map_err(|_| FsError::IoError)?
```

### Solution
```rust
.map_err(|e| {
    tracing::error!("Failed to flush database during snapshot creation: {}", e);
    FsError::IoError
})?
```

Or create specific error types:
```rust
pub enum SnapshotError {
    DatabaseFlushFailed(String),
    DirectoryCloneFailed { dir_inode: InodeId, reason: String },
    CorruptedEntry { name: String, inode_id: InodeId },
}
```

**Benefits**:
- Better debugging
- Better user experience
- Easier troubleshooting

**Effort**: Medium (2-3 hours)

---

## 6. Add Unit Tests (MEDIUM PRIORITY)

### Problem
No unit tests for:
- `decode_dir_scan_value()` validation
- Directory cloning logic
- Corrupted entry handling

### Solution
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_reject_corrupted_inode_ids() {
        let data = create_legacy_entry(0x2B100000001, b"test");
        assert!(matches!(
            decode_dir_scan_value(&data),
            Err(FsError::InvalidData)
        ));
    }
    
    #[test]
    fn test_accept_valid_inode_ids() {
        let data = create_legacy_entry(42, b"test.txt");
        assert!(decode_dir_scan_value(&data).is_ok());
    }
    
    #[test]
    fn test_accept_virtual_inode_ids() {
        let data = create_legacy_entry(SNAPSHOTS_ROOT_INODE, b"snapshots");
        assert!(decode_dir_scan_value(&data).is_ok());
    }
}
```

**Benefits**:
- Prevent regressions
- Document expected behavior
- Faster development

**Effort**: Medium (3-4 hours)

---

## Implementation Priority

### Phase 1: Quick Wins (1-2 days)
1. ✅ Extract constants (30 min)
2. ✅ Centralize validation (1 hour)
3. ✅ Simplify flush strategy (2 hours)
4. ✅ Improve error messages (2 hours)

### Phase 2: Refactoring (2-3 days)
1. ✅ Extract DirectoryCloner (3 hours)
2. ✅ Add unit tests (4 hours)
3. ✅ Integration tests (2 hours)

### Phase 3: Polish (1 day)
1. ✅ Documentation
2. ✅ Performance profiling
3. ✅ Edge case handling

---

## Estimated Total Effort
- **Minimum viable improvements**: 1-2 days
- **Full refactoring**: 5-7 days
- **With comprehensive testing**: 7-10 days

## Risk Assessment
- **Low risk**: Constants, validation centralization, error messages
- **Medium risk**: Flush strategy changes
- **Higher risk**: DirectoryCloner extraction (needs careful testing)

## Recommendation
Start with **Phase 1** (quick wins) to improve code quality with minimal risk, then evaluate if Phase 2 is needed based on maintenance burden.

