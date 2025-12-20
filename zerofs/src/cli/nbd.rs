use crate::config::Settings;
use crate::control::{send_control_request, ControlRequest, ControlResponse};
use anyhow::{Context, Result};
use comfy_table::{Cell, Color, Table};
use num_format::{Locale, ToFormattedString};
use std::path::PathBuf;
use std::process::Command;

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

