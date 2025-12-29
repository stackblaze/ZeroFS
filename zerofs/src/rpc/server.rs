use crate::checkpoint_manager::CheckpointManager;
use crate::fs::ZeroFS;
use crate::fs::clone;
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
    tracer: AccessTracer,
    fs: Arc<ZeroFS>,
}

impl AdminRpcServer {
    pub fn new(
        checkpoint_manager: Arc<CheckpointManager>,
        tracer: AccessTracer,
        fs: Arc<ZeroFS>,
    ) -> Self {
        Self {
            checkpoint_manager,
            tracer,
            fs,
        }
    }

    /// Recursively clone directory contents
    /// This performs true COW - all inodes and data chunks are shared until modified
    async fn clone_directory_recursive(
        &self,
        source_dir_inode: u64,
        dest_dir_inode: u64,
    ) -> Result<(), Status> {
        clone::clone_directory_deep(
            self.fs.db.clone(),
            &self.fs.inode_store,
            &self.fs.directory_store,
            &self.fs.chunk_store,
            source_dir_inode,
            dest_dir_inode,
        )
        .await
        .map_err(|e| Status::internal(format!("Failed to clone directory: {}", e)))?;

        Ok(())
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
        _request: Request<proto::CreateDatasetRequest>,
    ) -> Result<Response<proto::CreateDatasetResponse>, Status> {
        return Err(Status::unimplemented("Dataset management not implemented. Use clone command instead."));
    }

    async fn list_datasets(
        &self,
        _request: Request<proto::ListDatasetsRequest>,
    ) -> Result<Response<proto::ListDatasetsResponse>, Status> {
        return Err(Status::unimplemented("Dataset management not needed for clone-only"));
    }

    async fn delete_dataset(
        &self,
        _request: Request<proto::DeleteDatasetRequest>,
    ) -> Result<Response<proto::DeleteDatasetResponse>, Status> {
        return Err(Status::unimplemented("Dataset management not needed for clone-only"));
    }

    async fn get_dataset_info(
        &self,
        _request: Request<proto::GetDatasetInfoRequest>,
    ) -> Result<Response<proto::GetDatasetInfoResponse>, Status> {
        return Err(Status::unimplemented("Dataset management not needed for clone-only"));
    }

    async fn set_default_dataset(
        &self,
        _request: Request<proto::SetDefaultDatasetRequest>,
    ) -> Result<Response<proto::SetDefaultDatasetResponse>, Status> {
        return Err(Status::unimplemented("Dataset management not needed for clone-only"));
    }

    async fn get_default_dataset(
        &self,
        _request: Request<proto::GetDefaultDatasetRequest>,
    ) -> Result<Response<proto::GetDefaultDatasetResponse>, Status> {
        return Err(Status::unimplemented("Dataset management not needed for clone-only"));
    }

    async fn create_snapshot(
        &self,
        _request: Request<proto::CreateSnapshotRequest>,
    ) -> Result<Response<proto::CreateSnapshotResponse>, Status> {
        return Err(Status::unimplemented("Use clone command instead: zerofs dataset clone"));
    }

    async fn list_snapshots(
        &self,
        _request: Request<proto::ListSnapshotsRequest>,
    ) -> Result<Response<proto::ListSnapshotsResponse>, Status> {
        return Err(Status::unimplemented("Clones are just directories - use ls /snapshots/"));
    }

    async fn delete_snapshot(
        &self,
        _request: Request<proto::DeleteSnapshotRequest>,
    ) -> Result<Response<proto::DeleteSnapshotResponse>, Status> {
        return Err(Status::unimplemented("Clones are just directories - delete with rm -rf"));
    }

    type ReadSnapshotFileStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<proto::FileChunk, Status>> + Send>>;

    async fn read_snapshot_file(
        &self,
        _request: Request<proto::ReadSnapshotFileRequest>,
    ) -> Result<Response<Self::ReadSnapshotFileStream>, Status> {
        return Err(Status::unimplemented("Use clone command instead: zerofs dataset clone"));
    }

    async fn instant_restore_file(
        &self,
        _request: Request<proto::InstantRestoreFileRequest>,
    ) -> Result<Response<proto::InstantRestoreFileResponse>, Status> {
        return Err(Status::unimplemented("Use clone command instead: zerofs dataset clone"));
    }

    async fn clone_path(
        &self,
        request: Request<proto::ClonePathRequest>,
    ) -> Result<Response<proto::ClonePathResponse>, Status> {
        use crate::fs::inode::Inode;

        let req = request.into_inner();
        let source_path = req.source_path;
        let destination_path = req.destination_path;

        tracing::info!(
            "COW clone: source='{}', dest='{}'",
            source_path,
            destination_path
        );

        // Parse paths
        let source_parts: Vec<&str> = source_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        let dest_parts: Vec<&str> = destination_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if source_parts.is_empty() || dest_parts.is_empty() {
            return Err(Status::invalid_argument(
                "Source and destination paths cannot be empty",
            ));
        }

        let fs_ref = &self.fs;

        // Navigate to source
        let mut current_inode = 0u64; // root
        for part in &source_parts {
            let inode = fs_ref.inode_store.get(current_inode).await.map_err(|e| {
                Status::internal(format!("Failed to read inode {}: {}", current_inode, e))
            })?;

            match inode {
                Inode::Directory(_) => {
                    current_inode = fs_ref
                        .directory_store
                        .get(current_inode, part.as_bytes())
                        .await
                        .map_err(|_| {
                            Status::not_found(format!("Source path component '{}' not found", part))
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

        // Get source inode
        let source_inode = fs_ref.inode_store.get(current_inode).await.map_err(|e| {
            Status::not_found(format!("Source inode {} not found: {}", current_inode, e))
        })?;

        let is_directory = matches!(source_inode, Inode::Directory(_));
        let size = match &source_inode {
            Inode::File(f) => f.size,
            _ => 0,
        };

        // Navigate to destination parent directory
        let dest_name = dest_parts.last().unwrap();
        let dest_dir_parts = &dest_parts[..dest_parts.len() - 1];

        let mut dest_dir_inode = 0u64; // root
        for part in dest_dir_parts {
            let inode = fs_ref
                .inode_store
                .get(dest_dir_inode)
                .await
                .map_err(|e| {
                    Status::internal(format!("Failed to read inode {}: {}", dest_dir_inode, e))
                })?;

            match inode {
                Inode::Directory(_) => {
                    dest_dir_inode = fs_ref
                        .directory_store
                        .get(dest_dir_inode, part.as_bytes())
                        .await
                        .map_err(|_| {
                            Status::not_found(format!(
                                "Destination path component '{}' not found",
                                part
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

        // Check if destination already exists
        if fs_ref
            .directory_store
            .exists(dest_dir_inode, dest_name.as_bytes())
            .await
            .map_err(|e| Status::internal(format!("Failed to check destination: {}", e)))?
        {
            return Err(Status::already_exists(format!(
                "Destination '{}' already exists",
                destination_path
            )));
        }

        // Allocate new inode for the clone
        let new_inode_id = fs_ref.inode_store.allocate();

        // Clone the inode (COW - shares data chunks)
        let mut new_inode = source_inode.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Update timestamps for the clone
        match &mut new_inode {
            Inode::File(f) => {
                f.ctime = now;
                f.ctime_nsec = 0;
                f.mtime = now;
                f.mtime_nsec = 0;
                f.atime = now;
                f.atime_nsec = 0;
            }
            Inode::Directory(d) => {
                d.ctime = now;
                d.ctime_nsec = 0;
                d.mtime = now;
                d.mtime_nsec = 0;
                d.atime = now;
                d.atime_nsec = 0;
            }
            _ => {}
        }

        // Create transaction
        let mut txn = fs_ref
            .db
            .new_transaction()
            .map_err(|e| Status::internal(format!("Failed to create transaction: {}", e)))?;

        // Allocate cookie for directory entry
        let cookie = fs_ref
            .directory_store
            .allocate_cookie(dest_dir_inode, &mut txn)
            .await
            .map_err(|e| Status::internal(format!("Failed to allocate cookie: {}", e)))?;

        // Add directory entry
        fs_ref.directory_store.add(
            &mut txn,
            dest_dir_inode,
            dest_name.as_bytes(),
            new_inode_id,
            cookie,
            Some(&new_inode),
        );

        // Save the new inode
        fs_ref
            .inode_store
            .save(&mut txn, new_inode_id, &new_inode)
            .map_err(|e| Status::internal(format!("Failed to save inode: {}", e)))?;

        // Commit transaction
        let mut seq_guard = fs_ref.write_coordinator.allocate_sequence();
        fs_ref
            .commit_transaction_internal(txn, &mut seq_guard, true)
            .await
            .map_err(|e| Status::internal(format!("Failed to commit: {}", e)))?;

        // For files, copy chunk metadata (COW)
        if !is_directory && size > 0 {
            tracing::info!(
                "Copying chunks from source inode {} to cloned inode {} (file_size={})",
                current_inode,
                new_inode_id,
                size
            );
            fs_ref
                .chunk_store
                .copy_chunks_for_cow(current_inode, new_inode_id, size)
                .await
                .map_err(|e| Status::internal(format!("Failed to copy chunks: {}", e)))?;
            
            // Flush to ensure chunks are persisted
            if let Err(e) = fs_ref.db.flush().await {
                tracing::warn!("Failed to flush after file clone: {}", e);
            }
        }

        // If it's a directory, recursively clone its contents
        if is_directory {
            tracing::info!(
                "Recursively cloning directory contents from source inode {} to new inode {}",
                current_inode,
                new_inode_id
            );
            
            self.clone_directory_recursive(current_inode, new_inode_id)
                .await?;
            
            // Flush to ensure all cloned directory entries are persisted
            if let Err(e) = fs_ref.db.flush().await {
                tracing::warn!("Failed to flush after directory clone: {}", e);
                // Continue anyway - entries are written, just not guaranteed durable yet
            }
                
            tracing::info!(
                "Completed recursive clone of directory from {} to {}",
                current_inode,
                new_inode_id
            );
        }

        tracing::info!(
            "COW clone complete: created new inode {} (type: {}) from source inode {}",
            new_inode_id,
            if is_directory { "directory" } else { "file" },
            current_inode
        );

        Ok(Response::new(proto::ClonePathResponse {
            inode_id: new_inode_id,
            size,
            is_directory,
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
