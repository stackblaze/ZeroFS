use crate::config::Settings;
use crate::control::{send_control_request, ControlRequest, ControlResponse};
use anyhow::{Context, Result};
use comfy_table::{Cell, Color, Table};
use num_format::{Locale, ToFormattedString};
use std::io::Write;
use std::path::PathBuf;

fn get_control_socket_path(config: &PathBuf) -> Result<String> {
    let settings = Settings::from_file(config.to_str().unwrap())
        .with_context(|| format!("Failed to load config from {}", config.display()))?;
    
    let socket_path = settings.cache.dir.join("zerofs.sock");
    Ok(socket_path.to_str().unwrap().to_string())
}

pub async fn create_device(config: PathBuf, name: String, size: String) -> Result<()> {
    let socket_path = get_control_socket_path(&config)?;
    let size_bytes = parse_size(&size)
        .with_context(|| format!("Invalid size format: {}", size))?;

    let request = ControlRequest::CreateDevice {
        name: name.clone(),
        size: size_bytes,
    };

    let response = send_control_request(&socket_path, request).await?;

    match response {
        ControlResponse::Success { message } => {
            println!("✓ {}", message);
            println!("  Size: {}", format_size(size_bytes));
            println!("\nConnect with:");
            println!("  nbd-client <host> <port> /dev/nbd0 -N {}", name);
            Ok(())
        }
        ControlResponse::Error { message } => {
            anyhow::bail!("Failed to create device: {}", message)
        }
        _ => anyhow::bail!("Unexpected response from server"),
    }
}

pub async fn list_devices(config: PathBuf) -> Result<()> {
    let socket_path = get_control_socket_path(&config)?;

    let request = ControlRequest::ListDevices;
    let response = send_control_request(&socket_path, request).await?;

    match response {
        ControlResponse::DeviceList { devices } => {
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

            for device in devices {
                table.add_row(vec![
                    Cell::new(device.name),
                    Cell::new(device.inode),
                    Cell::new(format_size(device.size)),
                    Cell::new(device.size.to_formatted_string(&Locale::en)),
                ]);
            }

            println!("{}", table);
            Ok(())
        }
        ControlResponse::Error { message } => {
            anyhow::bail!("Failed to list devices: {}", message)
        }
        _ => anyhow::bail!("Unexpected response from server"),
    }
}

pub async fn delete_device(config: PathBuf, name: String, force: bool) -> Result<()> {
    if !force {
        println!("Are you sure you want to delete NBD device '{}'? This cannot be undone.", name);
        println!("Use --force to skip this confirmation.");
        anyhow::bail!("Deletion cancelled");
    }

    let socket_path = get_control_socket_path(&config)?;

    let request = ControlRequest::DeleteDevice {
        name: name.clone(),
        force,
    };

    let response = send_control_request(&socket_path, request).await?;

    match response {
        ControlResponse::Success { message } => {
            println!("✓ {}", message);
            Ok(())
        }
        ControlResponse::Error { message } => {
            anyhow::bail!("Failed to delete device: {}", message)
        }
        _ => anyhow::bail!("Unexpected response from server"),
    }
}

pub async fn resize_device(config: PathBuf, name: String, size: String) -> Result<()> {
    let socket_path = get_control_socket_path(&config)?;
    let size_bytes = parse_size(&size)
        .with_context(|| format!("Invalid size format: {}", size))?;

    let request = ControlRequest::ResizeDevice {
        name: name.clone(),
        size: size_bytes,
    };

    let response = send_control_request(&socket_path, request).await?;

    match response {
        ControlResponse::Success { message } => {
            println!("✓ {}", message);
            Ok(())
        }
        ControlResponse::Error { message } => {
            anyhow::bail!("Failed to resize device: {}", message)
        }
        _ => anyhow::bail!("Unexpected response from server"),
    }
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

pub async fn export_device(
    _config: PathBuf,
    name: String,
    size: String,
    nbd_device: String,
    filesystem: String,
    mount_point: PathBuf,
    nbd_host: String,
    nbd_port: u16,
) -> Result<()> {
    use std::process::Command;

    println!("🚀 Starting NBD device export workflow...\n");

    // Note: Device should already exist. Create it first with: zerofs nbd create
    println!("📝 Note: Assuming NBD device '{}' already exists in ZeroFS", name);
    println!("   If not, create it first with: zerofs nbd create -c <config> {} {}\n", name, size);

    // Step 1: Connect NBD client
    println!("🔌 Step 1/5: Connecting NBD client to {}...", nbd_device);
    let nbd_connect = Command::new("nbd-client")
        .args(&[
            &nbd_host,
            &nbd_port.to_string(),
            &nbd_device,
            "-N",
            &name,
            "-persist",
        ])
        .output()
        .context("Failed to execute nbd-client. Is it installed? (apt install nbd-client)")?;

    if !nbd_connect.status.success() {
        anyhow::bail!(
            "Failed to connect NBD client:\n{}",
            String::from_utf8_lossy(&nbd_connect.stderr)
        );
    }
    println!("✓ NBD device connected: {}", nbd_device);
    println!();

    // Step 2: Format the device
    println!("💾 Step 2/5: Formatting {} as {}...", nbd_device, filesystem);
    let format_result = match filesystem.as_str() {
        "btrfs" => Command::new("mkfs.btrfs")
            .args(&["-f", &nbd_device])
            .output(),
        "ext4" => Command::new("mkfs.ext4")
            .args(&["-F", &nbd_device])
            .output(),
        "xfs" => Command::new("mkfs.xfs")
            .args(&["-f", &nbd_device])
            .output(),
        "zfs" => {
            anyhow::bail!("ZFS requires manual setup. Use 'zpool create' with the NBD device.");
        }
        _ => anyhow::bail!("Unsupported filesystem: {}. Use btrfs, ext4, or xfs.", filesystem),
    };

    let format_output = format_result.context(format!(
        "Failed to format device. Is mkfs.{} installed?",
        filesystem
    ))?;

    if !format_output.status.success() {
        // Cleanup: disconnect NBD
        let _ = Command::new("nbd-client").args(&["-d", &nbd_device]).output();
        anyhow::bail!(
            "Failed to format device:\n{}",
            String::from_utf8_lossy(&format_output.stderr)
        );
    }
    println!("✓ Device formatted as {}", filesystem);
    println!();

    // Step 3: Create mount point
    println!("📁 Step 3/5: Creating mount point {}...", mount_point.display());
    std::fs::create_dir_all(&mount_point).context("Failed to create mount point")?;
    println!("✓ Mount point created");
    println!();

    // Step 4: Mount the filesystem
    println!("🔗 Step 4/5: Mounting {} to {}...", nbd_device, mount_point.display());
    
    // For btrfs, enable compression by default
    let mount_args = if filesystem == "btrfs" {
        vec!["-o", "compress=zstd", &nbd_device, mount_point.to_str().unwrap()]
    } else {
        vec![&nbd_device, mount_point.to_str().unwrap()]
    };
    
    let mount_output = Command::new("mount")
        .args(&mount_args)
        .output()
        .context("Failed to mount device")?;

    if !mount_output.status.success() {
        // Cleanup: disconnect NBD
        let _ = Command::new("nbd-client").args(&["-d", &nbd_device]).output();
        anyhow::bail!(
            "Failed to mount device:\n{}",
            String::from_utf8_lossy(&mount_output.stderr)
        );
    }
    println!("✓ Filesystem mounted");
    
    // For btrfs, create default subvolumes
    if filesystem == "btrfs" {
        println!("📂 Creating btrfs subvolumes...");
        let subvols = vec!["@", "@home", "@snapshots"];
        for subvol in &subvols {
            let subvol_path = format!("{}/{}", mount_point.display(), subvol);
            let _ = Command::new("btrfs")
                .args(&["subvolume", "create", &subvol_path])
                .output();
        }
        println!("✓ Created subvolumes: {}", subvols.join(", "));
    }
    println!();

    // Step 5: Done
    println!("✅ Step 5/5: Mount complete!");

    println!();
    println!("✅ Export complete!\n");
    println!("📊 Summary:");
    println!("  NBD Device: {}", nbd_device);
    println!("  Filesystem: {}", filesystem);
    if filesystem == "btrfs" {
        println!("  Compression: zstd (enabled)");
        println!("  Subvolumes: @, @home, @snapshots");
    }
    println!("  Mount Point: {}", mount_point.display());
    println!();
    println!("🌐 Remote Access via ZeroFS NFS:");
    println!("  Clients can access NBD devices through ZeroFS's built-in NFS server:");
    println!();
    println!("  1️⃣  Mount ZeroFS via NFS:");
    println!("     sudo mount -t nfs <zerofs-host>:/ /mnt/zerofs");
    println!();
    println!("  2️⃣  Connect to NBD device:");
    println!("     sudo nbd-client <zerofs-host> <nbd-port> /dev/nbd0 -N {}", name);
    println!();
    println!("  3️⃣  Mount the formatted filesystem:");
    println!("     sudo mount /dev/nbd0 /mnt/storage");
    println!();
    
    if filesystem == "btrfs" {
        println!("📸 Btrfs Snapshot Commands:");
        println!("  # Create snapshot");
        println!("  sudo btrfs subvolume snapshot {} {}/snapshots/snap-$(date +%Y%m%d-%H%M)",
            mount_point.display(), mount_point.display());
        println!("  # List snapshots");
        println!("  sudo btrfs subvolume list {}", mount_point.display());
        println!("  # Delete snapshot");
        println!("  sudo btrfs subvolume delete {}/snapshots/<snapshot-name>", mount_point.display());
        println!();
    }
    
    println!("📝 To make persistent, add to /etc/fstab on this server:");
    if filesystem == "btrfs" {
        println!("  {} {} {} compress=zstd 0 0", nbd_device, mount_point.display(), filesystem);
    } else {
        println!("  {} {} {} defaults 0 0", nbd_device, mount_point.display(), filesystem);
    }
    println!();
    println!("⚠️  Important: Ensure ZeroFS server is running before system boot!");

    Ok(())
}

pub async fn create_snapshot(
    mount_point: PathBuf,
    name: Option<String>,
    subvolume: String,
    readonly: bool,
) -> Result<()> {
    use std::process::Command;

    // Generate snapshot name if not provided
    let snapshot_name = name.unwrap_or_else(|| {
        format!("snapshot-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
    });

    let source_path = format!("{}/{}", mount_point.display(), subvolume);
    let snapshot_path = format!("{}/@snapshots/{}", mount_point.display(), snapshot_name);

    println!("📸 Creating snapshot...");
    println!("  Source: {}", source_path);
    println!("  Snapshot: {}", snapshot_path);
    if readonly {
        println!("  Mode: Read-only");
    }

    // Check if source exists
    if !std::path::Path::new(&source_path).exists() {
        anyhow::bail!("Source subvolume does not exist: {}", source_path);
    }

    // Create snapshot
    let mut cmd = Command::new("btrfs");
    cmd.args(&["subvolume", "snapshot"]);
    
    if readonly {
        cmd.arg("-r");
    }
    
    cmd.args(&[&source_path, &snapshot_path]);

    let output = cmd.output()
        .context("Failed to execute btrfs command. Is btrfs-progs installed?")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to create snapshot:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!("✅ Snapshot created successfully!");
    println!("\n📊 Snapshot details:");
    println!("  Name: {}", snapshot_name);
    println!("  Path: {}", snapshot_path);
    println!("  Type: {}", if readonly { "Read-only" } else { "Read-write" });
    println!("\n💡 Access snapshot:");
    println!("  Local: ls {}", snapshot_path);
    println!("  NFS clients: ls /mnt/remote/@snapshots/{}", snapshot_name);
    println!("\n🔄 Restore file:");
    println!("  cp {}/myfile.txt {}/myfile.txt.restored", 
        snapshot_path, source_path);

    Ok(())
}

pub async fn list_snapshots(mount_point: PathBuf) -> Result<()> {
    use std::process::Command;

    println!("📋 Listing snapshots for {}...\n", mount_point.display());

    // List all subvolumes
    let output = Command::new("btrfs")
        .args(&["subvolume", "list", mount_point.to_str().unwrap()])
        .output()
        .context("Failed to execute btrfs command. Is btrfs-progs installed?")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to list subvolumes:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    
    // Filter for snapshots in @snapshots directory
    let snapshots: Vec<&str> = output_str
        .lines()
        .filter(|line| line.contains("@snapshots/"))
        .collect();

    if snapshots.is_empty() {
        println!("No snapshots found in {}/@snapshots/", mount_point.display());
        println!("\n💡 Create a snapshot with:");
        println!("  zerofs nbd snapshot --mount-point {}", mount_point.display());
        return Ok(());
    }

    // Create table
    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("SNAPSHOT NAME").fg(Color::Green),
        Cell::new("ID").fg(Color::Green),
        Cell::new("PATH").fg(Color::Green),
    ]);

    for line in snapshots {
        // Parse btrfs output: "ID 258 gen 42 top level 5 path @snapshots/snapshot-20251202"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 9 {
            let id = parts[1];
            let full_path = parts[8];
            
            // Extract just the snapshot name
            if let Some(name) = full_path.strip_prefix("@snapshots/") {
                table.add_row(vec![
                    Cell::new(name),
                    Cell::new(id),
                    Cell::new(format!("{}/@snapshots/{}", mount_point.display(), name)),
                ]);
            }
        }
    }

    println!("{}", table);
    println!("\n💡 Delete snapshot:");
    println!("  zerofs nbd delete-snapshot --mount-point {} --name <snapshot-name> --force",
        mount_point.display());

    Ok(())
}

pub async fn delete_snapshot(
    mount_point: PathBuf,
    name: String,
    force: bool,
) -> Result<()> {
    use std::process::Command;

    let snapshot_path = format!("{}/@snapshots/{}", mount_point.display(), name);

    // Check if snapshot exists
    if !std::path::Path::new(&snapshot_path).exists() {
        anyhow::bail!("Snapshot does not exist: {}", snapshot_path);
    }

    if !force {
        println!("⚠️  WARNING: This will permanently delete the snapshot!");
        println!("  Snapshot: {}", snapshot_path);
        println!("\nUse --force to confirm deletion.");
        anyhow::bail!("Deletion cancelled");
    }

    println!("🗑️  Deleting snapshot: {}", name);

    let output = Command::new("btrfs")
        .args(&["subvolume", "delete", &snapshot_path])
        .output()
        .context("Failed to execute btrfs command")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to delete snapshot:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!("✅ Snapshot deleted successfully!");
    println!("  Name: {}", name);
    println!("  Path: {}", snapshot_path);

    Ok(())
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

