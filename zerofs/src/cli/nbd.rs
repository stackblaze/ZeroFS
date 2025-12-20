use crate::config::Settings;
use crate::control::{send_control_request, ControlRequest, ControlResponse};
use anyhow::{Context, Result};
use comfy_table::{Cell, Color, Table};
use num_format::{Locale, ToFormattedString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;

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

pub async fn format_device(
    config: PathBuf,
    name: String,
    filesystem: String,
    mkfs_options: Option<String>,
) -> Result<()> {
    let settings = Settings::from_file(config.to_str().unwrap())
        .with_context(|| format!("Failed to load config from {}", config.display()))?;

    // Verify device exists
    let socket_path = get_control_socket_path(&config)?;
    let request = ControlRequest::ListDevices;
    let response = send_control_request(&socket_path, request).await?;

    let device_info = match response {
        ControlResponse::DeviceList { devices } => {
            devices
                .into_iter()
                .find(|d| d.name == name)
                .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", name))?
        }
        ControlResponse::Error { message } => {
            anyhow::bail!("Failed to list devices: {}", message)
        }
        _ => anyhow::bail!("Unexpected response from server"),
    };

    println!("Formatting device '{}' ({}) with {} filesystem...", 
             name, format_size(device_info.size), filesystem);
    println!("(Formatting directly on server - no network overhead)");

    // Mount ZeroFS locally to access the device file directly
    // Prefer 9P Unix socket for best performance, fallback to NFS localhost
    let mount_point = format!("/tmp/zerofs-format-{}", std::process::id());
    let device_path = format!("{}/.nbd/{}", mount_point, name);

    // Create temporary mount point
    std::fs::create_dir_all(&mount_point)
        .context("Failed to create temporary mount point")?;

    // Determine mount method (prefer 9P Unix socket, then 9P TCP, then NFS)
    let mount_result = if let Some(ninep_config) = &settings.servers.ninep {
        if let Some(ref socket_path) = ninep_config.unix_socket {
            // Mount via 9P Unix socket (best performance)
            Command::new("mount")
                .arg("-t")
                .arg("9p")
                .arg("-o")
                .arg("trans=unix,version=9p2000.L,cache=mmap,access=user")
                .arg(socket_path.to_str().unwrap())
                .arg(&mount_point)
                .status()
        } else if let Some(ref addrs) = ninep_config.addresses {
            // Mount via 9P TCP
            let addr = addrs.iter().next()
                .ok_or_else(|| anyhow::anyhow!("No 9P server addresses configured"))?;
            Command::new("mount")
                .arg("-t")
                .arg("9p")
                .arg("-o")
                .arg(format!("trans=tcp,port={},version=9p2000.L,cache=mmap,access=user", addr.port()))
                .arg("127.0.0.1")
                .arg(&mount_point)
                .status()
        } else {
            anyhow::bail!("No 9P server configured. Please configure 9P or NFS server in zerofs.toml");
        }
    } else if let Some(_nfs_config) = &settings.servers.nfs {
        // Mount via NFS localhost
        Command::new("mount")
            .arg("-t")
            .arg("nfs")
            .arg("-o")
            .arg("vers=3,nolock,tcp,port=2049,mountport=2049")
            .arg("127.0.0.1:/")
            .arg(&mount_point)
            .status()
    } else {
        anyhow::bail!("No file access protocol (9P or NFS) configured. Please configure at least one in zerofs.toml");
    };

    let mount_status = mount_result
        .context("Failed to execute mount command. Make sure you have permission to mount filesystems.")?;

    if !mount_status.success() {
        let _ = std::fs::remove_dir(&mount_point);
        anyhow::bail!("Failed to mount ZeroFS locally. Is the server running? You may need sudo privileges.");
    }

    // Verify device file exists
    if !std::path::Path::new(&device_path).exists() {
        let _ = Command::new("umount").arg(&mount_point).status();
        let _ = std::fs::remove_dir(&mount_point);
        anyhow::bail!("Device file not found at {}", device_path);
    }

    // Format the device file directly (mkfs.btrfs can format regular files)
    let format_result = match filesystem.to_lowercase().as_str() {
        "btrfs" => {
            let mut cmd = Command::new("mkfs.btrfs");
            cmd.arg("-f"); // Force formatting
            
            // Add custom options if provided
            if let Some(opts) = &mkfs_options {
                // Parse options (simple space-separated)
                for opt in opts.split_whitespace() {
                    cmd.arg(opt);
                }
            }
            
            cmd.arg(&device_path);
            cmd.status()
        }
        _ => {
            let _ = Command::new("umount").arg(&mount_point).status();
            let _ = std::fs::remove_dir(&mount_point);
            anyhow::bail!("Unsupported filesystem type: {}. Currently only 'btrfs' is supported.", filesystem);
        }
    };

    let format_status = format_result
        .with_context(|| format!("Failed to execute mkfs.{}. Is it installed?", filesystem))?;

    // Unmount and cleanup
    let umount_status = Command::new("umount")
        .arg(&mount_point)
        .status()
        .context("Failed to unmount ZeroFS")?;
    
    let _ = std::fs::remove_dir(&mount_point);

    if !umount_status.success() {
        eprintln!("Warning: Failed to unmount {}. You may need to unmount manually.", mount_point);
    }

    if !format_status.success() {
        anyhow::bail!("Failed to format device with {} filesystem", filesystem);
    }

    println!("✓ Successfully formatted device '{}' with {} filesystem", name, filesystem);
    println!("  Device size: {}", format_size(device_info.size));
    println!("  Formatting completed server-side (no network overhead)");

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

pub async fn export_device(
    config: PathBuf,
    name: String,
    mount_point: PathBuf,
    nbd_device: PathBuf,
    filesystem: Option<String>,
    nfs_export_path: Option<String>,
    nfs_options: String,
) -> Result<()> {
    let settings = Settings::from_file(config.to_str().unwrap())
        .with_context(|| format!("Failed to load config from {}", config.display()))?;

    // Verify device exists
    let socket_path = get_control_socket_path(&config)?;
    let request = ControlRequest::ListDevices;
    let response = send_control_request(&socket_path, request).await?;

    let device_info = match response {
        ControlResponse::DeviceList { devices } => {
            devices
                .into_iter()
                .find(|d| d.name == name)
                .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", name))?
        }
        ControlResponse::Error { message } => {
            anyhow::bail!("Failed to list devices: {}", message)
        }
        _ => anyhow::bail!("Unexpected response from server"),
    };

    println!("Exporting device '{}' ({}) via NFS...", name, format_size(device_info.size));

    // Determine NBD server connection method
    let (host_opt, port_opt, unix_socket_opt) = if let Some(nbd_config) = &settings.servers.nbd {
        if let Some(socket) = &nbd_config.unix_socket {
            (None, None, Some(socket.clone()))
        } else if let Some(addrs) = &nbd_config.addresses {
            let addr = addrs.iter().next()
                .ok_or_else(|| anyhow::anyhow!("No NBD server addresses configured"))?;
            (Some(addr.ip().to_string()), Some(addr.port()), None)
        } else {
            (Some("127.0.0.1".to_string()), Some(10809), None)
        }
    } else {
        (Some("127.0.0.1".to_string()), Some(10809), None)
    };

    // Connect to NBD device
    println!("Connecting to NBD device...");
    let connect_result = if let Some(ref socket_path) = unix_socket_opt {
        Command::new("nbd-client")
            .arg("-u")
            .arg(socket_path.to_str().unwrap())
            .arg(nbd_device.to_str().unwrap())
            .arg("-N")
            .arg(&name)
            .status()
    } else {
        let host = host_opt.as_ref().unwrap();
        let port = port_opt.unwrap();
        Command::new("nbd-client")
            .arg(host)
            .arg(&port.to_string())
            .arg(nbd_device.to_str().unwrap())
            .arg("-N")
            .arg(&name)
            .status()
    };

    let connect_status = connect_result
        .context("Failed to execute nbd-client. Is it installed?")?;

    if !connect_status.success() {
        anyhow::bail!("Failed to connect to NBD device '{}'. Is the server running?", name);
    }

    // Check if device is already formatted
    let detected_fs = detect_filesystem(&nbd_device)?;
    let fs_type = if let Some(ref fs) = detected_fs {
        println!("Detected filesystem: {}", fs);
        fs.clone()
    } else if let Some(ref fs) = filesystem {
        // Format the device
        println!("Formatting device with {} filesystem...", fs);
        format_nbd_device(&nbd_device, fs, None)?;
        fs.clone()
    } else {
        anyhow::bail!("Device is not formatted and no filesystem type specified. Use --filesystem to format it.");
    };

    // Create mount point
    fs::create_dir_all(&mount_point)
        .context("Failed to create mount point")?;

    // Mount the device
    println!("Mounting device to {}...", mount_point.display());
    let mount_status = Command::new("mount")
        .arg("-t")
        .arg(&fs_type)
        .arg(nbd_device.to_str().unwrap())
        .arg(mount_point.to_str().unwrap())
        .status()
        .context("Failed to execute mount command")?;

    if !mount_status.success() {
        let _ = Command::new("nbd-client").arg("-d").arg(nbd_device.to_str().unwrap()).status();
        anyhow::bail!("Failed to mount device");
    }

    // Configure NFS export
    let export_path = nfs_export_path.as_deref().unwrap_or(mount_point.to_str().unwrap());
    println!("Configuring NFS export: {} ({})", export_path, nfs_options);
    
    add_nfs_export(export_path, &nfs_options)?;
    reload_nfs_exports()?;

    println!("✓ Successfully exported device '{}' via NFS", name);
    println!("  Mount point: {}", mount_point.display());
    println!("  NFS export: {}", export_path);
    println!("  Filesystem: {}", fs_type);
    println!("\nClients can mount with:");
    println!("  mount -t nfs <server-ip>:{} <local-mount-point>", export_path);

    Ok(())
}

pub async fn unexport_device(
    _config: PathBuf,
    name: String,
    mount_point: PathBuf,
    nbd_device: PathBuf,
) -> Result<()> {
    println!("Unexporting device '{}'...", name);

    // Remove NFS export
    let export_path = mount_point.to_str().unwrap();
    remove_nfs_export(export_path)?;
    reload_nfs_exports()?;
    println!("✓ Removed NFS export: {}", export_path);

    // Unmount device
    let umount_status = Command::new("umount")
        .arg(mount_point.to_str().unwrap())
        .status()
        .context("Failed to unmount device")?;

    if !umount_status.success() {
        eprintln!("Warning: Failed to unmount device (may already be unmounted)");
    } else {
        println!("✓ Unmounted device from {}", mount_point.display());
    }

    // Disconnect NBD
    let disconnect_status = Command::new("nbd-client")
        .arg("-d")
        .arg(nbd_device.to_str().unwrap())
        .status()
        .context("Failed to disconnect NBD device")?;

    if !disconnect_status.success() {
        eprintln!("Warning: Failed to disconnect NBD device (may already be disconnected)");
    } else {
        println!("✓ Disconnected NBD device");
    }

    println!("✓ Successfully unexported device '{}'", name);
    Ok(())
}

fn detect_filesystem(device: &Path) -> Result<Option<String>> {
    let output = Command::new("blkid")
        .arg("-s")
        .arg("TYPE")
        .arg("-o")
        .arg("value")
        .arg(device.to_str().unwrap())
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let fs_type = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if fs_type.is_empty() {
                Ok(None)
            } else {
                Ok(Some(fs_type))
            }
        }
        _ => Ok(None),
    }
}

fn format_nbd_device(device: &Path, filesystem: &str, mkfs_options: Option<&str>) -> Result<()> {
    let mut cmd = Command::new(format!("mkfs.{}", filesystem));
    cmd.arg("-f"); // Force formatting
    
    if let Some(opts) = mkfs_options {
        for opt in opts.split_whitespace() {
            cmd.arg(opt);
        }
    }
    
    cmd.arg(device.to_str().unwrap());
    let status = cmd.status()
        .with_context(|| format!("Failed to execute mkfs.{}. Is it installed?", filesystem))?;

    if !status.success() {
        anyhow::bail!("Failed to format device with {} filesystem", filesystem);
    }

    Ok(())
}

fn add_nfs_export(path: &str, options: &str) -> Result<()> {
    const EXPORTS_FILE: &str = "/etc/exports";
    
    // Read existing exports
    let content = fs::read_to_string(EXPORTS_FILE)
        .unwrap_or_else(|_| String::new());
    
    // Format: /path *(options) or /path host(options)
    let export_line = format!("{} *({})", path, options);
    
    // Check if export already exists (check for path)
    if content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with(path) && !trimmed.starts_with('#')
    }) {
        println!("NFS export for {} already exists in {}", path, EXPORTS_FILE);
        return Ok(());
    }

    // Append new export
    let mut new_content = content;
    if !new_content.ends_with('\n') && !new_content.is_empty() {
        new_content.push('\n');
    }
    new_content.push_str(&export_line);
    new_content.push('\n');

    // Write back (requires root)
    fs::write(EXPORTS_FILE, new_content)
        .context("Failed to write /etc/exports. Make sure you have root privileges.")?;

    Ok(())
}

fn remove_nfs_export(path: &str) -> Result<()> {
    const EXPORTS_FILE: &str = "/etc/exports";
    
    let content = fs::read_to_string(EXPORTS_FILE)
        .context("Failed to read /etc/exports")?;
    
    // Remove lines matching this export path (but keep comments)
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with(path) || trimmed.starts_with('#') || trimmed.is_empty()
        })
        .collect();

    let new_content = lines.join("\n");
    if !new_content.ends_with('\n') && !new_content.is_empty() {
        let mut final_content = new_content;
        final_content.push('\n');
        fs::write(EXPORTS_FILE, final_content)
            .context("Failed to write /etc/exports")?;
    } else {
        fs::write(EXPORTS_FILE, new_content)
            .context("Failed to write /etc/exports")?;
    }

    Ok(())
}

fn reload_nfs_exports() -> Result<()> {
    // Try exportfs -ra first (works without full NFS server)
    let status = Command::new("exportfs")
        .arg("-ra")
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("✓ Reloaded NFS exports");
            return Ok(());
        }
        _ => {}
    }

    // Fallback to systemctl reload (if NFS server is running)
    let status = Command::new("systemctl")
        .arg("reload")
        .arg("nfs-server")
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("✓ Reloaded NFS server");
            return Ok(());
        }
        _ => {}
    }

    // Last resort: restart
    println!("Warning: Could not reload NFS exports automatically. You may need to run:");
    println!("  sudo exportfs -ra");
    println!("  or");
    println!("  sudo systemctl restart nfs-server");

    Ok(())
}

pub async fn create_snapshot(
    _config: PathBuf,
    _name: String,
    mount_point: PathBuf,
    snapshot_name: String,
    snapshot_path: Option<String>,
    read_only: bool,
) -> Result<()> {
    // Verify mount point exists and is a BTRFS filesystem
    if !mount_point.exists() {
        anyhow::bail!("Mount point {} does not exist", mount_point.display());
    }

    // Check if it's a BTRFS filesystem
    let blkid_output = Command::new("findmnt")
        .arg("-n")
        .arg("-o")
        .arg("FSTYPE")
        .arg(mount_point.to_str().unwrap())
        .output()
        .context("Failed to check filesystem type")?;

    let fs_type = String::from_utf8_lossy(&blkid_output.stdout).trim().to_string();
    if fs_type != "btrfs" {
        anyhow::bail!("Mount point {} is not a BTRFS filesystem (detected: {})", mount_point.display(), fs_type);
    }

    // Determine snapshot path
    let snap_path = if let Some(ref path) = snapshot_path {
        mount_point.join(path)
    } else {
        mount_point.join(".snapshots").join(&snapshot_name)
    };

    // Create .snapshots directory if needed
    if let Some(parent) = snap_path.parent() {
        fs::create_dir_all(parent)
            .context("Failed to create snapshot directory")?;
    }

    // Create snapshot
    let mut cmd = Command::new("btrfs");
    cmd.arg("subvolume")
        .arg("snapshot");

    if read_only {
        cmd.arg("-r"); // Read-only snapshot
    }

    cmd.arg(mount_point.to_str().unwrap())
        .arg(snap_path.to_str().unwrap());

    let status = cmd.status()
        .context("Failed to execute btrfs command. Is btrfs-progs installed?")?;

    if !status.success() {
        anyhow::bail!("Failed to create snapshot");
    }

    println!("✓ Created {} snapshot: {}", if read_only { "read-only" } else { "read-write" }, snap_path.display());
    println!("  Source: {}", mount_point.display());
    println!("  Snapshot: {}", snap_path.display());

    Ok(())
}

pub async fn list_snapshots(
    _config: PathBuf,
    _name: String,
    mount_point: PathBuf,
) -> Result<()> {
    // Verify mount point exists and is a BTRFS filesystem
    if !mount_point.exists() {
        anyhow::bail!("Mount point {} does not exist", mount_point.display());
    }

    // List all subvolumes (snapshots are subvolumes)
    let output = Command::new("btrfs")
        .arg("subvolume")
        .arg("list")
        .arg("-o")
        .arg(mount_point.to_str().unwrap())
        .output()
        .context("Failed to execute btrfs command")?;

    if !output.status.success() {
        anyhow::bail!("Failed to list snapshots");
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = output_str.lines().collect();

    if lines.is_empty() {
        println!("No snapshots found");
        return Ok(());
    }

    // Parse and display snapshots
    let mut table = Table::new();
    table.set_header(vec![
        Cell::new("ID").fg(Color::Green),
        Cell::new("GEN").fg(Color::Green),
        Cell::new("PATH").fg(Color::Green),
        Cell::new("READ-ONLY").fg(Color::Green),
    ]);

    for line in lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let id = parts[0];
            let generation = parts[1];
            let path = parts[parts.len() - 1];
            let read_only = if line.contains("ro") { "Yes" } else { "No" };
            
            table.add_row(vec![
                Cell::new(id),
                Cell::new(generation),
                Cell::new(path),
                Cell::new(read_only),
            ]);
        }
    }

    println!("Snapshots for {}:", mount_point.display());
    println!("{}", table);

    Ok(())
}

pub async fn restore_snapshot(
    _config: PathBuf,
    _name: String,
    mount_point: PathBuf,
    snapshot_name: String,
    snapshot_path: Option<String>,
    target_path: Option<String>,
) -> Result<()> {
    // Verify mount point exists
    if !mount_point.exists() {
        anyhow::bail!("Mount point {} does not exist", mount_point.display());
    }

    // Determine snapshot path
    let snap_path = if let Some(ref path) = snapshot_path {
        mount_point.join(path)
    } else {
        mount_point.join(".snapshots").join(&snapshot_name)
    };

    if !snap_path.exists() {
        anyhow::bail!("Snapshot not found: {}", snap_path.display());
    }

    // Determine target path (defaults to root)
    let target = if let Some(ref path) = target_path {
        mount_point.join(path)
    } else {
        mount_point.clone()
    };

    println!("Restoring snapshot {} to {}...", snap_path.display(), target.display());
    println!("⚠ Warning: This will replace the target with snapshot contents!");

    // For BTRFS, we can either:
    // 1. Delete target subvolume and create new snapshot from snapshot (if target is subvolume)
    // 2. Use send/receive (for cross-filesystem restore)
    // 3. Use rsync or similar (simple but not atomic)

    // Check if target is the root of the filesystem (can't delete root subvolume)
    let is_root = target == mount_point;
    
    if is_root {
        // For root restore, use rsync to copy contents
        println!("Restoring to root filesystem, copying contents...");
        
        let rsync_status = Command::new("rsync")
            .arg("-a")
            .arg("--delete")
            .arg("--exclude")
            .arg(".snapshots")
            .arg(format!("{}/", snap_path.to_str().unwrap()))
            .arg(format!("{}/", target.to_str().unwrap()))
            .status()
            .context("Failed to execute rsync. Is it installed?")?;

        if !rsync_status.success() {
            anyhow::bail!("Failed to restore snapshot contents");
        }
    } else {
        // Check if target is a subvolume
        let subvol_output = Command::new("btrfs")
            .arg("subvolume")
            .arg("show")
            .arg(target.to_str().unwrap())
            .output();

        let is_subvolume = subvol_output.is_ok() && subvol_output.unwrap().status.success();

        if is_subvolume {
            // Delete target subvolume and create new snapshot
            println!("Target is a subvolume, deleting and recreating...");
            
            // Delete target
            let delete_status = Command::new("btrfs")
                .arg("subvolume")
                .arg("delete")
                .arg(target.to_str().unwrap())
                .status()
                .context("Failed to delete target subvolume")?;

            if !delete_status.success() {
                anyhow::bail!("Failed to delete target subvolume");
            }

            // Create new snapshot from snapshot
            let snapshot_status = Command::new("btrfs")
                .arg("subvolume")
                .arg("snapshot")
                .arg(snap_path.to_str().unwrap())
                .arg(target.to_str().unwrap())
                .status()
                .context("Failed to create snapshot from snapshot")?;

            if !snapshot_status.success() {
                anyhow::bail!("Failed to restore snapshot");
            }
        } else {
            // Use rsync to copy contents (safer for regular directories)
            println!("Target is a regular directory, copying contents...");
            
            let rsync_status = Command::new("rsync")
                .arg("-a")
                .arg("--delete")
                .arg(format!("{}/", snap_path.to_str().unwrap()))
                .arg(format!("{}/", target.to_str().unwrap()))
                .status()
                .context("Failed to execute rsync. Is it installed?")?;

            if !rsync_status.success() {
                anyhow::bail!("Failed to restore snapshot contents");
            }
        }
    }

    println!("✓ Successfully restored snapshot");
    println!("  Snapshot: {}", snap_path.display());
    println!("  Target: {}", target.display());

    Ok(())
}

pub async fn delete_snapshot(
    _config: PathBuf,
    _name: String,
    mount_point: PathBuf,
    snapshot_name: String,
    snapshot_path: Option<String>,
) -> Result<()> {
    // Verify mount point exists
    if !mount_point.exists() {
        anyhow::bail!("Mount point {} does not exist", mount_point.display());
    }

    // Determine snapshot path
    let snap_path = if let Some(ref path) = snapshot_path {
        mount_point.join(path)
    } else {
        mount_point.join(".snapshots").join(&snapshot_name)
    };

    if !snap_path.exists() {
        anyhow::bail!("Snapshot not found: {}", snap_path.display());
    }

    println!("Deleting snapshot: {}...", snap_path.display());

    // Delete snapshot (BTRFS subvolume)
    let status = Command::new("btrfs")
        .arg("subvolume")
        .arg("delete")
        .arg(snap_path.to_str().unwrap())
        .status()
        .context("Failed to execute btrfs command")?;

    if !status.success() {
        anyhow::bail!("Failed to delete snapshot");
    }

    println!("✓ Successfully deleted snapshot: {}", snap_path.display());

    Ok(())
}

