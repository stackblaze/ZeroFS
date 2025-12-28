# Content-Addressable Storage (CAS) Architecture Proposal

## Executive Summary

This proposal outlines a transition from the current `(inode_id, chunk_index)` keyed chunk storage to a content-addressable storage (CAS) system. This change will:

- ✅ Enable **true Copy-on-Write (COW)** for snapshots
- ✅ Provide **automatic deduplication** across all files
- ✅ Simplify snapshot/clone operations (no chunk copying needed)
- ✅ Reduce storage usage for identical data blocks
- ✅ Improve data integrity through content verification

## Current Architecture Problems

### Problem 1: Chunks Keyed by Inode ID

```rust
// Current: Chunks are tied to specific inodes
KeyCodec::chunk_key(inode_id, chunk_index) → chunk_data

// When cloning inode 100 → inode 200:
// - Inode 200 created ✓
// - Chunks still at (100, 0), (100, 1), ... ✗
// - Inode 200 can't access them!
```

**Impact**: Snapshots don't work because cloned inodes can't access their data.

### Problem 2: No Deduplication

```rust
// File A: "Hello World" → stored at (inode_1, 0)
// File B: "Hello World" → stored at (inode_2, 0)
// Same data, stored twice!
```

**Impact**: Wasted storage, especially for common data patterns.

### Problem 3: Complex Snapshot Logic

Current workaround requires:
1. Clone inode metadata
2. Scan all chunks for source inode
3. Copy each chunk entry with new inode ID
4. Manage chunk lifecycle for both inodes

**Impact**: Slow, complex, error-prone.

---

## Proposed Architecture: Content-Addressable Storage

### Core Concept

Store chunks by **content hash**, not inode ID. Multiple inodes can reference the same chunk.

```rust
// Proposed: Chunks keyed by content hash
BLAKE3(chunk_data) → chunk_data

// Inodes store a list of chunk hashes
pub struct FileInode {
    pub size: u64,
    pub chunks: Vec<ChunkHash>,  // ← List of hashes
    // ... other metadata
}
```

### Key Components

#### 1. Chunk Hash Type

```rust
use blake3::Hash;

/// Content hash for a chunk (32 bytes)
pub type ChunkHash = [u8; 32];

/// Chunk metadata stored alongside data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMetadata {
    /// Content hash (also the key)
    pub hash: ChunkHash,
    
    /// Compressed size in bytes
    pub compressed_size: u32,
    
    /// Uncompressed size in bytes
    pub uncompressed_size: u32,
    
    /// Reference count (for garbage collection)
    pub refcount: u32,
    
    /// Compression algorithm used
    pub compression: CompressionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Zstd,
    Lz4,
}
```

#### 2. Updated FileInode Structure

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInode {
    pub size: u64,
    pub mtime: u64,
    pub mtime_nsec: u32,
    pub ctime: u64,
    pub ctime_nsec: u32,
    pub atime: u64,
    pub atime_nsec: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub parent: Option<InodeId>,
    pub name: Option<Vec<u8>>,
    pub nlink: u32,
    
    /// NEW: List of chunk hashes (content-addressable)
    /// Each hash is 32 bytes (BLAKE3)
    pub chunks: Vec<ChunkHash>,
}
```

#### 3. Chunk Store Interface

```rust
pub struct ChunkStore {
    db: Arc<EncryptedDb>,
}

impl ChunkStore {
    /// Write a chunk and return its hash
    pub async fn put(&self, data: &[u8]) -> Result<ChunkHash, FsError> {
        // 1. Compute hash
        let hash = blake3::hash(data);
        
        // 2. Check if chunk already exists (deduplication)
        let key = KeyCodec::chunk_key_by_hash(&hash);
        if let Ok(Some(_)) = self.db.get_bytes(&key).await {
            // Chunk exists, increment refcount
            self.increment_refcount(&hash).await?;
            return Ok(hash.into());
        }
        
        // 3. Compress chunk (optional)
        let compressed = compress(data)?;
        
        // 4. Store chunk data
        self.db.put_bytes(&key, compressed).await?;
        
        // 5. Store metadata with refcount=1
        let metadata = ChunkMetadata {
            hash: hash.into(),
            compressed_size: compressed.len() as u32,
            uncompressed_size: data.len() as u32,
            refcount: 1,
            compression: CompressionType::Zstd,
        };
        self.put_metadata(&hash, &metadata).await?;
        
        Ok(hash.into())
    }
    
    /// Read a chunk by hash
    pub async fn get(&self, hash: &ChunkHash) -> Result<Bytes, FsError> {
        let key = KeyCodec::chunk_key_by_hash(hash);
        let compressed = self.db.get_bytes(&key).await?
            .ok_or(FsError::NotFound)?;
        
        // Decompress if needed
        let data = decompress(&compressed)?;
        Ok(data)
    }
    
    /// Increment reference count
    async fn increment_refcount(&self, hash: &ChunkHash) -> Result<(), FsError> {
        let mut metadata = self.get_metadata(hash).await?;
        metadata.refcount += 1;
        self.put_metadata(hash, &metadata).await
    }
    
    /// Decrement reference count (for garbage collection)
    pub async fn decrement_refcount(&self, hash: &ChunkHash) -> Result<(), FsError> {
        let mut metadata = self.get_metadata(hash).await?;
        metadata.refcount = metadata.refcount.saturating_sub(1);
        
        if metadata.refcount == 0 {
            // Chunk no longer referenced, can be deleted
            self.delete_chunk(hash).await?;
        } else {
            self.put_metadata(hash, &metadata).await?;
        }
        
        Ok(())
    }
}
```

#### 4. Key Codec Changes

```rust
impl KeyCodec {
    /// Chunk key by content hash (NEW)
    pub fn chunk_key_by_hash(hash: &ChunkHash) -> Bytes {
        let mut key = Vec::with_capacity(1 + 32);
        key.push(u8::from(KeyPrefix::Chunk));
        key.extend_from_slice(hash);
        Bytes::from(key)
    }
    
    /// Chunk metadata key (NEW)
    pub fn chunk_metadata_key(hash: &ChunkHash) -> Bytes {
        let mut key = Vec::with_capacity(1 + 32);
        key.push(u8::from(KeyPrefix::ChunkMetadata));
        key.extend_from_slice(hash);
        Bytes::from(key)
    }
    
    // DEPRECATED: Remove after migration
    // pub fn chunk_key(inode_id: InodeId, chunk_index: u64) -> Bytes { ... }
}
```

---

## Benefits

### 1. True Copy-on-Write

```rust
// Snapshot creation (INSTANT):
let snapshot_inode = source_inode.clone();
// That's it! Chunks are shared automatically via hashes
```

No chunk copying needed. Snapshots are instant regardless of file size.

### 2. Automatic Deduplication

```rust
// File A writes "Hello World"
let hash = chunk_store.put(b"Hello World").await?;
file_a.chunks = vec![hash];

// File B writes "Hello World" (same content)
let hash = chunk_store.put(b"Hello World").await?;
// Returns same hash, increments refcount
file_b.chunks = vec![hash];

// Only ONE copy of "Hello World" stored!
```

### 3. Data Integrity

```rust
// Read chunk
let data = chunk_store.get(&hash).await?;

// Verify integrity
let computed_hash = blake3::hash(&data);
assert_eq!(computed_hash.as_bytes(), hash);
```

Corruption is immediately detected.

### 4. Simplified Operations

```rust
// Clone file (COW)
let cloned_inode = source_inode.clone();
// Chunks automatically shared

// Modify cloned file
let new_chunk_hash = chunk_store.put(modified_data).await?;
cloned_inode.chunks[0] = new_chunk_hash;
// Original unchanged, true COW!
```

---

## Implementation Plan

### Phase 1: Add CAS Infrastructure (1-2 weeks)

1. **Define new types** (`ChunkHash`, `ChunkMetadata`)
2. **Implement new `ChunkStore` methods**:
   - `put()` with deduplication
   - `get()` by hash
   - Reference counting
3. **Add key codec methods** for hash-based keys
4. **Write unit tests** for CAS operations

**Deliverable**: CAS chunk storage working alongside old system

### Phase 2: Update FileInode (1 week)

1. **Add `chunks: Vec<ChunkHash>` field** to `FileInode`
2. **Update write operations** to use CAS
3. **Update read operations** to use CAS
4. **Maintain backward compatibility** (read old format, write new format)

**Deliverable**: New files use CAS, old files still readable

### Phase 3: Snapshot/Clone Integration (1 week)

1. **Simplify `clone_directory_deep`** (remove chunk copying)
2. **Update snapshot creation** to use CAS
3. **Update restore operations** to use CAS
4. **Test snapshot/restore thoroughly**

**Deliverable**: Snapshots work correctly with CAS

### Phase 4: Migration Tool (1-2 weeks)

1. **Create migration command** to convert old chunks to CAS
2. **Implement batch processing** for large filesystems
3. **Add progress reporting**
4. **Test on production-like data**

**Deliverable**: Tool to migrate existing data

### Phase 5: Garbage Collection (1 week)

1. **Implement GC scanner** to find unreferenced chunks
2. **Add GC command** to reclaim space
3. **Integrate with file deletion** (decrement refcounts)
4. **Test GC thoroughly**

**Deliverable**: Automatic cleanup of unused chunks

### Phase 6: Cleanup (1 week)

1. **Remove old chunk storage code**
2. **Remove backward compatibility**
3. **Update documentation**
4. **Performance benchmarks**

**Deliverable**: Clean, production-ready CAS system

---

## Migration Strategy

### For Existing Deployments

```bash
# 1. Backup
zerofs backup --output /backup/zerofs-backup.tar.gz

# 2. Upgrade binary
cp zerofs-new /usr/local/bin/zerofs

# 3. Run migration (online, no downtime)
zerofs migrate-to-cas --config /etc/zerofs/zerofs.toml

# 4. Verify
zerofs verify-cas --config /etc/zerofs/zerofs.toml

# 5. Run GC to reclaim old chunk space
zerofs gc --config /etc/zerofs/zerofs.toml
```

### Backward Compatibility

During migration period:
- **Read**: Support both old `(inode_id, chunk_index)` and new `hash` keys
- **Write**: Always use new CAS format
- **Gradual migration**: Convert chunks on-demand during reads

---

## Performance Considerations

### Hash Computation

- **BLAKE3**: ~3 GB/s on modern CPUs (very fast)
- **Overhead**: ~0.01ms per 32KB chunk
- **Mitigation**: Parallel hashing for large files

### Deduplication Lookup

- **Cost**: One DB lookup per chunk write
- **Benefit**: Saves storage if chunk exists
- **Optimization**: Bloom filter to skip lookups for unique chunks

### Reference Counting

- **Cost**: Extra metadata write per chunk operation
- **Benefit**: Enables automatic garbage collection
- **Optimization**: Batch refcount updates

### Storage Overhead

- **Old system**: `(8 bytes inode_id + 8 bytes chunk_index) = 16 bytes` per chunk
- **New system**: `32 bytes hash` per chunk in inode + `metadata` in DB
- **Trade-off**: Slightly more metadata, but enables deduplication

**Net result**: Storage savings from deduplication outweigh metadata overhead

---

## Security Considerations

### Hash Collision Resistance

- **BLAKE3**: 256-bit hash, collision probability ~2^-256
- **Practical**: More likely to have hardware failure than collision
- **Mitigation**: None needed (astronomically unlikely)

### Data Integrity

- **Benefit**: Corruption detected immediately via hash mismatch
- **Enhancement**: Add periodic integrity checks

### Deduplication Attack

- **Scenario**: Attacker writes known data to infer other users' data
- **Mitigation**: 
  - Encrypt chunks before hashing (keyed hash)
  - Or accept risk (similar to btrfs, ZFS)

---

## Comparison with Other Systems

| Feature | ZeroFS (Current) | ZeroFS (CAS) | btrfs | ZFS | Ceph |
|---------|------------------|--------------|-------|-----|------|
| COW Snapshots | ❌ Broken | ✅ Instant | ✅ | ✅ | ✅ |
| Deduplication | ❌ None | ✅ Automatic | ✅ | ✅ | ✅ |
| Content Verification | ❌ | ✅ | ✅ | ✅ | ✅ |
| Chunk Size | 32KB | 32KB | 4KB-128KB | Variable | 4MB |
| Hash Algorithm | N/A | BLAKE3 | SHA256 | SHA256 | SHA1 |

---

## Risks and Mitigation

### Risk 1: Migration Complexity

**Impact**: Data loss during migration
**Mitigation**: 
- Thorough testing on test data
- Mandatory backups before migration
- Rollback capability

### Risk 2: Performance Regression

**Impact**: Slower writes due to hashing
**Mitigation**:
- Benchmark before/after
- Parallel hashing
- Optimize hot paths

### Risk 3: Increased Metadata

**Impact**: More storage for chunk metadata
**Mitigation**:
- Deduplication savings offset metadata cost
- Compress metadata
- Periodic GC

---

## Conclusion

Content-addressable storage is the **correct long-term architecture** for ZeroFS. It:

1. ✅ **Fixes the snapshot bug** permanently
2. ✅ **Adds deduplication** for free
3. ✅ **Improves data integrity**
4. ✅ **Simplifies code** (no chunk copying)
5. ✅ **Aligns with industry standards** (btrfs, ZFS, Ceph)

**Recommendation**: Proceed with implementation in phases, starting with Phase 1.

---

## Next Steps

1. **Review this proposal** with team
2. **Create GitHub issue** tracking implementation
3. **Start Phase 1** (CAS infrastructure)
4. **Weekly progress updates**

---

## Appendix: Code Samples

### Example: Writing a File with CAS

```rust
pub async fn write_file_cas(
    &self,
    inode_id: InodeId,
    offset: u64,
    data: &[u8],
) -> Result<(), FsError> {
    let mut inode = self.inode_store.get(inode_id).await?;
    let file_inode = match &mut inode {
        Inode::File(f) => f,
        _ => return Err(FsError::NotFile),
    };
    
    // Calculate affected chunks
    let start_chunk = offset / CHUNK_SIZE as u64;
    let end_chunk = (offset + data.len() as u64 - 1) / CHUNK_SIZE as u64;
    
    for chunk_idx in start_chunk..=end_chunk {
        // Read existing chunk (if any)
        let mut chunk_data = if chunk_idx < file_inode.chunks.len() {
            self.chunk_store.get(&file_inode.chunks[chunk_idx as usize]).await?
        } else {
            Bytes::from(vec![0u8; CHUNK_SIZE])
        };
        
        // Modify chunk
        let chunk_offset = (chunk_idx * CHUNK_SIZE as u64) as usize;
        let write_start = (offset as usize).saturating_sub(chunk_offset);
        let write_end = ((offset + data.len() as u64) as usize).min(chunk_offset + CHUNK_SIZE);
        let data_start = write_start.saturating_sub(offset as usize);
        let data_end = write_end.saturating_sub(offset as usize);
        
        chunk_data[write_start..write_end].copy_from_slice(&data[data_start..data_end]);
        
        // Store modified chunk (COW!)
        let new_hash = self.chunk_store.put(&chunk_data).await?;
        
        // Update inode
        if chunk_idx < file_inode.chunks.len() {
            // Decrement old chunk refcount
            self.chunk_store.decrement_refcount(&file_inode.chunks[chunk_idx as usize]).await?;
            file_inode.chunks[chunk_idx as usize] = new_hash;
        } else {
            file_inode.chunks.push(new_hash);
        }
    }
    
    // Update file size
    file_inode.size = file_inode.size.max(offset + data.len() as u64);
    
    // Save inode
    self.inode_store.put(inode_id, inode).await?;
    
    Ok(())
}
```

### Example: Snapshot with CAS

```rust
pub async fn create_snapshot_cas(
    &self,
    source_id: DatasetId,
    snapshot_name: String,
) -> Result<Dataset, FsError> {
    // 1. Clone root inode (cheap - just metadata)
    let source_root = self.inode_store.get(source.root_inode).await?;
    let snapshot_root_id = self.inode_store.allocate();
    self.inode_store.put(snapshot_root_id, source_root.clone()).await?;
    
    // 2. Recursively clone directory structure
    self.clone_directory_structure(source.root_inode, snapshot_root_id).await?;
    
    // That's it! Chunks are automatically shared via hashes
    // No chunk copying needed!
    
    Ok(snapshot)
}
```

**Total time**: O(number of inodes), not O(data size) ✨

