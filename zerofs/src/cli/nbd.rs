use crate::config::Settings;
use crate::control::{send_control_request, ControlRequest, ControlResponse};
use anyhow::{Context, Result};
use comfy_table::{Cell, Color, Table};
use num_format::{Locale, ToFormattedString};
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

