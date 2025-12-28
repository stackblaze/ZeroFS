use crate::encryption::EncryptedDb;
use crate::fs::errors::FsError;
use crate::fs::inode::{Inode, InodeId};
use crate::fs::key_codec::KeyCodec;
use crate::fs::store::directory::DirScanValue;
use crate::fs::store::{ChunkStore, DirectoryStore, InodeStore};
use bytes::Bytes;
use futures::{StreamExt, pin_mut};
use std::sync::Arc;
use tracing::{debug, info};

/// Encode directory scan entry value: name + DirScanValue
/// This matches the format expected by directory listing
fn encode_dir_scan_value(name: &[u8], value: &DirScanValue) -> Bytes {
    let value_bytes =
        bincode::serialize(value).expect("DirScanValue serialization should not fail");
    let mut buf = Vec::with_capacity(4 + name.len() + value_bytes.len());
    buf.extend_from_slice(&(name.len() as u32).to_le_bytes());
    buf.extend_from_slice(name);
    buf.extend_from_slice(&value_bytes);
    Bytes::from(buf)
}

/// Deep clone directory and all its contents recursively
/// This creates new inodes for all files and subdirectories
/// Data chunks are shared via CAS (COW) but inodes are independent
pub async fn clone_directory_deep(
    db: Arc<EncryptedDb>,
    inode_store: &InodeStore,
    directory_store: &DirectoryStore,
    chunk_store: &ChunkStore,
    source_dir_id: InodeId,
    dest_dir_id: InodeId,
) -> Result<(), FsError> {
    // Get all entries from source directory
    let mut entries: Vec<(Vec<u8>, InodeId, u64)> = vec![];
    let stream = directory_store.list_from(source_dir_id, 0).await?;
    pin_mut!(stream);

    while let Some(result) = stream.next().await {
        let entry = match result {
            Ok(e) => e,
            Err(FsError::InvalidData) => {
                debug!("Skipping corrupted entry in directory {}", source_dir_id);
                continue;
            }
            Err(e) => return Err(e),
        };
        entries.push((entry.name.clone(), entry.inode_id, entry.cookie));
    }

    info!(
        "Deep cloning {} entries from directory {} to {}",
        entries.len(),
        source_dir_id,
        dest_dir_id
    );

    let mut cloned_count = 0;
    let mut skipped_count = 0;
    
    for (name, source_inode_id, _cookie) in entries {
        let name_str = String::from_utf8_lossy(&name);
        
        // Skip . and .. entries
        if name_str == "." || name_str == ".." {
            skipped_count += 1;
            continue;
        }
        
        debug!("Cloning entry '{}' (inode {})", name_str, source_inode_id);
        
        // Get the source inode
        let source_inode = match inode_store.get(source_inode_id).await {
            Ok(inode) => inode,
            Err(e) => {
                debug!("Failed to get inode {} for entry '{}': {}. Skipping.", source_inode_id, name_str, e);
                continue;
            }
        };
        
        // Allocate new inode ID for the clone
        let new_inode_id = inode_store.allocate();
        
        // Clone the inode (COW for data chunks via CAS)
        let cloned_inode = source_inode.clone();
        let is_directory = matches!(cloned_inode, Inode::Directory(_));
        
        // Save the cloned inode
        let inode_key = KeyCodec::inode_key(new_inode_id);
        let inode_bytes = bincode::serialize(&cloned_inode).map_err(|_| FsError::IoError)?;
        db.put_with_options(
            &inode_key,
            &inode_bytes,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions {
                await_durable: false, // Batch writes for performance, flush at end
            },
        )
        .await
        .map_err(|_| FsError::IoError)?;
        
        // Note: With CAS, files automatically share chunks via hash references
        // No need to copy chunk metadata - the cloned inode's chunks vector
        // already points to the same chunk hashes
        
        // Get next cookie for directory entry
        let cookie_key = KeyCodec::dir_cookie_counter_key(dest_dir_id);
        let cookie: u64 = match db.get_bytes(&cookie_key).await {
            Ok(Some(val)) => {
                let bytes: [u8; 8] = val.as_ref().try_into().map_err(|_| FsError::IoError)?;
                u64::from_be_bytes(bytes)
            }
            _ => crate::fs::store::directory::COOKIE_FIRST_ENTRY,
        };
        
        let new_cookie = cookie + 1;
        db.put_with_options(
            &cookie_key,
            &new_cookie.to_be_bytes(),
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions {
                await_durable: false, // Batch writes for performance, flush at end
            },
        )
        .await
        .map_err(|_| FsError::IoError)?;
        
        // Create directory entry in destination
        let entry_key = KeyCodec::dir_entry_key(dest_dir_id, &name);
        let entry_value = KeyCodec::encode_dir_entry(new_inode_id, cookie);
        db.put_with_options(
            &entry_key,
            &entry_value,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions {
                await_durable: false, // Batch writes for performance, flush at end
            },
        )
        .await
        .map_err(|_| FsError::IoError)?;
        
        // Create dir_scan entry with proper encoding (name + DirScanValue)
        let scan_key = KeyCodec::dir_scan_key(dest_dir_id, cookie);
        let scan_value = DirScanValue::Reference {
            inode_id: new_inode_id,
        };
        let scan_value_bytes = encode_dir_scan_value(&name, &scan_value);
        db.put_with_options(
            &scan_key,
            &scan_value_bytes,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions {
                await_durable: false, // Batch writes for performance, flush at end
            },
        )
        .await
        .map_err(|_| FsError::IoError)?;
        
        // If it's a directory, recursively clone its contents
        if is_directory {
            Box::pin(clone_directory_deep(
                db.clone(),
                inode_store,
                directory_store,
                chunk_store,
                source_inode_id,
                new_inode_id,
            ))
            .await?;
        }
        
        cloned_count += 1;
    }

    info!(
        "Completed deep cloning: {} entries cloned, {} skipped from directory {} to {}",
        cloned_count,
        skipped_count,
        source_dir_id,
        dest_dir_id
    );

    Ok(())
}
