use crate::config::Settings;
use crate::rpc::client::RpcClient;
use anyhow::{Context, Result};
use comfy_table::{presets::UTF8_FULL, Table};
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

/// Create a new subvolume
pub async fn create_subvolume(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let subvolume = client.create_subvolume(name).await?;

    println!("✓ Subvolume created successfully!");
    println!("  Name: {}", subvolume.name);
    println!("  ID: {}", subvolume.id);
    println!("  UUID: {}", subvolume.uuid);
    println!("  Created at: {}", format_timestamp(subvolume.created_at));
    println!("  Root inode: {}", subvolume.root_inode);

    Ok(())
}

/// List all subvolumes
pub async fn list_subvolumes(config_path: &Path) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let subvolumes = client.list_subvolumes().await?;

    if subvolumes.is_empty() {
        println!("No subvolumes found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        "ID",
        "Name",
        "UUID",
        "Created At",
        "Type",
        "Readonly",
    ]);

    for subvol in subvolumes {
        let sv_type = if subvol.is_snapshot {
            "Snapshot"
        } else {
            "Subvolume"
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

/// Delete a subvolume
pub async fn delete_subvolume(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    client.delete_subvolume(name).await?;

    println!("✓ Subvolume '{}' deleted successfully!", name);
    Ok(())
}

/// Get subvolume info
pub async fn get_subvolume_info(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let subvolume = client
        .get_subvolume_info(name)
        .await?
        .context("Subvolume not found")?;

    println!("Subvolume Information:");
    println!("  Name: {}", subvolume.name);
    println!("  ID: {}", subvolume.id);
    println!("  UUID: {}", subvolume.uuid);
    println!("  Type: {}", if subvolume.is_snapshot { "Snapshot" } else { "Subvolume" });
    println!("  Readonly: {}", subvolume.is_readonly);
    println!("  Created at: {}", format_timestamp(subvolume.created_at));
    println!("  Root inode: {}", subvolume.root_inode);
    println!("  Generation: {}", subvolume.generation);
    
    if let Some(parent_id) = subvolume.parent_id {
        println!("  Parent ID: {}", parent_id);
    }
    
    if let Some(parent_uuid) = subvolume.parent_uuid {
        println!("  Parent UUID: {}", parent_uuid);
    }

    Ok(())
}

/// Create a snapshot
pub async fn create_snapshot(config_path: &Path, source_name: &str, snapshot_name: &str, readonly: bool) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let snapshot = client.create_snapshot_with_options(source_name, snapshot_name, readonly).await?;

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

    // Get all subvolumes to map IDs to names
    let subvolumes = client.list_subvolumes().await?;
    let mut id_to_name: std::collections::HashMap<u64, String> = subvolumes
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect();

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        "ID",
        "Name",
        "Source",
        "UUID",
        "Created At",
    ]);

    for snapshot in snapshots {
        let source = snapshot.parent_id
            .and_then(|id| id_to_name.get(&id).cloned())
            .unwrap_or_else(|| snapshot.parent_id.map(|id| format!("ID:{}", id)).unwrap_or_else(|| "-".to_string()));
        
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

/// Set default subvolume
pub async fn set_default_subvolume(config_path: &Path, name: &str) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    client.set_default_subvolume(name).await?;

    println!("✓ Default subvolume set to '{}'", name);
    Ok(())
}

/// Get default subvolume
pub async fn get_default_subvolume(config_path: &Path) -> Result<()> {
    let client = connect_rpc_client(config_path).await?;
    let default_id = client.get_default_subvolume().await?;

    println!("Default subvolume ID: {}", default_id);
    Ok(())
}

/// Restore a file from a snapshot (instant COW copy)
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
    let snapshot = client.get_subvolume_info(snapshot_name).await?
        .ok_or_else(|| anyhow::anyhow!("Snapshot '{}' not found", snapshot_name))?;
    
    if !snapshot.is_snapshot {
        anyhow::bail!("'{}' is not a snapshot", snapshot_name);
    }
    
    println!("📸 Restoring from snapshot: {}", snapshot_name);
    println!("   Created: {}", format_timestamp(snapshot.created_at));
    println!("   Source path: {}", source_path);
    println!("   Destination: {}", destination_path);
    println!();
    
    print!("⏳ Reading file from snapshot...");
    std::io::stdout().flush()?;
    
    // Read the file from the snapshot via RPC
    let file_data = client.read_snapshot_file(snapshot_name, source_path).await
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
    
    Ok(())
}

