use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub type SubvolumeId = u64;

/// Subvolume metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subvolume {
    /// Unique subvolume ID
    pub id: SubvolumeId,
    /// Human-readable name
    pub name: String,
    /// UUID for this subvolume
    pub uuid: Uuid,
    /// Parent subvolume ID (None for root subvolume)
    pub parent_id: Option<SubvolumeId>,
    /// Parent UUID (for snapshots - references source subvolume)
    pub parent_uuid: Option<Uuid>,
    /// Root inode ID for this subvolume's tree
    pub root_inode: u64,
    /// Creation timestamp (seconds since UNIX epoch)
    pub created_at: u64,
    /// Whether this is a read-only subvolume (snapshots are read-only)
    pub is_readonly: bool,
    /// Whether this is a snapshot (vs a regular subvolume)
    pub is_snapshot: bool,
    /// Generation number (incremented on each modification)
    pub generation: u64,
    /// Flags for future extensions
    pub flags: u64,
}

impl Subvolume {
    pub fn new(
        id: SubvolumeId,
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
        id: SubvolumeId,
        name: String,
        source: &Subvolume,
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

/// Subvolume tree entry - links inode to subvolume
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubvolumeInodeMapping {
    pub inode_id: u64,
    pub subvolume_id: SubvolumeId,
}

/// Subvolume registry - maintains all subvolumes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubvolumeRegistry {
    /// Next subvolume ID to allocate
    pub next_id: SubvolumeId,
    /// Map of subvolume ID to metadata
    pub subvolumes: HashMap<SubvolumeId, Subvolume>,
    /// Map of subvolume name to ID
    pub name_to_id: HashMap<String, SubvolumeId>,
    /// Default subvolume (mounted by default)
    pub default_subvolume_id: SubvolumeId,
}

impl SubvolumeRegistry {
    pub fn new_with_root(root_inode: u64, created_at: u64) -> Self {
        let root_subvol = Subvolume::new(0, "root".to_string(), root_inode, created_at, false);
        let mut subvolumes = HashMap::new();
        let mut name_to_id = HashMap::new();
        
        subvolumes.insert(0, root_subvol);
        name_to_id.insert("root".to_string(), 0);

        Self {
            next_id: 1,
            subvolumes,
            name_to_id,
            default_subvolume_id: 0,
        }
    }

    pub fn allocate_id(&mut self) -> SubvolumeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_subvolume(&mut self, subvolume: Subvolume) -> Result<(), String> {
        if self.name_to_id.contains_key(&subvolume.name) {
            return Err(format!("Subvolume '{}' already exists", subvolume.name));
        }
        
        let id = subvolume.id;
        let name = subvolume.name.clone();
        
        self.subvolumes.insert(id, subvolume);
        self.name_to_id.insert(name, id);
        
        Ok(())
    }

    pub fn get_by_id(&self, id: SubvolumeId) -> Option<&Subvolume> {
        self.subvolumes.get(&id)
    }

    pub fn get_by_name(&self, name: &str) -> Option<&Subvolume> {
        self.name_to_id.get(name).and_then(|id| self.subvolumes.get(id))
    }

    pub fn remove_subvolume(&mut self, id: SubvolumeId) -> Result<Subvolume, String> {
        // Don't allow removing root subvolume
        if id == 0 {
            return Err("Cannot remove root subvolume".to_string());
        }

        let subvol = self.subvolumes.remove(&id)
            .ok_or_else(|| format!("Subvolume {} not found", id))?;
        
        self.name_to_id.remove(&subvol.name);
        
        Ok(subvol)
    }

    pub fn list_subvolumes(&self) -> Vec<&Subvolume> {
        let mut subvols: Vec<_> = self.subvolumes.values().collect();
        subvols.sort_by_key(|s| s.id);
        subvols
    }

    pub fn list_snapshots(&self) -> Vec<&Subvolume> {
        let mut snapshots: Vec<_> = self.subvolumes.values()
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
    fn test_subvolume_registry() {
        let mut registry = SubvolumeRegistry::new_with_root(0, 1000);
        
        assert_eq!(registry.subvolumes.len(), 1);
        assert_eq!(registry.default_subvolume_id, 0);
        
        // Add a new subvolume
        let id = registry.allocate_id();
        let subvol = Subvolume::new(id, "data".to_string(), 100, 2000, false);
        assert!(registry.add_subvolume(subvol).is_ok());
        
        // Verify we can retrieve it
        assert!(registry.get_by_name("data").is_some());
        assert!(registry.get_by_id(1).is_some());
        
        // Test duplicate name
        let dup = Subvolume::new(2, "data".to_string(), 200, 3000, false);
        assert!(registry.add_subvolume(dup).is_err());
    }

    #[test]
    fn test_snapshot_creation() {
        let source = Subvolume::new(1, "source".to_string(), 100, 1000, false);
        let snapshot = Subvolume::new_snapshot(2, "snap1".to_string(), &source, 200, 2000);
        
        assert!(snapshot.is_snapshot);
        assert!(snapshot.is_readonly);
        assert_eq!(snapshot.parent_id, Some(1));
        assert_eq!(snapshot.parent_uuid, Some(source.uuid));
        assert_eq!(snapshot.generation, source.generation);
    }
}

