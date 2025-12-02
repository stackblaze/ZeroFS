// Control protocol for CLI to communicate with running server
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use crate::fs::ZeroFS;
use std::sync::Arc;
use tracing::{info, error};

#[derive(Debug, Serialize, Deserialize)]
pub enum ControlRequest {
    CreateDevice { name: String, size: u64 },
    ListDevices,
    DeleteDevice { name: String, force: bool },
    ResizeDevice { name: String, size: u64 },
    Ping,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ControlResponse {
    Success { message: String },
    DeviceList { devices: Vec<DeviceInfo> },
    Error { message: String },
    Pong,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub inode: u64,
    pub size: u64,
}

pub struct ControlServer {
    filesystem: Arc<ZeroFS>,
    socket_path: String,
}

impl ControlServer {
    pub fn new(filesystem: Arc<ZeroFS>, socket_path: String) -> Self {
        Self {
            filesystem,
            socket_path,
        }
    }

    pub async fn run(&self) -> Result<()> {
        // Remove old socket if it exists
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        info!("Control server listening on {}", self.socket_path);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let fs = self.filesystem.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, fs).await {
                            error!("Control connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept control connection: {}", e);
                }
            }
        }
    }
}

async fn handle_connection(mut stream: UnixStream, fs: Arc<ZeroFS>) -> Result<()> {
    // Read request length (4 bytes)
    let len = stream.read_u32().await? as usize;
    
    // Read request data
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    
    let request: ControlRequest = serde_json::from_slice(&buf)?;
    
    let response = match request {
        ControlRequest::Ping => ControlResponse::Pong,
        ControlRequest::CreateDevice { name, size } => {
            match create_device_internal(&fs, &name, size).await {
                Ok(inode) => ControlResponse::Success {
                    message: format!("Created device '{}' (inode: {}, size: {} bytes)", name, inode, size),
                },
                Err(e) => ControlResponse::Error {
                    message: format!("Failed to create device: {}", e),
                },
            }
        }
        ControlRequest::ListDevices => {
            match list_devices_internal(&fs).await {
                Ok(devices) => ControlResponse::DeviceList { devices },
                Err(e) => ControlResponse::Error {
                    message: format!("Failed to list devices: {}", e),
                },
            }
        }
        ControlRequest::DeleteDevice { name, force } => {
            match delete_device_internal(&fs, &name, force).await {
                Ok(_) => ControlResponse::Success {
                    message: format!("Deleted device '{}'", name),
                },
                Err(e) => ControlResponse::Error {
                    message: format!("Failed to delete device: {}", e),
                },
            }
        }
        ControlRequest::ResizeDevice { name, size } => {
            match resize_device_internal(&fs, &name, size).await {
                Ok(_) => ControlResponse::Success {
                    message: format!("Resized device '{}' to {} bytes", name, size),
                },
                Err(e) => ControlResponse::Error {
                    message: format!("Failed to resize device: {}", e),
                },
            }
        }
    };
    
    // Send response
    let response_bytes = serde_json::to_vec(&response)?;
    stream.write_u32(response_bytes.len() as u32).await?;
    stream.write_all(&response_bytes).await?;
    stream.flush().await?;
    
    Ok(())
}

async fn create_device_internal(fs: &ZeroFS, name: &str, size: u64) -> Result<u64> {
    use crate::fs::permissions::Credentials;
    use crate::fs::types::{SetAttributes, SetGid, SetMode, SetSize, SetUid};

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
            inode
        }
    };

    // Check if device already exists
    if fs.lookup(&creds, nbd_dir_inode, name.as_bytes()).await.is_ok() {
        anyhow::bail!("Device '{}' already exists", name);
    }

    // Create the device file
    let attr = SetAttributes {
        mode: SetMode::Set(0o644),
        uid: SetUid::Set(0),
        gid: SetGid::Set(0),
        ..Default::default()
    };

    let (device_inode, _) = fs.create(&creds, nbd_dir_inode, name.as_bytes(), &attr).await?;
    
    // Set the size (create always creates files with size 0)
    let size_attr = SetAttributes {
        size: SetSize::Set(size),
        ..Default::default()
    };
    fs.setattr(&creds, device_inode, &size_attr).await?;
    
    // Flush to ensure persistence
    fs.flush_coordinator.flush().await?;

    Ok(device_inode)
}

async fn list_devices_internal(fs: &ZeroFS) -> Result<Vec<DeviceInfo>> {
    use crate::fs::types::AuthContext;
    use crate::fs::inode::Inode;

    let auth = AuthContext {
        uid: 0,
        gid: 0,
        gids: vec![],
    };

    // Look up .nbd directory
    let nbd_dir_inode = fs.directory_store.get(0, b".nbd").await?;

    let entries = fs.readdir(&auth, nbd_dir_inode, 0, 1000).await?;

    let mut devices = Vec::new();
    for entry in &entries.entries {
        let name = &entry.name;
        if name == b"." || name == b".." {
            continue;
        }

        let inode = fs.inode_store.get(entry.fileid).await?;

        if let Inode::File(file_inode) = inode {
            devices.push(DeviceInfo {
                name: String::from_utf8_lossy(name).to_string(),
                inode: entry.fileid,
                size: file_inode.size,
            });
        }
    }

    Ok(devices)
}

async fn delete_device_internal(fs: &ZeroFS, name: &str, _force: bool) -> Result<()> {
    use crate::fs::permissions::Credentials;
    use crate::fs::types::AuthContext;

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    // Look up .nbd directory
    let nbd_dir_inode = fs.lookup(&creds, 0, b".nbd").await?;

    // Check if device exists
    let _device_inode = fs.lookup(&creds, nbd_dir_inode, name.as_bytes()).await?;

    // Remove the device
    let auth = AuthContext {
        uid: 0,
        gid: 0,
        gids: vec![],
    };
    fs.remove(&auth, nbd_dir_inode, name.as_bytes()).await?;
    
    // Flush to ensure persistence
    fs.flush_coordinator.flush().await?;

    Ok(())
}

async fn resize_device_internal(fs: &ZeroFS, name: &str, new_size: u64) -> Result<()> {
    use crate::fs::permissions::Credentials;
    use crate::fs::types::{SetAttributes, SetSize};

    let creds = Credentials {
        uid: 0,
        gid: 0,
        groups: [0; 16],
        groups_count: 1,
    };

    // Look up .nbd directory
    let nbd_dir_inode = fs.lookup(&creds, 0, b".nbd").await?;

    // Check if device exists
    let device_inode = fs.lookup(&creds, nbd_dir_inode, name.as_bytes()).await?;

    // Resize the device
    let attr = SetAttributes {
        size: SetSize::Set(new_size),
        ..Default::default()
    };

    fs.setattr(&creds, device_inode, &attr).await?;
    
    // Flush to ensure persistence
    fs.flush_coordinator.flush().await?;

    Ok(())
}

// Client functions
pub async fn send_control_request(socket_path: &str, request: ControlRequest) -> Result<ControlResponse> {
    let mut stream = UnixStream::connect(socket_path).await
        .context("Failed to connect to control socket. Is the server running?")?;
    
    // Send request
    let request_bytes = serde_json::to_vec(&request)?;
    stream.write_u32(request_bytes.len() as u32).await?;
    stream.write_all(&request_bytes).await?;
    stream.flush().await?;
    
    // Read response length
    let len = stream.read_u32().await? as usize;
    
    // Read response data
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    
    let response: ControlResponse = serde_json::from_slice(&buf)?;
    
    Ok(response)
}

