use crate::encryption::EncryptedDb;
use crate::fs::errors::FsError;
use crate::fs::key_codec::KeyCodec;
use crate::fs::subvolume::{Subvolume, SubvolumeId, SubvolumeRegistry};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct SubvolumeStore {
    db: Arc<EncryptedDb>,
    registry: Arc<RwLock<SubvolumeRegistry>>,
}

impl SubvolumeStore {
    pub async fn new(db: Arc<EncryptedDb>, root_inode: u64, created_at: u64) -> Result<Self, FsError> {
        let registry_key = KeyCodec::subvolume_registry_key();
        
        let registry = match db.get_bytes(&registry_key).await.map_err(|_| FsError::IoError)? {
            Some(data) => {
                bincode::deserialize(&data).map_err(|e| {
                    tracing::warn!("Failed to deserialize subvolume registry: {:?}", e);
                    FsError::InvalidData
                })?
            }
            None => {
                // Initialize with root subvolume if not exists
                if !db.is_read_only() {
                    let registry = SubvolumeRegistry::new_with_root(root_inode, created_at);
                    
                    // Persist the registry
                    let serialized = bincode::serialize(&registry).map_err(|_| FsError::IoError)?;
                    db.put_with_options(&registry_key, &serialized, &slatedb::config::PutOptions::default(), &slatedb::config::WriteOptions { await_durable: false })
                        .await
                        .map_err(|_| FsError::IoError)?;
                    
                    registry
                } else {
                    return Err(FsError::IoError);
                }
            }
        };

        Ok(Self {
            db,
            registry: Arc::new(RwLock::new(registry)),
        })
    }

    /// Get the current registry (for reading)
    pub async fn get_registry(&self) -> SubvolumeRegistry {
        self.registry.read().await.clone()
    }

    /// Create a new subvolume
    pub async fn create_subvolume(
        &self,
        name: String,
        root_inode: u64,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Subvolume, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;
        
        let id = registry.allocate_id();
        let subvolume = Subvolume::new(id, name, root_inode, created_at, is_readonly);
        
        registry.add_subvolume(subvolume.clone()).map_err(|e| {
            tracing::warn!("Failed to add subvolume to registry: {}", e);
            FsError::Exists
        })?;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(subvolume)
    }

    /// Create a snapshot from an existing subvolume
    pub async fn create_snapshot(
        &self,
        source_id: SubvolumeId,
        snapshot_name: String,
        snapshot_root_inode: u64,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Subvolume, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;
        
        let source = registry.get_by_id(source_id)
            .ok_or(FsError::NotFound)?
            .clone();
        
        let id = registry.allocate_id();
        let snapshot = Subvolume::new_snapshot(id, snapshot_name, &source, snapshot_root_inode, created_at, is_readonly);
        
        registry.add_subvolume(snapshot.clone()).map_err(|e| {
            tracing::warn!("Failed to add snapshot to registry: {}", e);
            FsError::Exists
        })?;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(snapshot)
    }

    /// Delete a subvolume or snapshot
    pub async fn delete_subvolume(&self, id: SubvolumeId) -> Result<Subvolume, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;
        
        let subvolume = registry.remove_subvolume(id).map_err(|e| {
            tracing::warn!("Failed to remove subvolume: {}", e);
            FsError::NotFound
        })?;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(subvolume)
    }

    /// Get subvolume by ID
    pub async fn get_by_id(&self, id: SubvolumeId) -> Option<Subvolume> {
        let registry = self.registry.read().await;
        registry.get_by_id(id).cloned()
    }

    /// Get subvolume by name
    pub async fn get_by_name(&self, name: &str) -> Option<Subvolume> {
        let registry = self.registry.read().await;
        registry.get_by_name(name).cloned()
    }

    /// List all subvolumes
    pub async fn list_subvolumes(&self) -> Vec<Subvolume> {
        let registry = self.registry.read().await;
        registry.list_subvolumes().into_iter().cloned().collect()
    }

    /// List all snapshots
    pub async fn list_snapshots(&self) -> Vec<Subvolume> {
        let registry = self.registry.read().await;
        registry.list_snapshots().into_iter().cloned().collect()
    }

    /// Set the default subvolume
    pub async fn set_default(&self, id: SubvolumeId) -> Result<(), FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;
        
        // Verify the subvolume exists
        if registry.get_by_id(id).is_none() {
            return Err(FsError::NotFound);
        }

        registry.default_subvolume_id = id;
        
        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(())
    }

    /// Get the default subvolume
    pub async fn get_default(&self) -> SubvolumeId {
        let registry = self.registry.read().await;
        registry.default_subvolume_id
    }

    /// Persist the registry to the database
    async fn persist_registry(&self, registry: &SubvolumeRegistry) -> Result<(), FsError> {
        let registry_key = KeyCodec::subvolume_registry_key();
        let serialized = bincode::serialize(registry).map_err(|e| {
            tracing::error!("Failed to serialize subvolume registry: {:?}", e);
            FsError::IoError
        })?;
        
        self.db.put_with_options(
            &registry_key,
            &serialized,
            &slatedb::config::PutOptions::default(),
            &slatedb::config::WriteOptions { await_durable: false }
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to persist subvolume registry: {:?}", e);
            FsError::IoError
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::ZeroFS;

    #[tokio::test]
    async fn test_subvolume_store() {
        let encryption_key = [0u8; 32];
        let fs = ZeroFS::new_in_memory_with_encryption(encryption_key)
            .await
            .unwrap();
        
        let store = SubvolumeStore::new(fs.db.clone(), 0, 1000)
            .await
            .unwrap();

        // Should have root subvolume
        let registry = store.get_registry().await;
        assert_eq!(registry.subvolumes.len(), 1);

        // Create a new subvolume
        let subvol = store.create_subvolume("data".to_string(), 100, 2000, false)
            .await
            .unwrap();
        
        assert_eq!(subvol.name, "data");
        assert_eq!(subvol.root_inode, 100);
        assert!(!subvol.is_readonly);
        assert!(!subvol.is_snapshot);

        // Retrieve by name
        let found = store.get_by_name("data").await.unwrap();
        assert_eq!(found.id, subvol.id);

        // Create a snapshot
        let snapshot = store.create_snapshot(subvol.id, "snap1".to_string(), 200, 3000)
            .await
            .unwrap();
        
        assert!(snapshot.is_snapshot);
        assert!(snapshot.is_readonly);
        assert_eq!(snapshot.parent_id, Some(subvol.id));

        // List snapshots
        let snapshots = store.list_snapshots().await;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].name, "snap1");
    }
}

