use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub type DatasetId = u64;

/// Dataset metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    /// Unique dataset ID
    pub id: DatasetId,
    /// Human-readable name
    pub name: String,
    /// UUID for this dataset
    pub uuid: Uuid,
    /// Parent dataset ID (None for root dataset)
    pub parent_id: Option<DatasetId>,
    /// Parent UUID (for snapshots - references source dataset)
    pub parent_uuid: Option<Uuid>,
    /// Root inode ID for this dataset's tree
    pub root_inode: u64,
    /// Creation timestamp (seconds since UNIX epoch)
    pub created_at: u64,
    /// Whether this is a read-only dataset (snapshots are read-only)
    pub is_readonly: bool,
    /// Whether this is a snapshot (vs a regular dataset)
    pub is_snapshot: bool,
    /// Generation number (incremented on each modification)
    pub generation: u64,
    /// Flags for future extensions
    pub flags: u64,
}

impl Dataset {
    pub fn new(
        id: DatasetId,
        name: String,
        root_inode: u64,
        created_at: u64,
        is_readonly: bool,
    ) -> Self {
        Self {
            id,
            name,
            uuid: Uuid::new_v4(),
            parent_id: None,
            parent_uuid: None,
            root_inode,
            created_at,
            is_readonly,
            is_snapshot: false,
            generation: 1,
            flags: 0,
        }
    }

    pub fn new_snapshot(
        id: DatasetId,
        name: String,
        source: &Dataset,
        root_inode: u64,
        created_at: u64,
        is_readonly: bool,
    ) -> Self {
        Self {
            id,
            name,
            uuid: Uuid::new_v4(),
            parent_id: Some(source.id),
            parent_uuid: Some(source.uuid),
            root_inode,
            created_at,
            is_readonly, // Snapshots can be read-write (like btrfs)
            is_snapshot: true,
            generation: source.generation,
            flags: 0,
        }
    }
}

/// Dataset tree entry - links inode to dataset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetInodeMapping {
    pub inode_id: u64,
    pub dataset_id: DatasetId,
}

/// Dataset registry - maintains all datasets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRegistry {
    /// Next dataset ID to allocate
    pub next_id: DatasetId,
    /// Map of dataset ID to metadata
    pub datasets: HashMap<DatasetId, Dataset>,
    /// Map of dataset name to ID
    pub name_to_id: HashMap<String, DatasetId>,
    /// Default dataset (mounted by default)
    pub default_dataset_id: DatasetId,
}

impl DatasetRegistry {
    pub fn new_with_root(root_inode: u64, created_at: u64) -> Self {
        let root_subvol = Dataset::new(0, "root".to_string(), root_inode, created_at, false);
        let mut datasets = HashMap::new();
        let mut name_to_id = HashMap::new();
        
        datasets.insert(0, root_subvol);
        name_to_id.insert("root".to_string(), 0);

        Self {
            next_id: 1,
            datasets,
            name_to_id,
            default_dataset_id: 0,
        }
    }

    pub fn allocate_id(&mut self) -> DatasetId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_dataset(&mut self, dataset: Dataset) -> Result<(), String> {
        if self.name_to_id.contains_key(&dataset.name) {
            return Err(format!("Dataset '{}' already exists", dataset.name));
        }
        
        let id = dataset.id;
        let name = dataset.name.clone();
        
        self.datasets.insert(id, dataset);
        self.name_to_id.insert(name, id);
        
        Ok(())
    }

    pub fn get_by_id(&self, id: DatasetId) -> Option<&Dataset> {
        self.datasets.get(&id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&Dataset> {
        self.name_to_id.get(name).and_then(|id| self.datasets.get(id))
    }

    pub fn remove_dataset(&mut self, id: DatasetId) -> Result<Dataset, String> {
        // Don't allow removing root dataset
        if id == 0 {
            return Err("Cannot remove root dataset".to_string());
        }

        let subvol = self.datasets.remove(&id)
            .ok_or_else(|| format!("Dataset {} not found", id))?;
        
        self.name_to_id.remove(&subvol.name);
        
        Ok(subvol)
    }

    pub fn list_datasets(&self) -> Vec<&Dataset> {
        let mut subvols: Vec<_> = self.datasets.values().collect();
        subvols.sort_by_key(|s| s.id);
        subvols
    }

    pub fn list_snapshots(&self) -> Vec<&Dataset> {
        let mut snapshots: Vec<_> = self.datasets.values()
            .filter(|s| s.is_snapshot)
            .collect();
        snapshots.sort_by_key(|s| s.created_at);
        snapshots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dataset_registry() {
        let mut registry = DatasetRegistry::new_with_root(0, 1000);
        
        assert_eq!(registry.datasets.len(), 1);
        assert_eq!(registry.default_dataset_id, 0);
        
        // Add a new dataset
        let id = registry.allocate_id();
        let subvol = Dataset::new(id, "data".to_string(), 100, 2000, false);
        assert!(registry.add_dataset(subvol).is_ok());
        
        // Verify we can retrieve it
        assert!(registry.get_by_name("data").is_some());
        assert!(registry.get_by_id(1).is_some());
        
        // Test duplicate name
        let dup = Dataset::new(2, "data".to_string(), 200, 3000, false);
        assert!(registry.add_dataset(dup).is_err());
    }

    #[test]
    fn test_snapshot_creation() {
        let source = Dataset::new(1, "source".to_string(), 100, 1000, false);
        let snapshot = Dataset::new_snapshot(2, "snap1".to_string(), &source, 200, 2000);
        
        assert!(snapshot.is_snapshot);
        assert!(snapshot.is_readonly);
        assert_eq!(snapshot.parent_id, Some(1));
        assert_eq!(snapshot.parent_uuid, Some(source.uuid));
        assert_eq!(snapshot.generation, source.generation);
    }
}

