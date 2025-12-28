# ZeroFS Development Session Summary

**Date**: December 28, 2025  
**Focus**: Snapshot/Restore Bug Fixes and Architecture Improvements

---

## What We Accomplished

### 1. Phase 1 Improvements ✅

**Created `fs/constants.rs` module** with:
- Validation helpers (`is_valid_inode_id()`, `is_valid_filename()`)
- Timeout constants (`DB_FLUSH_TIMEOUT`, `CACHE_FLUSH_TIMEOUT`)
- Special inode constants (`SNAPSHOTS_ROOT_INODE`)
- Unit tests for all validation functions

**Centralized validation logic**:
- Moved validation from 3 places to 1 (`directory.rs`)
- Removed redundant code from `snapshot_manager.rs` and `rpc/server.rs`
- Replaced magic numbers with named constants

**Files modified**:
- ✅ `zerofs/src/fs/constants.rs` (NEW)
- ✅ `zerofs/src/fs/mod.rs`
- ✅ `zerofs/src/fs/store/directory.rs`
- ✅ `zerofs/src/fs/snapshot_manager.rs`
- ✅ `zerofs/src/rpc/server.rs`

**Status**: Code compiles, tests pass

### 2. Chunk COW Implementation (Partial) ⚠️

**Added `copy_chunks_for_cow()` to ChunkStore**:
- Copies chunk metadata from source to destination inode
- Enables snapshots to access file data
- Batched writes for performance

**Modified SnapshotManager**:
- Added `ChunkStore` field
- Integrated chunk copying into `clone_directory_deep()`
- Copies chunks for file inodes during snapshot creation

**Files modified**:
- ✅ `zerofs/src/fs/store/chunk.rs`
- ✅ `zerofs/src/fs/snapshot_manager.rs`
- ✅ `zerofs/src/cli/server.rs`

**Status**: Code compiles, but snapshots still don't work

---

## Root Cause Analysis

### The Fundamental Problem

**Current architecture stores chunks by `(inode_id, chunk_index)`**:

```rust
// Chunk key: PREFIX + inode_id (8 bytes) + chunk_index (8 bytes)
KeyCodec::chunk_key(inode_id, chunk_index) → chunk_data
```

**When cloning an inode**:
1. New inode created with new ID ✓
2. Chunks still keyed by old inode ID ✗
3. New inode can't access its data ✗

**Our workaround** (copying chunk entries):
- Works in theory
- But fails because:
  - Small files (< 32KB) have no chunk entries
  - Validation still rejecting valid snapshot entries
  - Complex and error-prone

### Why Validation Keeps Failing

**Pattern of "corrupted" entries**: `0xXXX00000001`
- Low 32 bits: `0x00000001`
- High 32 bits: Small value (< 1000)

**Examples**:
- `0x2B100000001` (high=689) - Actually corrupted ✓
- `0xC200000001` (high=194) - Valid snapshot entry, but rejected ✗
- `0x76B00000001` (high=1899) - Valid snapshot entry ✓

**The problem**: Can't distinguish corrupted data from valid snapshot inodes using just the inode ID pattern.

---

## The Real Solution: Content-Addressable Storage

### Why CAS is the Answer

**Store chunks by content hash, not inode ID**:

```rust
// Proposed: Chunks keyed by content hash
BLAKE3(chunk_data) → chunk_data

pub struct FileInode {
    pub size: u64,
    pub chunks: Vec<ChunkHash>,  // List of hashes, not inode-specific
    // ... other metadata
}
```

**Benefits**:
1. ✅ **True COW**: Cloning an inode = cloning metadata (instant)
2. ✅ **Automatic deduplication**: Same data = same hash
3. ✅ **No chunk copying**: Chunks shared automatically
4. ✅ **Data integrity**: Hash mismatch = corruption detected
5. ✅ **Simpler code**: No workarounds needed

**Industry standard**: btrfs, ZFS, Ceph all use content-addressable storage.

---

## Documentation Created

### 1. `CONTENT_ADDRESSABLE_STORAGE_PROPOSAL.md` ✅

**Comprehensive design document** covering:
- Current architecture problems
- Proposed CAS design
- Implementation plan (6 phases, ~8-10 weeks)
- Migration strategy
- Performance considerations
- Security implications
- Code samples

**Key sections**:
- Executive summary
- Detailed architecture
- Phase-by-phase implementation plan
- Migration guide for existing deployments
- Comparison with other systems (btrfs, ZFS, Ceph)

### 2. `IMPROVEMENT_PROPOSAL.md` (Existing)

**Documents Phase 1 improvements**:
- Duplicate code issues
- Redundant validation
- Magic numbers
- Areas for simplification

### 3. `REST_API_DOCUMENTATION.md` (Existing)

**REST API for Kubernetes CSI integration**:
- Dataset management
- Snapshot operations
- Restore functionality

---

## Current State

### What Works ✅

- ✅ Server starts and runs
- ✅ Files can be created and read
- ✅ REST API functional
- ✅ RPC server functional
- ✅ Phase 1 improvements (constants, validation)
- ✅ Chunk copying infrastructure exists

### What Doesn't Work ❌

- ❌ Snapshots don't capture file data
- ❌ Directory restore fails (inodes not found)
- ❌ Validation rejecting valid snapshot entries
- ❌ Small files (< 32KB) not handled by chunk copying

### Why It Doesn't Work

**Root cause**: Architectural limitation of `(inode_id, chunk_index)` keyed storage.

**Workarounds tried**:
1. Copy chunk entries → Doesn't work for small files
2. Adjust validation threshold → Can't find right value
3. Skip corrupted entries → Loses valid data

**Conclusion**: Need architectural change (CAS).

---

## Recommended Next Steps

### Option A: Quick Fix (1-2 days)

**Goal**: Get snapshots working with current architecture

**Approach**:
1. Investigate how small files are stored (inline data?)
2. Fix validation to not reject any entries
3. Accept that this is a temporary solution

**Pros**: Fast
**Cons**: Technical debt, no deduplication, complex code

### Option B: Implement CAS (8-10 weeks) ⭐ RECOMMENDED

**Goal**: Proper long-term solution

**Approach**: Follow the 6-phase plan in `CONTENT_ADDRESSABLE_STORAGE_PROPOSAL.md`

**Phase 1** (1-2 weeks):
- Add CAS infrastructure
- New `ChunkHash` type
- Hash-based chunk storage
- Reference counting

**Pros**: 
- Fixes snapshots permanently
- Adds deduplication
- Simplifies code
- Industry-standard approach

**Cons**: Takes time

### Option C: Hybrid Approach (2-3 weeks)

**Goal**: Get snapshots working now, plan CAS migration

**Week 1**: Quick fix for snapshots
**Week 2-3**: Start Phase 1 of CAS implementation
**Future**: Gradual migration to CAS

**Pros**: Immediate results + long-term solution
**Cons**: Some throwaway work

---

## Code Changes Summary

### Files Modified (Committed)

```
zerofs/src/fs/constants.rs              (NEW - 171 lines)
zerofs/src/fs/mod.rs                    (added constants module)
zerofs/src/fs/store/directory.rs        (centralized validation)
zerofs/src/fs/snapshot_manager.rs       (uses constants, removed validation)
zerofs/src/rpc/server.rs                (removed redundant validation)
zerofs/src/fs/store/chunk.rs            (added copy_chunks_for_cow)
zerofs/src/cli/server.rs                (pass ChunkStore to SnapshotManager)
```

### Suggested Commit Message

```
feat: Phase 1 improvements + CAS proposal

Phase 1 (Completed):
- Created fs/constants.rs with validation helpers and timeouts
- Centralized inode ID and filename validation
- Removed redundant validation code
- Replaced magic numbers with named constants
- Added unit tests for validation

Chunk COW Attempt (Partial):
- Added copy_chunks_for_cow() to ChunkStore
- Modified SnapshotManager to copy chunks during clone
- Works for large files, but not small files

Root Cause Identified:
- Chunks keyed by (inode_id, chunk_index) prevents true COW
- Workarounds are complex and incomplete
- Need content-addressable storage

Documentation:
- CONTENT_ADDRESSABLE_STORAGE_PROPOSAL.md: Full CAS design
- SESSION_SUMMARY.md: What we accomplished and next steps

Recommendation: Implement CAS architecture (see proposal)

Testing: Code compiles, server runs, but snapshots still broken
```

---

## Technical Debt

### Immediate

1. **Validation logic**: Too strict, rejects valid entries
2. **Small file handling**: No chunks for files < 32KB
3. **Snapshot creation**: Doesn't capture file data correctly

### Long-term

1. **Chunk storage architecture**: Need CAS
2. **Deduplication**: Not implemented
3. **Garbage collection**: No automatic cleanup
4. **Data integrity**: No content verification

---

## Questions for Decision

1. **Timeline**: Quick fix now, or invest in CAS?
2. **Resources**: How many developers available?
3. **Priority**: Is snapshot functionality blocking other work?
4. **Migration**: Can we afford downtime for CAS migration?

---

## Conclusion

We've made significant progress on code quality (Phase 1) and identified the root cause of the snapshot bug. The **recommended path forward is implementing Content-Addressable Storage** as outlined in the proposal.

This is not just a bug fix—it's an architectural improvement that will:
- Fix snapshots permanently
- Add deduplication
- Improve data integrity
- Align with industry standards

**The investment is worth it.**

---

## Contact

For questions or clarifications about this session:
- Review `CONTENT_ADDRESSABLE_STORAGE_PROPOSAL.md` for technical details
- Check `IMPROVEMENT_PROPOSAL.md` for Phase 1 improvements
- See git history for all code changes

**Next session should start with**: Reviewing the CAS proposal and deciding on implementation approach.

