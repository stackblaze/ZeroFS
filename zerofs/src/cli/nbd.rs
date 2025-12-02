use crate::config::Settings;
use crate::fs::permissions::Credentials;
use crate::fs::types::{AuthContext, SetAttributes, SetGid, SetMode, SetUid};
use crate::fs::{CacheConfig, ZeroFS};
use crate::key_management;
use crate::parse_object_store::parse_url_opts;
use anyhow::{Context, Result};
use comfy_table::{Cell, Color, Table};
use num_format::{Locale, ToFormattedString};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

async fn init_filesystem(config: &PathBuf) -> Result<Arc<ZeroFS>> {
    let settings = Settings::from_file(config.to_str().unwrap())
        .with_context(|| format!("Failed to load config from {}", config.display()))?;

    let url = settings.storage.url.clone();

    let cache_config = CacheConfig {
        root_folder: settings.cache.dir.to_str().unwrap().to_string(),
        max_cache_size_gb: settings.cache.disk_size_gb,
        memory_cache_size_gb: settings.cache.memory_size_gb,
    };

    let env_vars = settings.cloud_provider_env_vars();
    let (object_store, path_from_url) = parse_url_opts(&url.parse()?, env_vars.into_iter())?;
    let object_store: Arc<dyn object_store::ObjectStore> = Arc::from(object_store);

    let actual_db_path = path_from_url.to_string();

    let bucket =
        crate::bucket_identity::BucketIdentity::get_or_create(&object_store, &actual_db_path)
            .await?;

    let cache_config = CacheConfig {
        root_folder: format!(
            "{}/{}",
            cache_config.root_folder,
            bucket.cache_directory_name()
        ),
        ..cache_config
    };

    let password = settings.storage.encryption_password.clone();

    crate::cli::password::validate_password(&password)
        .map_err(|e| anyhow::anyhow!("Password validation failed: {}", e))?;

    let (slatedb, _) = crate::cli::server::build_slatedb(
        object_store,
        &cache_config,
        actual_db_path,
        crate::cli::server::DatabaseMode::ReadWrite,
        settings.lsm,
    )
    .await?;

    let encryption_key = key_management::load_or_init_encryption_key(&slatedb, &password).await?;

    let max_bytes = settings
        .filesystem
        .as_ref()
        .map(|fs_config| fs_config.max_bytes())
        .unwrap_or(crate::config::FilesystemConfig::DEFAULT_MAX_BYTES);

    let fs = ZeroFS::new_with_slatedb(slatedb, encryption_key, max_bytes).await?;

    Ok(Arc::new(fs))
}

pub async fn create_device(config: PathBuf, name: String, size: String) -> Result<()> {
    let fs = init_filesystem(&config).await?;

    let size_bytes = parse_size(&size)
        .with_context(|| format!("Invalid size format: {}", size))?;

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    // Ensure .nbd directory exists
    let nbd_dir_inode = match fs.lookup(&creds, 0, b".nbd").await {
        Ok(inode) => inode,
        Err(_) => {
            let attr = SetAttributes {
                mode: SetMode::Set(0o755),
                uid: SetUid::Set(0),
                gid: SetGid::Set(0),
                ..Default::default()
            };
            let (inode, _) = fs.mkdir(&creds, 0, b".nbd", &attr).await?;
            info!("Created .nbd directory");
            inode
        }
    };

    // Check if device already exists
    if fs.lookup(&creds, nbd_dir_inode, name.as_bytes()).await.is_ok() {
        anyhow::bail!("NBD device '{}' already exists", name);
    }

    // Create the device file
    let attr = SetAttributes {
        mode: SetMode::Set(0o644),
        uid: SetUid::Set(0),
        gid: SetGid::Set(0),
        size: crate::fs::types::SetSize::Set(size_bytes),
        ..Default::default()
    };

    let (device_inode, _) = fs.create(&creds, nbd_dir_inode, name.as_bytes(), &attr).await?;
    
    // Flush to ensure persistence
    fs.flush_coordinator.flush().await?;

    println!("✓ Created NBD device '{}' ({} bytes)", name, size_bytes.to_formatted_string(&Locale::en));
    println!("  Inode: {}", device_inode);
    println!("  Size: {}", format_size(size_bytes));
    println!("\nConnect with:");
    println!("  nbd-client <host> <port> /dev/nbd0 -N {}", name);

    Ok(())
}

pub async fn list_devices(config: PathBuf) -> Result<()> {
    let fs = init_filesystem(&config).await?;

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    // Look up .nbd directory
    let nbd_dir_inode = match fs.lookup(&creds, 0, b".nbd").await {
        Ok(inode) => inode,
        Err(_) => {
            println!("No NBD devices found (.nbd directory does not exist)");
            return Ok(());
        }
    };

    let auth = crate::fs::types::AuthContext {
        uid: 0,
        gid: 0,
        gids: vec![],
    };

    let entries = fs.readdir(&auth, nbd_dir_inode, 0, 1000).await?;

    let mut devices = Vec::new();
    for entry in &entries.entries {
        let name = &entry.name;
        if name == b"." || name == b".." {
            continue;
        }

        let inode = fs.inode_store.get(entry.fileid).await?;
        if let crate::fs::inode::Inode::File(file_inode) = inode {
            devices.push((
                String::from_utf8_lossy(name).to_string(),
                entry.fileid,
                file_inode.size,
            ));
        }
    }

    if devices.is_empty() {
        println!("No NBD devices found");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("NAME").fg(Color::Green),
        Cell::new("INODE").fg(Color::Green),
        Cell::new("SIZE").fg(Color::Green),
        Cell::new("SIZE (bytes)").fg(Color::Green),
    ]);

    for (name, inode, size) in devices {
        table.add_row(vec![
            Cell::new(name),
            Cell::new(inode),
            Cell::new(format_size(size)),
            Cell::new(size.to_formatted_string(&Locale::en)),
        ]);
    }

    println!("{}", table);
    Ok(())
}

pub async fn delete_device(config: PathBuf, name: String, force: bool) -> Result<()> {
    let fs = init_filesystem(&config).await?;

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    // Look up .nbd directory
    let nbd_dir_inode = fs.lookup(&creds, 0, b".nbd").await
        .context("Failed to find .nbd directory")?;

    // Check if device exists
    let device_inode = fs.lookup(&creds, nbd_dir_inode, name.as_bytes()).await
        .with_context(|| format!("NBD device '{}' not found", name))?;

    if !force {
        println!("Are you sure you want to delete NBD device '{}'? This cannot be undone.", name);
        println!("Use --force to skip this confirmation.");
        anyhow::bail!("Deletion cancelled");
    }

    // Delete the device
    let auth = AuthContext {
        uid: 0,
        gid: 0,
        gids: vec![],
    };
    fs.remove(&auth, nbd_dir_inode, name.as_bytes()).await?;
    
    // Flush to ensure persistence
    fs.flush_coordinator.flush().await?;

    println!("✓ Deleted NBD device '{}'", name);
    println!("  Inode: {}", device_inode);

    Ok(())
}

pub async fn resize_device(config: PathBuf, name: String, size: String) -> Result<()> {
    let fs = init_filesystem(&config).await?;

    let size_bytes = parse_size(&size)
        .with_context(|| format!("Invalid size format: {}", size))?;

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    // Look up .nbd directory
    let nbd_dir_inode = fs.lookup(&creds, 0, b".nbd").await
        .context("Failed to find .nbd directory")?;

    // Look up device
    let device_inode = fs.lookup(&creds, nbd_dir_inode, name.as_bytes()).await
        .with_context(|| format!("NBD device '{}' not found", name))?;

    // Get current size
    let inode = fs.inode_store.get(device_inode).await?;
    let old_size = match inode {
        crate::fs::inode::Inode::File(file_inode) => file_inode.size,
        _ => anyhow::bail!("'{}' is not a file", name),
    };

    // Resize
    let attr = SetAttributes {
        size: crate::fs::types::SetSize::Set(size_bytes),
        ..Default::default()
    };

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    fs.setattr(&creds, device_inode, &attr).await?;
    
    // Flush to ensure persistence
    fs.flush_coordinator.flush().await?;

    println!("✓ Resized NBD device '{}'", name);
    println!("  Old size: {} ({})", format_size(old_size), old_size.to_formatted_string(&Locale::en));
    println!("  New size: {} ({})", format_size(size_bytes), size_bytes.to_formatted_string(&Locale::en));
    
    if size_bytes < old_size {
        println!("\n⚠ Warning: Device was shrunk. Make sure no filesystem is using the truncated space!");
    }

    Ok(())
}

fn parse_size(size: &str) -> Result<u64> {
    let size = size.trim().to_uppercase();
    
    // Try to parse as plain number first
    if let Ok(bytes) = size.parse::<u64>() {
        return Ok(bytes);
    }

    // Parse with suffix (e.g., "10G", "512M", "1T")
    let (num_str, suffix) = if size.ends_with("TB") || size.ends_with("GB") || size.ends_with("MB") || size.ends_with("KB") {
        size.split_at(size.len() - 2)
    } else if size.ends_with('T') || size.ends_with('G') || size.ends_with('M') || size.ends_with('K') {
        size.split_at(size.len() - 1)
    } else {
        anyhow::bail!("Invalid size format. Use formats like: 10G, 512M, 1T, or plain bytes");
    };

    let num: f64 = num_str.trim().parse()
        .context("Invalid number in size specification")?;

    let multiplier = match suffix {
        "K" | "KB" => 1024u64,
        "M" | "MB" => 1024u64 * 1024,
        "G" | "GB" => 1024u64 * 1024 * 1024,
        "T" | "TB" => 1024u64 * 1024 * 1024 * 1024,
        _ => anyhow::bail!("Unknown size suffix: {}", suffix),
    };

    Ok((num * multiplier as f64) as u64)
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1KB").unwrap(), 1024);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1T").unwrap(), 1024u64 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("1TB").unwrap(), 1024u64 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("10.5G").unwrap(), (10.5 * 1024.0 * 1024.0 * 1024.0) as u64);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_size(1024u64 * 1024 * 1024 * 1024), "1.00 TB");
    }
}

