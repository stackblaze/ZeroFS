use crate::config::Settings;
use crate::rpc::client::RpcClient;
use anyhow::{Context, Result};
use comfy_table::{Table, presets::UTF8_FULL};
use std::path::Path;

async fn connect_rpc_client(config_path: &Path) -> Result<RpcClient> {
    let settings = Settings::from_file(config_path)
        .with_context(|| format!("Failed to load config from {}", config_path.display()))?;

    let rpc_config = settings
        .servers
        .rpc
        .as_ref()
        .context("RPC server not configured in config file")?;

    RpcClient::connect_from_config(rpc_config)
        .await
        .context("Failed to connect to RPC server. Is the server running?")
}

fn format_timestamp(timestamp: u64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp as i64, 0)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
    dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    if unit_idx == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.2} {}", size, UNITS[unit_idx])
    }
}

/// Create a new dataset
pub async fn create_dataset(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let dataset = client.create_dataset(name).await?;

    println!("✓ Dataset created successfully!");
    println!("  Name: {}", dataset.name);
    println!("  ID: {}", dataset.id);
    println!("  UUID: {}", dataset.uuid);
    println!("  Created at: {}", format_timestamp(dataset.created_at));
    println!("  Root inode: {}", dataset.root_inode);

    Ok(())
}

/// List all datasets
pub async fn list_datasets(config_path: &Path) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let datasets = client.list_datasets().await?;

    if datasets.is_empty() {
        println!("No datasets found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["ID", "Name", "UUID", "Created At", "Type", "Readonly"]);

    for subvol in datasets {
        let sv_type = if subvol.is_snapshot {
            "Snapshot"
        } else {
            "Dataset"
        };

        table.add_row(vec![
            subvol.id.to_string(),
            subvol.name,
            subvol.uuid.to_string(),
            format_timestamp(subvol.created_at),
            sv_type.to_string(),
            subvol.is_readonly.to_string(),
        ]);
    }

    println!("{table}");
    Ok(())
}

/// Delete a dataset
pub async fn delete_dataset(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    client.delete_dataset(name).await?;

    println!("✓ Dataset '{}' deleted successfully!", name);
    Ok(())
}

/// Get dataset info
pub async fn get_dataset_info(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let dataset = client
        .get_dataset_info(name)
        .await?
        .context("Dataset not found")?;

    println!("Dataset Information:");
    println!("  Name: {}", dataset.name);
    println!("  ID: {}", dataset.id);
    println!("  UUID: {}", dataset.uuid);
    println!(
        "  Type: {}",
        if dataset.is_snapshot {
            "Snapshot"
        } else {
            "Dataset"
        }
    );
    println!("  Readonly: {}", dataset.is_readonly);
    println!("  Created at: {}", format_timestamp(dataset.created_at));
    println!("  Root inode: {}", dataset.root_inode);
    println!("  Generation: {}", dataset.generation);

    if let Some(parent_id) = dataset.parent_id {
        println!("  Parent ID: {}", parent_id);
    }

    if let Some(parent_uuid) = dataset.parent_uuid {
        println!("  Parent UUID: {}", parent_uuid);
    }

    Ok(())
}

/// Create a snapshot
pub async fn create_snapshot(
    config_path: &Path,
    source_name: &str,
    snapshot_name: &str,
    readonly: bool,
) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let snapshot = client
        .create_snapshot_with_options(source_name, snapshot_name, readonly)
        .await?;

    println!("✓ Snapshot created successfully!");
    println!("  Name: {}", snapshot.name);
    println!("  ID: {}", snapshot.id);
    println!("  UUID: {}", snapshot.uuid);
    println!("  Source: {}", source_name);
    println!("  Created at: {}", format_timestamp(snapshot.created_at));

    if let Some(parent_uuid) = snapshot.parent_uuid {
        println!("  Parent UUID: {}", parent_uuid);
    }

    Ok(())
}

/// List all snapshots
pub async fn list_snapshots(config_path: &Path) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let snapshots = client.list_snapshots().await?;

    if snapshots.is_empty() {
        println!("No snapshots found.");
        return Ok(());
    }

    // Get all datasets to map IDs to names
    let datasets = client.list_datasets().await?;
    let id_to_name: std::collections::HashMap<u64, String> =
        datasets.into_iter().map(|s| (s.id, s.name)).collect();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec!["ID", "Name", "Source", "UUID", "Created At"]);

    for snapshot in snapshots {
        let source = snapshot
            .parent_id
            .and_then(|id| id_to_name.get(&id).cloned())
            .unwrap_or_else(|| {
                snapshot
                    .parent_id
                    .map(|id| format!("ID:{}", id))
                    .unwrap_or_else(|| "-".to_string())
            });

        table.add_row(vec![
            snapshot.id.to_string(),
            snapshot.name,
            source,
            snapshot.uuid.to_string(),
            format_timestamp(snapshot.created_at),
        ]);
    }

    println!("{table}");
    Ok(())
}

/// Delete a snapshot
pub async fn delete_snapshot(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    client.delete_snapshot(name).await?;

    println!("✓ Snapshot '{}' deleted successfully!", name);
    Ok(())
}

/// Set default dataset
pub async fn set_default_dataset(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    client.set_default_dataset(name).await?;

    println!("✓ Default dataset set to '{}'", name);
    Ok(())
}

/// Get default dataset
pub async fn get_default_dataset(config_path: &Path) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let default_id = client.get_default_dataset().await?;

    println!("Default dataset ID: {}", default_id);
    Ok(())
}

/// Check if a path is within a ZeroFS mount point
/// For internal restore (within ZeroFS filesystem), paths should NOT include external mount points
/// Examples of INTERNAL paths (use instant restore):
///   - /file.txt                                  (root of ZeroFS filesystem)
///   - /data/file.txt                            (subdirectory in ZeroFS)
///   - /var/lib/kubelet/pods/.../file.txt        (Kubernetes CSI volume path - internal to ZeroFS)
/// Examples of EXTERNAL paths (use copy-based restore):
///   - /tmp/file.txt                             (outside ZeroFS, on local filesystem)
///   - /home/user/file.txt                       (outside ZeroFS, on local filesystem)
fn is_internal_zerofs_path(destination_path: &str) -> bool {

    // For instant restore to work, the destination must be:
    // 1. An absolute path starting with /
    // 2. NOT a path on the local filesystem outside ZeroFS

    if !destination_path.starts_with('/') {
        return false;
    }

    // Paths that are definitely EXTERNAL (local filesystem):
    let external_prefixes = [
        "/tmp/", "/home/", "/root/", "/opt/", "/usr/", "/etc/", "/boot/", "/sys/", "/proc/",
        "/dev/",
    ];

    for prefix in &external_prefixes {
        if destination_path.starts_with(prefix) {
            return false; // External path, use copy-based restore
        }
    }

    // All other absolute paths are considered internal to ZeroFS
    // This includes:
    // - /file.txt (root of ZeroFS)
    // - /data/file.txt (ZeroFS subdirectories)
    // - /mnt/... (if ZeroFS is mounted at /mnt)
    // - /var/lib/kubelet/... (Kubernetes CSI volumes)
    true
}

/// Restore a file from a snapshot (instant COW copy when destination is within ZeroFS)
pub async fn restore_from_snapshot(
    config_path: &Path,
    snapshot_name: &str,
    source_path: &str,
    destination_path: &str,
) -> Result<()> {
    use std::fs;
    use std::io::Write;

    let client = connect_rpc_client(config_path).await?;

    // Get snapshot info to verify it exists
    let snapshot = client
        .get_dataset_info(snapshot_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Snapshot '{}' not found", snapshot_name))?;

    if !snapshot.is_snapshot {
        anyhow::bail!("'{}' is not a snapshot", snapshot_name);
    }

    println!("📸 Restoring from snapshot: {}", snapshot_name);
    println!("   Created: {}", format_timestamp(snapshot.created_at));
    println!("   Source path: {}", source_path);
    println!("   Destination: {}", destination_path);
    println!();

    // Check if destination is internal to ZeroFS filesystem
    // For Kubernetes CSI, paths will be absolute paths like /var/lib/kubelet/pods/.../volumes/...
    // For direct use, paths will be like /file.txt or /data/file.txt (relative to ZeroFS root)
    let use_instant_restore = is_internal_zerofs_path(destination_path);

    if use_instant_restore {
        // INSTANT RESTORE: Create directory entry pointing to snapshot inode (COW)
        print!("⚡ Instant restore (COW - no data copying)...");
        std::io::stdout().flush()?;

        let (inode_id, file_size, nlink) = client
            .instant_restore_file(snapshot_name, source_path, destination_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to instant restore file '{}' from snapshot",
                    source_path
                )
            })?;

        println!(" done!");
        println!();
        println!("✅ File restored instantly (COW)!");
        println!("   Inode: {}", inode_id);
        println!("   Size: {}", format_size(file_size));
        println!("   Links: {} (shared with snapshot)", nlink);
        println!("   ⚡ No data copied - instant restore!");
    } else {
        // COPY-BASED RESTORE: For external destinations (outside ZeroFS)
        print!("⏳ Reading file from snapshot...");
        std::io::stdout().flush()?;

        // Read the file from the snapshot via RPC
        let file_data = client
            .read_snapshot_file(snapshot_name, source_path)
            .await
            .with_context(|| format!("Failed to read file '{}' from snapshot", source_path))?;

        println!(" done! ({} bytes)", file_data.len());

        print!("⏳ Writing to destination...");
        std::io::stdout().flush()?;

        // Write to destination
        fs::write(destination_path, &file_data)
            .with_context(|| format!("Failed to write to destination '{}'", destination_path))?;

        println!(" done!");
        println!();
        println!("✅ File restored successfully!");
        println!("   Size: {}", format_size(file_data.len() as u64));
        println!("   Note: Data copied (destination outside ZeroFS)");
    }

    Ok(())
}

/// Clone a file or directory using COW (Copy-on-Write)
/// This creates an instant copy with no data duplication until modified
pub async fn clone_path(
    config_path: &Path,
    source_path: &str,
    destination_path: &str,
) -> Result<()> {
    use std::io::Write;

    let mut client = connect_rpc_client(config_path).await?;

    println!("🔄 Cloning with COW (Copy-on-Write)");
    println!("   Source: {}", source_path);
    println!("   Destination: {}", destination_path);
    println!();

    print!("⏳ Creating COW clone...");
    std::io::stdout().flush()?;

    let (inode_id, size, is_dir) = client
        .clone_path(source_path, destination_path)
        .await
        .with_context(|| {
            format!(
                "Failed to clone '{}' to '{}'",
                source_path, destination_path
            )
        })?;

    println!(" done!");
    println!();
    println!("✅ Clone created successfully!");
    println!("   Type: {}", if is_dir { "Directory" } else { "File" });
    println!("   Inode: {}", inode_id);
    if !is_dir {
        println!("   Size: {}", format_size(size));
    }
    println!("   ⚡ COW: Data shared until modified (zero copy)");
    println!();
    println!("Note: Source and destination are now independent.");
    println!("      Modifications to either won't affect the other.");

    Ok(())
}
