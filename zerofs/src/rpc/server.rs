use crate::checkpoint_manager::CheckpointManager;
use crate::fs::ZeroFS;
use crate::fs::snapshot_manager::SnapshotManager;
use crate::fs::tracing::AccessTracer;
use crate::rpc::proto::{self, admin_service_server::AdminService};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, UnixListenerStream};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use tracing::info;

#[derive(Clone)]
pub struct AdminRpcServer {
    checkpoint_manager: Arc<CheckpointManager>,
    snapshot_manager: Arc<SnapshotManager>,
    tracer: AccessTracer,
    fs: Arc<ZeroFS>,
}

impl AdminRpcServer {
    pub fn new(
        checkpoint_manager: Arc<CheckpointManager>,
        snapshot_manager: Arc<SnapshotManager>,
        tracer: AccessTracer,
        fs: Arc<ZeroFS>,
    ) -> Self {
        Self {
            checkpoint_manager,
            snapshot_manager,
            tracer,
            fs,
        }
    }
}

#[tonic::async_trait]
impl AdminService for AdminRpcServer {
    type WatchFileAccessStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<proto::FileAccessEvent, Status>> + Send>>;

    async fn create_checkpoint(
        &self,
        request: Request<proto::CreateCheckpointRequest>,
    ) -> Result<Response<proto::CreateCheckpointResponse>, Status> {
        let name = request.into_inner().name;

        let info = self
            .checkpoint_manager
            .create_checkpoint(&name)
            .await
            .map_err(|e| Status::internal(format!("Failed to create checkpoint: {}", e)))?;

        Ok(Response::new(proto::CreateCheckpointResponse {
            checkpoint: Some(info.into()),
        }))
    }

    async fn list_checkpoints(
        &self,
        _request: Request<proto::ListCheckpointsRequest>,
    ) -> Result<Response<proto::ListCheckpointsResponse>, Status> {
        let checkpoints = self
            .checkpoint_manager
            .list_checkpoints()
            .await
            .map_err(|e| Status::internal(format!("Failed to list checkpoints: {}", e)))?;

        Ok(Response::new(proto::ListCheckpointsResponse {
            checkpoints: checkpoints.into_iter().map(|c| c.into()).collect(),
        }))
    }

    async fn delete_checkpoint(
        &self,
        request: Request<proto::DeleteCheckpointRequest>,
    ) -> Result<Response<proto::DeleteCheckpointResponse>, Status> {
        let name = request.into_inner().name;

        self.checkpoint_manager
            .delete_checkpoint(&name)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete checkpoint: {}", e)))?;

        Ok(Response::new(proto::DeleteCheckpointResponse {}))
    }

    async fn get_checkpoint_info(
        &self,
        request: Request<proto::GetCheckpointInfoRequest>,
    ) -> Result<Response<proto::GetCheckpointInfoResponse>, Status> {
        let name = request.into_inner().name;

        let info = self
            .checkpoint_manager
            .get_checkpoint_info(&name)
            .await
            .map_err(|e| Status::internal(format!("Failed to get checkpoint info: {}", e)))?;

        match info {
            Some(checkpoint) => Ok(Response::new(proto::GetCheckpointInfoResponse {
                checkpoint: Some(checkpoint.into()),
            })),
            None => Err(Status::not_found(format!(
                "Checkpoint '{}' not found",
                name
            ))),
        }
    }

    async fn watch_file_access(
        &self,
        _request: Request<proto::WatchFileAccessRequest>,
    ) -> Result<Response<Self::WatchFileAccessStream>, Status> {
        let receiver = self.tracer.subscribe();

        let stream = BroadcastStream::new(receiver)
            .filter_map(|result| result.ok())
            .map(|event| Ok(event.into()));

        Ok(Response::new(Box::pin(stream)))
    }

    async fn create_dataset(
        &self,
        request: Request<proto::CreateDatasetRequest>,
    ) -> Result<Response<proto::CreateDatasetResponse>, Status> {
        let name = request.into_inner().name;

        // Allocate a new inode for the dataset root
        let root_inode = self.snapshot_manager.allocate_inode();
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let dataset = self
            .snapshot_manager
            .create_dataset(name, root_inode, created_at, false)
            .await
            .map_err(|e| Status::internal(format!("Failed to create dataset: {}", e)))?;

        Ok(Response::new(proto::CreateDatasetResponse {
            dataset: Some(dataset.into()),
        }))
    }

    async fn list_datasets(
        &self,
        _request: Request<proto::ListDatasetsRequest>,
    ) -> Result<Response<proto::ListDatasetsResponse>, Status> {
        let datasets = self.snapshot_manager.list_datasets().await;

        Ok(Response::new(proto::ListDatasetsResponse {
            datasets: datasets.into_iter().map(|s| s.into()).collect(),
        }))
    }

    async fn delete_dataset(
        &self,
        request: Request<proto::DeleteDatasetRequest>,
    ) -> Result<Response<proto::DeleteDatasetResponse>, Status> {
        let name = request.into_inner().name;

        self.snapshot_manager
            .delete_dataset(&name)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete dataset: {}", e)))?;

        Ok(Response::new(proto::DeleteDatasetResponse {}))
    }

    async fn get_dataset_info(
        &self,
        request: Request<proto::GetDatasetInfoRequest>,
    ) -> Result<Response<proto::GetDatasetInfoResponse>, Status> {
        let name = request.into_inner().name;

        let dataset = self
            .snapshot_manager
            .get_dataset_by_name(&name)
            .await
            .ok_or_else(|| Status::not_found(format!("Dataset '{}' not found", name)))?;

        Ok(Response::new(proto::GetDatasetInfoResponse {
            dataset: Some(dataset.into()),
        }))
    }

    async fn set_default_dataset(
        &self,
        request: Request<proto::SetDefaultDatasetRequest>,
    ) -> Result<Response<proto::SetDefaultDatasetResponse>, Status> {
        let name = request.into_inner().name;

        self.snapshot_manager
            .set_default_dataset(&name)
            .await
            .map_err(|e| Status::internal(format!("Failed to set default dataset: {}", e)))?;

        Ok(Response::new(proto::SetDefaultDatasetResponse {}))
    }

    async fn get_default_dataset(
        &self,
        _request: Request<proto::GetDefaultDatasetRequest>,
    ) -> Result<Response<proto::GetDefaultDatasetResponse>, Status> {
        let dataset_id = self.snapshot_manager.get_default_dataset().await;

        Ok(Response::new(proto::GetDefaultDatasetResponse {
            dataset_id,
        }))
    }

    async fn create_snapshot(
        &self,
        request: Request<proto::CreateSnapshotRequest>,
    ) -> Result<Response<proto::CreateSnapshotResponse>, Status> {
        let req = request.into_inner();
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Default to read-write snapshots (like btrfs)
        let is_readonly = req.readonly.unwrap_or(false);

        let snapshot = self
            .snapshot_manager
            .create_snapshot_by_name(&req.source_name, req.snapshot_name, created_at, is_readonly)
            .await
            .map_err(|e| Status::internal(format!("Failed to create snapshot: {}", e)))?;

        Ok(Response::new(proto::CreateSnapshotResponse {
            snapshot: Some(snapshot.into()),
        }))
    }

    async fn list_snapshots(
        &self,
        _request: Request<proto::ListSnapshotsRequest>,
    ) -> Result<Response<proto::ListSnapshotsResponse>, Status> {
        let snapshots = self.snapshot_manager.list_snapshots().await;

        Ok(Response::new(proto::ListSnapshotsResponse {
            snapshots: snapshots.into_iter().map(|s| s.into()).collect(),
        }))
    }

    async fn delete_snapshot(
        &self,
        request: Request<proto::DeleteSnapshotRequest>,
    ) -> Result<Response<proto::DeleteSnapshotResponse>, Status> {
        let name = request.into_inner().name;

        self.snapshot_manager
            .delete_snapshot_by_name(&name)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete snapshot: {}", e)))?;

        Ok(Response::new(proto::DeleteSnapshotResponse {}))
    }

    type ReadSnapshotFileStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<proto::FileChunk, Status>> + Send>>;

    async fn read_snapshot_file(
        &self,
        request: Request<proto::ReadSnapshotFileRequest>,
    ) -> Result<Response<Self::ReadSnapshotFileStream>, Status> {
        use crate::fs::inode::Inode;
        use tokio_stream::StreamExt;

        let req = request.into_inner();
        let snapshot_name = req.snapshot_name;
        let file_path = req.file_path;

        // Get snapshot info
        let snapshot = self
            .snapshot_manager
            .get_dataset_by_name(&snapshot_name)
            .await
            .ok_or_else(|| Status::not_found(format!("Snapshot '{}' not found", snapshot_name)))?;

        if !snapshot.is_snapshot {
            return Err(Status::invalid_argument(format!(
                "'{}' is not a snapshot",
                snapshot_name
            )));
        }

        // Parse the file path and navigate to the file inode
        let snapshot_root = snapshot.root_inode;
        let path_parts: Vec<&str> = file_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        tracing::info!(
            "Reading file from snapshot '{}' root inode {}: {:?}",
            snapshot_name,
            snapshot_root,
            path_parts
        );

        // Navigate through the directory tree to find the file
        let mut current_inode = snapshot_root;
        let fs_ref = self.fs.clone();

        // Navigate to the file
        for part in &path_parts {
            let inode = fs_ref.inode_store.get(current_inode).await.map_err(|e| {
                Status::internal(format!("Failed to read inode {}: {}", current_inode, e))
            })?;

            tracing::info!(
                "Looking up '{}' in inode {} (type: {:?})",
                part,
                current_inode,
                match &inode {
                    Inode::Directory(_) => "Dir",
                    Inode::File(_) => "File",
                    _ => "Other",
                }
            );

            match inode {
                Inode::Directory(_) => {
                    // Look up the next component in the directory
                    tracing::info!(
                        "Attempting directory_store.get(dir_id={}, name='{}')",
                        current_inode,
                        part
                    );
                    current_inode = fs_ref
                        .directory_store
                        .get(current_inode, part.as_bytes())
                        .await
                        .map_err(|e| {
                            tracing::error!(
                                "Failed to find '{}' in directory {}: {}",
                                part,
                                current_inode,
                                e
                            );
                            Status::not_found(format!("Path component '{}' not found: {}", part, e))
                        })?;
                    tracing::info!("Found '{}' -> inode {}", part, current_inode);
                }
                _ => {
                    return Err(Status::invalid_argument(format!(
                        "'{}' is not a directory",
                        part
                    )));
                }
            }
        }

        // Now current_inode should be the file inode
        let file_inode = fs_ref
            .inode_store
            .get(current_inode)
            .await
            .map_err(|e| Status::internal(format!("Failed to read file inode: {}", e)))?;

        let total_size = match file_inode {
            Inode::File(file) => file.size,
            _ => {
                return Err(Status::invalid_argument("Path does not point to a file"));
            }
        };

        // Create a stream that reads the file in chunks
        const CHUNK_SIZE: u64 = 4 * 1024 * 1024; // 4MB
        let num_chunks = (total_size + CHUNK_SIZE - 1) / CHUNK_SIZE;

        let file_id = current_inode;
        let fs_clone = fs_ref.clone();

        let stream = tokio_stream::iter(0..num_chunks).then(move |chunk_idx| {
            let fs = fs_clone.clone();
            let fid = file_id;
            let ts = total_size;
            async move {
                let offset = chunk_idx * CHUNK_SIZE;
                let read_size = std::cmp::min(CHUNK_SIZE, ts - offset);

                // Use root auth context for snapshot reading
                let auth = crate::fs::types::AuthContext {
                    uid: 0,
                    gid: 0,
                    gids: vec![],
                };

                match fs.read_file(&auth, fid, offset, read_size as u32).await {
                    Ok((data, _eof)) => Ok(proto::FileChunk {
                        data: data.to_vec(),
                        offset,
                        total_size: ts,
                    }),
                    Err(e) => Err(Status::internal(format!(
                        "Failed to read file chunk at offset {}: {}",
                        offset, e
                    ))),
                }
            }
        });

        Ok(Response::new(
            Box::pin(stream) as Self::ReadSnapshotFileStream
        ))
    }

    async fn instant_restore_file(
        &self,
        request: Request<proto::InstantRestoreFileRequest>,
    ) -> Result<Response<proto::InstantRestoreFileResponse>, Status> {
        use crate::fs::inode::Inode;
        use crate::fs::types::AuthContext;

        let req = request.into_inner();
        let snapshot_name = req.snapshot_name;
        let source_path = req.source_path;
        let destination_path = req.destination_path;

        // CRITICAL: Flush writeback cache before instant restore to ensure consistency
        // This ensures:
        // 1. Recently deleted files are actually deleted in backend
        // 2. Recently created files are visible in snapshots
        // 3. directory_store.exists() sees accurate state
        tracing::info!("Flushing writeback cache before instant restore...");
        if let Some(ref cache) = self.fs.writeback_cache {
            cache.flush_now(self.fs.db.as_ref()).await.map_err(|e| {
                Status::internal(format!("Failed to flush writeback cache: {}", e))
            })?;
            tracing::info!("Writeback cache flushed successfully");
        }

        // Get snapshot info
        let snapshot = self
            .snapshot_manager
            .get_dataset_by_name(&snapshot_name)
            .await
            .ok_or_else(|| Status::not_found(format!("Snapshot '{}' not found", snapshot_name)))?;

        if !snapshot.is_snapshot {
            return Err(Status::invalid_argument(format!(
                "'{}' is not a snapshot",
                snapshot_name
            )));
        }

        // Navigate to source file in snapshot
        let snapshot_root = snapshot.root_inode;
        let source_parts: Vec<&str> = source_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if source_parts.is_empty() {
            return Err(Status::invalid_argument("Source path cannot be empty"));
        }

        tracing::info!(
            "Instant restore: snapshot '{}' root inode {}: source={:?}, dest={}",
            snapshot_name,
            snapshot_root,
            source_parts,
            destination_path
        );

        let mut current_inode = snapshot_root;
        let fs_ref = self.fs.clone();

        // Navigate to the parent directory (all parts except the last, which is the filename)
        let dir_parts = &source_parts[0..source_parts.len() - 1];
        let filename = source_parts[source_parts.len() - 1];

        for part in dir_parts {
            let inode = fs_ref.inode_store.get(current_inode).await.map_err(|e| {
                Status::internal(format!("Failed to read inode {}: {}", current_inode, e))
            })?;

            match inode {
                Inode::Directory(_) => {
                    current_inode = fs_ref
                        .directory_store
                        .get(current_inode, part.as_bytes())
                        .await
                        .map_err(|e| {
                            Status::not_found(format!("Path component '{}' not found: {}", part, e))
                        })?;
                }
                _ => {
                    return Err(Status::invalid_argument(format!(
                        "'{}' is not a directory",
                        part
                    )));
                }
            }
        }

        // Now look up the filename in the parent directory
        tracing::info!(
            "Looking up filename '{}' in snapshot dir inode {}",
            filename,
            current_inode
        );
        
        current_inode = fs_ref
            .directory_store
            .get(current_inode, filename.as_bytes())
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to find '{}' in snapshot dir {}: {:?}",
                    filename,
                    current_inode,
                    e
                );
                Status::not_found(format!("File '{}' not found in directory: {}", filename, e))
            })?;

        // current_inode is now the source file inode (from snapshot)
        let source_file_inode = fs_ref
            .inode_store
            .get(current_inode)
            .await
            .map_err(|e| Status::internal(format!("Failed to read file inode: {}", e)))?;

        let (file_size, file_nlink) = match &source_file_inode {
            Inode::File(file) => (file.size, file.nlink),
            _ => {
                return Err(Status::invalid_argument(
                    "Source path does not point to a file",
                ));
            }
        };

        tracing::info!(
            "Source file inode {} size={} nlink={} - will copy to new inode in root dataset",
            current_inode,
            file_size,
            file_nlink
        );

        // Parse destination path to get directory and filename
        let dest_parts: Vec<&str> = destination_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        if dest_parts.is_empty() {
            return Err(Status::invalid_argument(
                "Destination path must include a filename",
            ));
        }

        let filename = dest_parts.last().unwrap();
        let dir_parts = &dest_parts[..dest_parts.len() - 1];

        // Navigate to destination directory (start from root dataset)
        let root_subvol = self
            .snapshot_manager
            .get_dataset_by_name("root")
            .await
            .ok_or_else(|| Status::internal("Root dataset not found"))?;

        let mut dest_dir_inode = root_subvol.root_inode;

        for part in dir_parts {
            let inode = fs_ref.inode_store.get(dest_dir_inode).await.map_err(|e| {
                Status::internal(format!("Failed to read inode {}: {}", dest_dir_inode, e))
            })?;

            match inode {
                Inode::Directory(_) => {
                    dest_dir_inode = fs_ref
                        .directory_store
                        .get(dest_dir_inode, part.as_bytes())
                        .await
                        .map_err(|e| {
                            Status::not_found(format!(
                                "Destination directory component '{}' not found: {}",
                                part, e
                            ))
                        })?;
                }
                _ => {
                    return Err(Status::invalid_argument(format!(
                        "'{}' is not a directory",
                        part
                    )));
                }
            }
        }

        // Use root auth context for instant restore
        let auth = AuthContext {
            uid: 0,
            gid: 0,
            gids: vec![],
        };

        // INSTANT RESTORE: Create a new inode in the root dataset that shares data with the snapshot inode
        // This is COW - the data chunks are shared until modified
        tracing::info!(
            "Creating new inode in root dataset that shares data chunks with snapshot inode {}",
            current_inode
        );
        
        // Allocate a new inode ID in the root dataset
        let new_inode_id = fs_ref.inode_store.allocate();
        
        // Clone the file inode but with new metadata for the root dataset
        let mut new_file_inode = source_file_inode.clone();
        if let Inode::File(ref mut file) = new_file_inode {
            file.nlink = 1; // New inode starts with nlink=1
            file.parent = Some(dest_dir_inode);
            file.name = Some(filename.as_bytes().to_vec());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            file.ctime = now;
            file.ctime_nsec = 0;
        }
        
        // Create transaction to add the new inode
        let mut txn = fs_ref.db.new_transaction().map_err(|e| {
            Status::internal(format!("Failed to create transaction: {}", e))
        })?;
        
        // Allocate directory cookie
        let cookie = fs_ref
            .directory_store
            .allocate_cookie(dest_dir_inode, &mut txn)
            .await
            .map_err(|e| Status::internal(format!("Failed to allocate cookie: {}", e)))?;
        
        // Add directory entry
        fs_ref.directory_store.add(
            &mut txn,
            dest_dir_inode,
            filename.as_bytes(),
            new_inode_id,
            cookie,
            Some(&new_file_inode),
        );
        
        // Save the new inode
        fs_ref
            .inode_store
            .save(&mut txn, new_inode_id, &new_file_inode)
            .map_err(|e| Status::internal(format!("Failed to save inode: {}", e)))?;
        
        // Update parent directory metadata
        let mut dest_dir_inode_obj = fs_ref
            .inode_store
            .get(dest_dir_inode)
            .await
            .map_err(|e| Status::internal(format!("Failed to get dest dir inode: {}", e)))?;
        
        if let Inode::Directory(ref mut dir) = dest_dir_inode_obj {
            dir.entry_count += 1;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            dir.mtime = now;
            dir.mtime_nsec = 0;
            dir.ctime = now;
            dir.ctime_nsec = 0;
        }
        
        fs_ref
            .inode_store
            .save(&mut txn, dest_dir_inode, &dest_dir_inode_obj)
            .map_err(|e| Status::internal(format!("Failed to save dest dir: {}", e)))?;
        
        // Commit with bypass cache for immediate visibility
        let mut seq_guard = fs_ref.write_coordinator.allocate_sequence();
        fs_ref
            .commit_transaction_internal(txn, &mut seq_guard, true)
            .await
            .map_err(|e| Status::internal(format!("Failed to commit: {}", e)))?;
        
        tracing::info!(
            "Instant restore complete: created new inode {} sharing data with snapshot inode {}, size {}, in dir {}",
            new_inode_id,
            current_inode,
            file_size,
            dest_dir_inode
        );

        Ok(Response::new(proto::InstantRestoreFileResponse {
            inode_id: new_inode_id,
            file_size,
            nlink: 1, // New file starts with nlink=1
        }))
    }
}

/// Serve gRPC over TCP
pub async fn serve_tcp(
    addr: SocketAddr,
    service: AdminRpcServer,
    shutdown: CancellationToken,
) -> Result<()> {
    info!("RPC server listening on {}", addr);

    let grpc_service = proto::admin_service_server::AdminServiceServer::new(service);

    tonic::transport::Server::builder()
        .add_service(grpc_service)
        .serve_with_shutdown(addr, shutdown.cancelled_owned())
        .await
        .with_context(|| format!("Failed to run RPC TCP server on {}", addr))?;

    info!("RPC TCP server shutting down on {}", addr);
    Ok(())
}

/// Serve gRPC over Unix socket
pub async fn serve_unix(
    socket_path: PathBuf,
    service: AdminRpcServer,
    shutdown: CancellationToken,
) -> Result<()> {
    // Remove existing socket file if present
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("Failed to remove existing socket file: {:?}", socket_path))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("Failed to bind RPC Unix socket to {:?}", socket_path))?;

    info!("RPC server listening on Unix socket: {:?}", socket_path);

    let uds_stream = UnixListenerStream::new(listener);

    let grpc_service = proto::admin_service_server::AdminServiceServer::new(service);

    tonic::transport::Server::builder()
        .add_service(grpc_service)
        .serve_with_incoming_shutdown(uds_stream, shutdown.cancelled_owned())
        .await
        .with_context(|| format!("Failed to run RPC Unix socket server on {:?}", socket_path))?;

    info!("RPC Unix socket server shutting down at {:?}", socket_path);
    Ok(())
}
