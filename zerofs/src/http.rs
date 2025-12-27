// REST API server for ZeroFS - wraps gRPC calls for Kubernetes CSI integration
use crate::config::HttpConfig;
use crate::rpc::client::RpcClient;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Clone)]
struct AppState {
    rpc_config: crate::config::RpcConfig,
}

// Request/Response types for REST API
#[derive(Debug, Deserialize)]
struct CreateDatasetRequest {
    name: String,
}

#[derive(Debug, Serialize)]
struct DatasetResponse {
    id: u64,
    name: String,
    uuid: String,
    created_at: u64,
    root_inode: u64,
    is_readonly: bool,
    is_snapshot: bool,
}

#[derive(Debug, Serialize)]
struct ListDatasetsResponse {
    datasets: Vec<DatasetResponse>,
}

#[derive(Debug, Deserialize)]
struct CreateSnapshotRequest {
    /// Source dataset name (e.g., "root") - NOT a path. ZeroFS snapshots entire datasets.
    source: String,
    /// Snapshot name (must be unique)
    name: String,
    /// Create read-only snapshot (default: false, read-write like btrfs)
    #[serde(default)]
    readonly: bool,
}

#[derive(Debug, Serialize)]
struct SnapshotResponse {
    id: u64,
    name: String,
    uuid: String,
    source: String,
    created_at: u64,
    readonly: bool,
}

#[derive(Debug, Serialize)]
struct ListSnapshotsResponse {
    snapshots: Vec<SnapshotResponse>,
}

#[derive(Debug, Deserialize)]
struct RestoreRequest {
    snapshot: String,
    source: String,
    destination: String,
}

#[derive(Debug, Serialize)]
struct RestoreResponse {
    inode_id: u64,
    file_size: u64,
    message: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

// Helper to create RPC client
async fn get_rpc_client(state: &AppState) -> Result<RpcClient, (StatusCode, Json<ErrorResponse>)> {
    tracing::debug!(
        "Attempting to connect to RPC server: unix_socket={:?}, addresses={:?}",
        state.rpc_config.unix_socket,
        state.rpc_config.addresses
    );
    
    RpcClient::connect_from_config(&state.rpc_config)
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to connect to RPC server (unix_socket={:?}, addresses={:?}): {}",
                state.rpc_config.unix_socket,
                state.rpc_config.addresses,
                e
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "RPC_CONNECTION_FAILED".to_string(),
                    message: format!("Failed to connect to RPC server: {}", e),
                }),
            )
        })
}

// Health check endpoint
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

// Dataset endpoints
async fn create_dataset(
    State(state): State<AppState>,
    Json(req): Json<CreateDatasetRequest>,
) -> Result<(StatusCode, Json<DatasetResponse>), (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    let dataset = client.create_dataset(&req.name).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "CREATE_DATASET_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(DatasetResponse {
            id: dataset.id,
            name: dataset.name,
            uuid: dataset.uuid.to_string(),
            created_at: dataset.created_at,
            root_inode: dataset.root_inode,
            is_readonly: dataset.is_readonly,
            is_snapshot: dataset.is_snapshot,
        }),
    ))
}

async fn list_datasets(
    State(state): State<AppState>,
) -> Result<Json<ListDatasetsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    let datasets = client.list_datasets().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "LIST_DATASETS_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    Ok(Json(ListDatasetsResponse {
        datasets: datasets
            .into_iter()
            .map(|d| DatasetResponse {
                id: d.id,
                name: d.name,
                uuid: d.uuid.to_string(),
                created_at: d.created_at,
                root_inode: d.root_inode,
                is_readonly: d.is_readonly,
                is_snapshot: d.is_snapshot,
            })
            .collect(),
    }))
}

async fn get_dataset(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<DatasetResponse>, (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    let dataset = client.get_dataset_info(&name).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "GET_DATASET_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    let dataset = dataset.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "DATASET_NOT_FOUND".to_string(),
                message: format!("Dataset '{}' not found", name),
            }),
        )
    })?;

    Ok(Json(DatasetResponse {
        id: dataset.id,
        name: dataset.name,
        uuid: dataset.uuid.to_string(),
        created_at: dataset.created_at,
        root_inode: dataset.root_inode,
        is_readonly: dataset.is_readonly,
        is_snapshot: dataset.is_snapshot,
    }))
}

async fn delete_dataset(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    client.delete_dataset(&name).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "DELETE_DATASET_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
}

// Snapshot endpoints
async fn create_snapshot(
    State(state): State<AppState>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<(StatusCode, Json<SnapshotResponse>), (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    // Validate that source is a dataset name, not a path
    if req.source.starts_with('/') {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "INVALID_SOURCE".to_string(),
                message: format!(
                    "Source must be a dataset name (e.g., 'root'), not a path. Got: '{}'. Use GET /api/v1/datasets to list available datasets.",
                    req.source
                ),
            }),
        ));
    }

    let snapshot = client
        .create_snapshot_with_options(&req.source, &req.name, req.readonly)
        .await
        .map_err(|e| {
            let error_msg = if e.to_string().contains("Not found") || e.to_string().contains("not found") {
                format!(
                    "Dataset '{}' not found. Use GET /api/v1/datasets to list available datasets. Note: ZeroFS snapshots entire datasets, not paths within datasets.",
                    req.source
                )
            } else {
                e.to_string()
            };
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "CREATE_SNAPSHOT_FAILED".to_string(),
                    message: error_msg,
                }),
            )
        })?;

    Ok((
        StatusCode::CREATED,
        Json(SnapshotResponse {
            id: snapshot.id,
            name: snapshot.name,
            uuid: snapshot.uuid.to_string(),
            source: req.source,
            created_at: snapshot.created_at,
            readonly: snapshot.is_readonly,
        }),
    ))
}

async fn list_snapshots(
    State(state): State<AppState>,
) -> Result<Json<ListSnapshotsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    let snapshots = client.list_snapshots().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "LIST_SNAPSHOTS_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    // Get datasets to map parent IDs to names
    let datasets = client.list_datasets().await.unwrap_or_default();
    let id_to_name: HashMap<u64, String> = datasets
        .into_iter()
        .map(|d| (d.id, d.name))
        .collect();

    Ok(Json(ListSnapshotsResponse {
        snapshots: snapshots
            .into_iter()
            .map(|s| SnapshotResponse {
                id: s.id,
                name: s.name,
                uuid: s.uuid.to_string(),
                source: s
                    .parent_id
                    .and_then(|id| id_to_name.get(&id).cloned())
                    .unwrap_or_else(|| format!("ID:{}", s.parent_id.unwrap_or(0))),
                created_at: s.created_at,
                readonly: s.is_readonly,
            })
            .collect(),
    }))
}

async fn get_snapshot(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<SnapshotResponse>, (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    let snapshot = client.get_dataset_info(&name).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "GET_SNAPSHOT_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    let snapshot = snapshot.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "SNAPSHOT_NOT_FOUND".to_string(),
                message: format!("Snapshot '{}' not found", name),
            }),
        )
    })?;

    if !snapshot.is_snapshot {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "NOT_A_SNAPSHOT".to_string(),
                message: format!("'{}' is not a snapshot", name),
            }),
        ));
    }

    // Get datasets to find source name
    let datasets = client.list_datasets().await.unwrap_or_default();
    let id_to_name: HashMap<u64, String> = datasets
        .into_iter()
        .map(|d| (d.id, d.name))
        .collect();

    Ok(Json(SnapshotResponse {
        id: snapshot.id,
        name: snapshot.name,
        uuid: snapshot.uuid.to_string(),
        source: snapshot
            .parent_id
            .and_then(|id| id_to_name.get(&id).cloned())
            .unwrap_or_else(|| format!("ID:{}", snapshot.parent_id.unwrap_or(0))),
        created_at: snapshot.created_at,
        readonly: snapshot.is_readonly,
    }))
}

async fn delete_snapshot(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;
    client.delete_snapshot(&name).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "DELETE_SNAPSHOT_FAILED".to_string(),
                message: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::NO_CONTENT)
}

async fn restore_from_snapshot(
    State(state): State<AppState>,
    Json(req): Json<RestoreRequest>,
) -> Result<(StatusCode, Json<RestoreResponse>), (StatusCode, Json<ErrorResponse>)> {
    let client = get_rpc_client(&state).await?;

    // Check if destination is internal (for instant restore) or external (copy-based)
    let is_internal = !req.destination.starts_with("/tmp/")
        && !req.destination.starts_with("/home/")
        && !req.destination.starts_with("/root/")
        && req.destination.starts_with('/');

    if is_internal {
        // Instant restore (COW)
        let (inode_id, file_size, _nlink) = client
            .instant_restore_file(&req.snapshot, &req.source, &req.destination)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "INSTANT_RESTORE_FAILED".to_string(),
                        message: e.to_string(),
                    }),
                )
            })?;

        Ok((
            StatusCode::OK,
            Json(RestoreResponse {
                inode_id,
                file_size,
                message: format!(
                    "File restored instantly (COW) - no data copied. Inode: {}, Size: {} bytes",
                    inode_id, file_size
                ),
            }),
        ))
    } else {
        // Copy-based restore
        let file_data = client
            .read_snapshot_file(&req.snapshot, &req.source)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "READ_SNAPSHOT_FILE_FAILED".to_string(),
                        message: e.to_string(),
                    }),
                )
            })?;

        tokio::fs::write(&req.destination, &file_data)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "WRITE_DESTINATION_FAILED".to_string(),
                        message: format!("Failed to write to {}: {}", req.destination, e),
                    }),
                )
            })?;

        Ok((
            StatusCode::OK,
            Json(RestoreResponse {
                inode_id: 0,
                file_size: file_data.len() as u64,
                message: format!(
                    "File restored (copy-based). Size: {} bytes",
                    file_data.len()
                ),
            }),
        ))
    }
}

pub fn create_router(rpc_config: crate::config::RpcConfig) -> Router {
    let state = AppState { rpc_config };

    Router::new()
        .route("/health", get(health))
        .route("/api/v1/datasets", post(create_dataset))
        .route("/api/v1/datasets", get(list_datasets))
        .route("/api/v1/datasets/{name}", get(get_dataset))
        .route("/api/v1/datasets/{name}", delete(delete_dataset))
        .route("/api/v1/snapshots", post(create_snapshot))
        .route("/api/v1/snapshots", get(list_snapshots))
        .route("/api/v1/snapshots/{name}", get(get_snapshot))
        .route("/api/v1/snapshots/{name}", delete(delete_snapshot))
        .route("/api/v1/snapshots/restore", post(restore_from_snapshot))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn start_http_servers(
    config: Option<&HttpConfig>,
    rpc_config: crate::config::RpcConfig,
    shutdown: CancellationToken,
) -> Vec<JoinHandle<Result<(), std::io::Error>>> {
    let config = match config {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut handles = Vec::new();

    if let Some(addresses) = &config.addresses {
        for &addr in addresses {
            info!("Starting HTTP REST API server on {}", addr);
            let router = create_router(rpc_config.clone());
            let shutdown_rx = shutdown.clone().cancelled_owned();
            handles.push(tokio::spawn(async move {
                let listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .map_err(|e| std::io::Error::other(format!("Failed to bind HTTP server: {}", e)))?;

                axum::serve(listener, router)
                    .with_graceful_shutdown(async {
                        shutdown_rx.await;
                    })
                    .await
                    .map_err(|e| std::io::Error::other(format!("HTTP server error: {}", e)))?;

                info!("HTTP REST API server shutting down on {}", addr);
                Ok(())
            }));
        }
    }

    handles
}

