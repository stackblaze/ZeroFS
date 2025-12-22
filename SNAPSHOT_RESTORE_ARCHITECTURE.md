# ZeroFS Snapshot & Restore Architecture

## Overview

ZeroFS implements **Copy-on-Write (COW) snapshots** similar to btrfs, where snapshots are instant and space-efficient because they initially share data with the source.

## How Snapshots Work

### 1. **Snapshot Creation Process**

When you create a snapshot with:
```bash
zerofs dataset snapshot -c zerofs.toml root my-snapshot
```

Here's what happens internally:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Get Source Dataset                                     │
│    - Load "root" dataset metadata                         │
│    - Find its root inode (typically inode 0)                │
└─────────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────────┐
│ 2. Create New Root Inode for Snapshot                      │
│    - Allocate new inode ID (e.g., inode 9)                 │
│    - Clone directory metadata (permissions, timestamps)     │
└─────────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────────┐
│ 3. Clone Directory Entries (COW - No Data Copy!)           │
│    - Scan all entries in source directory                   │
│    - For each entry (file/dir):                             │
│      • Copy directory entry → snapshot's inode              │
│      • Increment nlink on original inode                    │
│      • NO file data is copied!                              │
└─────────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────────┐
│ 4. Create Dataset Metadata                               │
│    - Store snapshot info in SlateDB:                        │
│      • Name: "my-snapshot"                                  │
│      • UUID: <new UUID>                                     │
│      • Parent ID: 0 (source dataset)                      │
│      • Root inode: 9 (new snapshot root)                    │
│      • is_snapshot: true                                    │
└─────────────────────────────────────────────────────────────┘
```

### 2. **Copy-on-Write Mechanics**

```
Before Snapshot:
┌──────────┐
│ Root (0) │─────► Inode 5 (file.txt, nlink=1)
└──────────┘              │
                          └─► Chunk 100 (data)

After Snapshot:
┌──────────┐
│ Root (0) │─────► Inode 5 (file.txt, nlink=2) ◄──┐
└──────────┘              │                        │
                          └─► Chunk 100 (data)     │
                                     ▲              │
┌─────────────┐                     │              │
│ Snapshot (9)│─────────────────────┴──────────────┘
└─────────────┘

Both point to SAME inode and SAME data chunks!
```

**When you modify file.txt in root:**
```
After Modification:
┌──────────┐
│ Root (0) │─────► Inode 5' (file.txt, nlink=1) [NEW]
└──────────┘              │
                          └─► Chunk 200 (new data)

┌─────────────┐
│ Snapshot (9)│─────► Inode 5 (file.txt, nlink=1)
└─────────────┘              │
                             └─► Chunk 100 (OLD data preserved!)
```

The snapshot still points to the original data!

### 3. **Key Implementation Details**

#### **Directory Entry Cloning** (`snapshot_manager.rs`)
```rust
async fn clone_directory_entries(
    source_dir_id: InodeId,
    dest_dir_id: InodeId,
) -> Result<()> {
    // Use range scan to get ALL entries (handles gaps in cookies)
    let iter = db.scan(start_key..end_key).await?;
    
    while let Some((key, value)) = iter.next().await {
        let (inode_id, name) = decode_entry(&value);
        
        // Create entry in snapshot directory
        // This creates a NEW directory entry pointing to SAME inode
        db.put(dir_entry_key(dest_dir_id, name), encode(inode_id));
        
        // Increment reference count (nlink) on the shared inode
        let inode = load_inode(inode_id);
        inode.nlink += 1;  // Now referenced by both root and snapshot
        db.put(inode_key(inode_id), serialize(inode));
    }
}
```

#### **Data Structure in SlateDB**
```
Key Prefix Structure:
0x08 + UUID → Dataset metadata (name, parent_id, root_inode)
0x09 + UUID → Snapshot metadata (same as dataset)
0x01 + inode_id → Inode data (file metadata, nlink count)
0x02 + dir_id + name → Directory entry (maps name to inode_id)
0x03 + dir_id + cookie → Directory scan entry (for readdir)
```

## How Restore Works

### 1. **File Restoration Process**

When you restore a file:
```bash
zerofs dataset restore \
  --snapshot my-snapshot \
  --source path/to/file.txt \
  --destination /tmp/restored.txt
```

Here's what happens:

```
┌─────────────────────────────────────────────────────────────┐
│ 1. Lookup Snapshot by Name                                  │
│    - Query SlateDB for snapshot "my-snapshot"               │
│    - Get snapshot root inode (e.g., inode 9)                │
└─────────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────────┐
│ 2. Navigate Path in Snapshot                                │
│    - Start from snapshot root (inode 9)                     │
│    - Lookup "path" → inode 10                               │
│    - Lookup "to" → inode 11                                 │
│    - Lookup "file.txt" → inode 12                           │
└─────────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────────┐
│ 3. Stream File Data via RPC                                 │
│    - Read file inode 12 metadata                            │
│    - Stream chunks over gRPC:                               │
│      • Read chunk 0 → send to client                        │
│      • Read chunk 1 → send to client                        │
│      • ...continue until EOF                                │
└─────────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────────┐
│ 4. Write to Destination                                     │
│    - Client receives streamed data                          │
│    - Write to /tmp/restored.txt                             │
└─────────────────────────────────────────────────────────────┘
```

### 2. **RPC Implementation** (`rpc/server.rs`)

```rust
async fn read_snapshot_file(
    &self,
    request: Request<ReadSnapshotFileRequest>,
) -> Result<Response<impl Stream<Item = ReadSnapshotFileResponse>>> {
    let snapshot = get_snapshot_by_name(snapshot_name)?;
    let snapshot_root = snapshot.root_inode;
    
    // Navigate path within snapshot's directory tree
    let inode_id = self.fs.lookup_path_from_root(
        snapshot_root,  // Start from snapshot root, not filesystem root!
        source_path
    ).await?;
    
    // Stream file contents
    let stream = self.fs.read_file(inode_id).chunks(64KB);
    Ok(Response::new(stream))
}
```

### 3. **Why Restore is Fast (Instant Access)**

Restoration doesn't need to "recover" data from some backup format:

✅ **Snapshot data is already in ZeroFS**: Inodes and chunks exist in SlateDB  
✅ **No decompression needed**: Data is stored in native format  
✅ **Same read path as normal files**: Just use different root inode  
✅ **Streaming**: Large files stream chunk-by-chunk, no full buffering  

## Comparison with Traditional Backups

### Traditional Backup/Restore:
```
Backup:  Copy all data → compress → store in archive
         Time: O(n), Space: O(n)

Restore: Find archive → decompress → extract → copy
         Time: O(n)
```

### ZeroFS COW Snapshot:
```
Snapshot: Clone directory entries only
          Time: O(directory_entries), Space: O(1)
          
Restore:  Stream chunks directly from SlateDB
          Time: O(file_size) - same as normal read!
```

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    ZeroFS Layer                         │
│  ┌──────────────┐           ┌──────────────┐           │
│  │ Dataset    │           │  Snapshot    │           │
│  │ "root" (0)   │           │ "my-snap" (9)│           │
│  │ root_inode:0 │           │ root_inode:9 │           │
│  └──────┬───────┘           └──────┬───────┘           │
│         │                          │                    │
│         ├─► Dir Entry: file.txt → Inode 5 ◄────┤       │
│         │                          │                    │
│         └─► Dir Entry: dir/ ────→ Inode 6 ◄─────┘       │
│                                                          │
├─────────────────────────────────────────────────────────┤
│                   Inode Store                           │
│  Inode 5: FileInode { nlink: 2, chunks: [100,101] }    │
│  Inode 6: DirectoryInode { nlink: 2, entries: [...] }  │
├─────────────────────────────────────────────────────────┤
│                   Chunk Store                           │
│  Chunk 100: [encrypted data block]                     │
│  Chunk 101: [encrypted data block]                     │
├─────────────────────────────────────────────────────────┤
│                    SlateDB                              │
│  LSM-tree based key-value store                        │
│  ├─ Memtable (in-memory writes)                        │
│  ├─ WAL (write-ahead log)                              │
│  └─ SSTables (sorted immutable files)                  │
└─────────────────────────────────────────────────────────┘
```

## Performance Characteristics

| Operation | Time Complexity | Space | Notes |
|-----------|----------------|-------|-------|
| Create snapshot | O(entries) | O(1) | Only clones directory entries |
| Access snapshot file | O(depth) | O(1) | Path lookup + normal read |
| Restore file | O(size) | O(size) | Same as copying any file |
| Delete snapshot | O(entries) | Varies | Decrement nlinks, GC if 0 |

## Current Limitations

1. **File-level restore only**: Cannot restore entire directory trees (TODO)
2. **No incremental snapshots**: Each snapshot is independent
3. **No snapshot diffs**: Cannot show what changed between snapshots
4. **NFS visibility**: Snapshots accessible via CLI/RPC, not yet fully exposed in NFS mounts

## Usage Examples

### Create Snapshot
```bash
# Snapshot the root dataset
zerofs dataset snapshot -c zerofs.toml root backup-$(date +%s)

# Snapshot with read-only flag
zerofs dataset snapshot -c zerofs.toml --readonly root ro-backup
```

### List Snapshots
```bash
zerofs dataset list-snapshots -c zerofs.toml
```

### Restore File
```bash
# Restore specific file from snapshot
zerofs dataset restore \
  -c zerofs.toml \
  --snapshot backup-1234567890 \
  --source path/to/file.txt \
  --destination /tmp/recovered.txt
```

### Get Snapshot Info
```bash
zerofs dataset info -c zerofs.toml backup-1234567890
```

## Implementation Files

- **`zerofs/src/fs/snapshot_manager.rs`**: Core snapshot creation/deletion logic
- **`zerofs/src/fs/dataset.rs`**: Dataset and snapshot data structures
- **`zerofs/src/fs/store/dataset.rs`**: Persistence layer for datasets
- **`zerofs/src/cli/dataset.rs`**: CLI commands for snapshot management
- **`zerofs/src/rpc/server.rs`**: RPC endpoints for snapshot operations
- **`zerofs/src/rpc/proto/admin.proto`**: Protocol buffer definitions

## Key Advantages

✅ **Instant snapshots**: O(directory entries), not O(data size)  
✅ **Space-efficient**: Shares data until COW triggers  
✅ **No performance impact**: Snapshot reads are normal filesystem reads  
✅ **Crash-safe**: All operations are transactional via SlateDB  
✅ **Encrypted**: Snapshots inherit encryption from ZeroFS  

This is a production-ready snapshot implementation similar to modern filesystems like btrfs and ZFS!

