use crate::config::Settings;
use crate::rpc::client::RpcClient;
use anyhow::{Context, Result};
use comfy_table::{Table, presets::UTF8_FULL};
use std::io::Write;
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

/// Create a snapshot (implemented as a COW clone)
pub async fn create_snapshot(
    config_path: &Path,
    source_path: &str,
    snapshot_name: &str,
    _readonly: bool, // Ignored - all clones are independent
) -> Result<()> {
    // Snapshot is just a clone to /snapshots/<name>
    let dest_path = format!("/snapshots/{}", snapshot_name);
    
    println!("📸 Creating snapshot (COW clone)...");
    println!("   Source: {}", source_path);
    println!("   Destination: {}", dest_path);
    println!();
    
    // Call clone
    clone_path(config_path, source_path, &dest_path).await?;
    
    println!();
    println!("✅ Snapshot created!");
    println!("   Location: {}", dest_path);
    println!("   Type: Independent COW clone");
    println!();
    println!("💡 Tip: Snapshots are just directories");
    println!("   View: ls {}", dest_path);
    println!("   Restore: cp -r {} {}", dest_path, source_path);
    
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

// Removed: is_internal_zerofs_path() - no longer needed

/// Restore is deprecated - just use clone or copy the directory
pub async fn restore_from_snapshot(
    _config_path: &Path,
    snapshot_name: &str,
    source_path: &str,
    destination_path: &str,
) -> Result<()> {
    println!("❌ 'restore' command is deprecated!");
    println!();
    println!("Snapshots are just directories now. To restore:");
    println!();
    println!("Option 1: Copy the directory (filesystem operations)");
    println!("  cp -r /snapshots/{}/{} {}", snapshot_name, source_path, destination_path);
    println!();
    println!("Option 2: Use clone command (COW, instant)");
    println!("  zerofs dataset clone --source /snapshots/{}/{} --destination {}", 
        snapshot_name, source_path, destination_path);
    println!();
    println!("💡 Tip: Snapshots are independent copies, not special entities.");
    println!("   Just treat them as regular directories!");
    
    anyhow::bail!("Use 'clone' or 'cp' instead of 'restore'")
}

/// Clone a path (COW, instant copy)
pub async fn clone_path(
    config_path: &Path,
    source_path: &str,
    destination_path: &str,
) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    
    println!("🔄 Cloning with COW (Copy-on-Write)");
    println!("   Source: {}", source_path);
    println!("   Destination: {}", destination_path);
    println!();
    
    print!("⏳ Creating COW clone...");
    std::io::stdout().flush()?;
    
    let (inode_id, size, is_directory) = client
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
    println!("   Type: {}", if is_directory { "Directory" } else { "File" });
    println!("   Inode: {}", inode_id);
    println!("   Size: {}", format_size(size));
    println!("   ⚡ COW: Data shared until modified (zero copy)");
    println!();
    println!("Note: Source and destination are now independent.");
    println!("      Modifications to either won't affect the other.");
    
    Ok(())
}
