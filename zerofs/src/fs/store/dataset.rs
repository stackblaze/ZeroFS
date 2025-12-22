use crate::encryption::EncryptedDb;
use crate::fs::dataset::{Dataset, DatasetId, DatasetRegistry};
use crate::fs::errors::FsError;
use crate::fs::key_codec::KeyCodec;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct DatasetStore {
    db: Arc<EncryptedDb>,
    registry: Arc<RwLock<DatasetRegistry>>,
}

impl DatasetStore {
    pub async fn new(
        db: Arc<EncryptedDb>,
        root_inode: u64,
        created_at: u64,
    ) -> Result<Self, FsError> {
        let registry_key = KeyCodec::dataset_registry_key();

        let registry = match db
            .get_bytes(&registry_key)
            .await
            .map_err(|_| FsError::IoError)?
        {
            Some(data) => bincode::deserialize(&data).map_err(|e| {
                tracing::warn!("Failed to deserialize dataset registry: {:?}", e);
                FsError::InvalidData
            })?,
            None => {
                // Initialize with root dataset if not exists
                if !db.is_read_only() {
                    let registry = DatasetRegistry::new_with_root(root_inode, created_at);

                    // Persist the registry
                    let serialized = bincode::serialize(&registry).map_err(|_| FsError::IoError)?;
                    db.put_with_options(
                        &registry_key,
                        &serialized,
                        &slatedb::config::PutOptions::default(),
                        &slatedb::config::WriteOptions {
                            await_durable: false,
                        },
                    )
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
    pub async fn get_registry(&self) -> DatasetRegistry {
        self.registry.read().await.clone()
    }

    /// Create a new dataset
    pub async fn create_dataset(
        &self,
        name: String,
        root_inode: u64,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Dataset, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;

        let id = registry.allocate_id();
        let dataset = Dataset::new(id, name, root_inode, created_at, is_readonly);

        registry.add_dataset(dataset.clone()).map_err(|e| {
            tracing::warn!("Failed to add dataset to registry: {}", e);
            FsError::Exists
        })?;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(dataset)
    }

    /// Create a snapshot from an existing dataset
    pub async fn create_snapshot(
        &self,
        source_id: DatasetId,
        snapshot_name: String,
        snapshot_root_inode: u64,
        created_at: u64,
        is_readonly: bool,
    ) -> Result<Dataset, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;

        let source = registry
            .get_by_id(source_id)
            .ok_or(FsError::NotFound)?
            .clone();

        let id = registry.allocate_id();
        let snapshot = Dataset::new_snapshot(
            id,
            snapshot_name,
            &source,
            snapshot_root_inode,
            created_at,
            is_readonly,
        );

        registry.add_dataset(snapshot.clone()).map_err(|e| {
            tracing::warn!("Failed to add snapshot to registry: {}", e);
            FsError::Exists
        })?;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(snapshot)
    }

    /// Delete a dataset or snapshot
    pub async fn delete_dataset(&self, id: DatasetId) -> Result<Dataset, FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;

        let dataset = registry.remove_dataset(id).map_err(|e| {
            tracing::warn!("Failed to remove dataset: {}", e);
            FsError::NotFound
        })?;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(dataset)
    }

    /// Get dataset by ID
    pub async fn get_by_id(&self, id: DatasetId) -> Option<Dataset> {
        let registry = self.registry.read().await;
        registry.get_by_id(id).cloned()
    }

    /// Get dataset by name
    pub async fn get_by_name(&self, name: &str) -> Option<Dataset> {
        let registry = self.registry.read().await;
        registry.get_by_name(name).cloned()
    }

    /// List all datasets
    pub async fn list_datasets(&self) -> Vec<Dataset> {
        let registry = self.registry.read().await;
        registry.list_datasets().into_iter().cloned().collect()
    }

    /// List all snapshots
    pub async fn list_snapshots(&self) -> Vec<Dataset> {
        let registry = self.registry.read().await;
        registry.list_snapshots().into_iter().cloned().collect()
    }

    /// Set the default dataset
    pub async fn set_default(&self, id: DatasetId) -> Result<(), FsError> {
        if self.db.is_read_only() {
            return Err(FsError::ReadOnlyFilesystem);
        }

        let mut registry = self.registry.write().await;

        // Verify the dataset exists
        if registry.get_by_id(id).is_none() {
            return Err(FsError::NotFound);
        }

        registry.default_dataset_id = id;

        // Persist the registry
        self.persist_registry(&registry).await?;

        Ok(())
    }

    /// Get the default dataset
    pub async fn get_default(&self) -> DatasetId {
        let registry = self.registry.read().await;
        registry.default_dataset_id
    }

    /// Persist the registry to the database
    async fn persist_registry(&self, registry: &DatasetRegistry) -> Result<(), FsError> {
        let registry_key = KeyCodec::dataset_registry_key();
        let serialized = bincode::serialize(registry).map_err(|e| {
            tracing::error!("Failed to serialize dataset registry: {:?}", e);
            FsError::IoError
        })?;

        self.db
            .put_with_options(
                &registry_key,
                &serialized,
                &slatedb::config::PutOptions::default(),
                &slatedb::config::WriteOptions {
                    await_durable: false,
                },
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to persist dataset registry: {:?}", e);
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
    async fn test_dataset_store() {
        let encryption_key = [0u8; 32];
        let fs = ZeroFS::new_in_memory_with_encryption(encryption_key)
            .await
            .unwrap();

        let store = DatasetStore::new(fs.db.clone(), 0, 1000).await.unwrap();

        // Should have root dataset
        let registry = store.get_registry().await;
        assert_eq!(registry.datasets.len(), 1);

        // Create a new dataset
        let subvol = store
            .create_dataset("data".to_string(), 100, 2000, false)
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
        let snapshot = store
            .create_snapshot(subvol.id, "snap1".to_string(), 200, 3000)
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
