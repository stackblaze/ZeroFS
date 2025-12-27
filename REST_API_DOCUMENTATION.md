# ZeroFS REST API Documentation

## Overview

ZeroFS REST API provides HTTP endpoints for managing datasets and snapshots, designed for Kubernetes CSI driver integration.

**Version:** v0.22.8 (with REST API support)  
**Base URL:** `http://<zerofs-server>:8080`  
**Content-Type:** `application/json`

---

## Important Concepts

### Datasets vs Paths

**ZeroFS snapshots entire datasets, not individual paths.**

- A **dataset** is a logical container with its own root directory
- The default dataset is named `"root"` (ID: 0)
- Volumes (e.g., `/volumes/pvc-xxx`) are **subdirectories** within a dataset
- To snapshot a volume, you snapshot the **entire dataset** that contains it

### For Kubernetes CSI Integration

**Recommended approach (Efficient):**
- Create a **separate dataset for each PVC/volume**
- When PVC is created: `POST /api/v1/datasets` with `{"name": "pvc-uuid"}`
- Create volume directory within that dataset
- Snapshot individual datasets: `{"source": "pvc-uuid", "name": "snapshot-name"}`
- **Benefits:** Only snapshot the volume you need, better isolation, easier management

**Alternative approach (Simpler but less efficient):**
- Use the `"root"` dataset for all volumes
- Volumes are subdirectories: `/volumes/pvc-xxx`, `/volumes/pvc-yyy`, etc.
- Create snapshots of the `"root"` dataset to capture all volumes
- **Note:** This snapshots ALL volumes, not just one
- Use instant restore to restore specific files/directories from snapshots

---

## Endpoints

### Health Check

```http
GET /health
```

**Response:**
```json
{
  "status": "ok",
  "version": "0.22.8"
}
```

**Status Codes:**
- `200 OK` - Server is healthy

---

### List Datasets

```http
GET /api/v1/datasets
```

**Response:**
```json
{
  "datasets": [
    {
      "id": 0,
      "name": "root",
      "uuid": "54988e7a-72b7-49a7-a149-80d114758ac6",
      "created_at": 1766506929,
      "root_inode": 0,
      "is_readonly": false,
      "is_snapshot": false
    }
  ]
}
```

**Status Codes:**
- `200 OK` - Success

---

### Get Dataset Info

```http
GET /api/v1/datasets/{name}
```

**Parameters:**
- `name` (path) - Dataset name (e.g., `"root"`)

**Response:**
```json
{
  "id": 0,
  "name": "root",
  "uuid": "54988e7a-72b7-49a7-a149-80d114758ac6",
  "created_at": 1766506929,
  "root_inode": 0,
  "is_readonly": false,
  "is_snapshot": false
}
```

**Status Codes:**
- `200 OK` - Success
- `404 Not Found` - Dataset not found

---

### Create Dataset

```http
POST /api/v1/datasets
Content-Type: application/json

{
  "name": "my-dataset"
}
```

**Request Body:**
- `name` (string, required) - Unique dataset name

**Response:**
```json
{
  "id": 1,
  "name": "my-dataset",
  "uuid": "uuid-here",
  "created_at": 1766506929,
  "root_inode": 123,
  "is_readonly": false,
  "is_snapshot": false
}
```

**Status Codes:**
- `201 Created` - Dataset created
- `500 Internal Server Error` - Creation failed

---

### Delete Dataset

```http
DELETE /api/v1/datasets/{name}
```

**Status Codes:**
- `204 No Content` - Success
- `500 Internal Server Error` - Deletion failed

---

### Create Snapshot

```http
POST /api/v1/snapshots
Content-Type: application/json

{
  "source": "root",
  "name": "backup-2024-12-23",
  "readonly": false
}
```

**Request Body:**
- `source` (string, required) - **Dataset name** (e.g., `"root"`), NOT a path
- `name` (string, required) - Snapshot name (must be unique)
- `readonly` (boolean, optional) - Create read-only snapshot (default: `false`)

**Important Notes:**
- `source` must be a **dataset name**, not a path like `/volumes/pvc-xxx`
- ZeroFS snapshots **entire datasets**, not individual paths
- To snapshot a volume at `/volumes/pvc-xxx`, snapshot the dataset that contains it (usually `"root"`)
- The snapshot captures the entire dataset state at creation time

**Response:**
```json
{
  "id": 1,
  "name": "backup-2024-12-23",
  "uuid": "uuid-here",
  "source": "root",
  "created_at": 1766506929,
  "readonly": false
}
```

**Status Codes:**
- `201 Created` - Snapshot created
- `400 Bad Request` - Invalid request (e.g., source is a path instead of dataset name)
- `500 Internal Server Error` - Creation failed (e.g., dataset not found)

**Error Examples:**

```json
// Error: Source is a path, not a dataset name
{
  "error": "INVALID_SOURCE",
  "message": "Source must be a dataset name (e.g., 'root'), not a path. Got: '/volumes/pvc-xxx'. Use GET /api/v1/datasets to list available datasets."
}

// Error: Dataset not found
{
  "error": "CREATE_SNAPSHOT_FAILED",
  "message": "Dataset 'nonexistent' not found. Use GET /api/v1/datasets to list available datasets. Note: ZeroFS snapshots entire datasets, not paths within datasets."
}
```

---

### List Snapshots

```http
GET /api/v1/snapshots
```

**Response:**
```json
{
  "snapshots": [
    {
      "id": 1,
      "name": "backup-2024-12-23",
      "uuid": "uuid-here",
      "source": "root",
      "created_at": 1766506929,
      "readonly": false
    }
  ]
}
```

**Status Codes:**
- `200 OK` - Success

---

### Get Snapshot Info

```http
GET /api/v1/snapshots/{name}
```

**Parameters:**
- `name` (path) - Snapshot name

**Response:**
```json
{
  "id": 1,
  "name": "backup-2024-12-23",
  "uuid": "uuid-here",
  "source": "root",
  "created_at": 1766506929,
  "readonly": false
}
```

**Status Codes:**
- `200 OK` - Success
- `404 Not Found` - Snapshot not found
- `400 Bad Request` - Resource exists but is not a snapshot

---

### Delete Snapshot

```http
DELETE /api/v1/snapshots/{name}
```

**Status Codes:**
- `204 No Content` - Success
- `500 Internal Server Error` - Deletion failed

---

### Restore from Snapshot

```http
POST /api/v1/snapshots/restore
Content-Type: application/json

{
  "snapshot": "backup-2024-12-23",
  "source": "/volumes/pvc-xxx/file.txt",
  "destination": "/volumes/pvc-yyy/restored-file.txt"
}
```

**Request Body:**
- `snapshot` (string, required) - Snapshot name
- `source` (string, required) - Path within snapshot (file or directory, e.g., `/volumes/pvc-xxx/file.txt` or `/volumes/pvc-xxx/`)
- `destination` (string, required) - Destination path (file or directory)

**✅ Supports both files and directories:**
- **Files**: Instant COW restore of individual files
- **Directories**: Recursive COW restore of entire directory trees
- All data chunks are shared (copy-on-write) for space efficiency

**Restore Modes:**

1. **Instant Restore (COW)** - When destination is within ZeroFS:
   - Destination path starts with `/` but NOT `/tmp/`, `/home/`, `/root/`, etc.
   - Zero data copying (~11ms restore time)
   - Shares data chunks with snapshot (copy-on-write)
   - Example: `destination: "/volumes/pvc-yyy/file.txt"`

2. **Copy-Based Restore** - When destination is outside ZeroFS:
   - Destination path starts with `/tmp/`, `/home/`, `/root/`, etc.
   - Full data copy required
   - Example: `destination: "/tmp/restored-file.txt"`

**Response:**

Instant Restore:
```json
{
  "inode_id": 123,
  "file_size": 1024,
  "message": "File restored instantly (COW) - no data copied. Inode: 123, Size: 1024 bytes"
}
```

Copy-Based Restore:
```json
{
  "inode_id": 0,
  "file_size": 1024,
  "message": "File restored (copy-based). Size: 1024 bytes"
}
```

**Status Codes:**
- `200 OK` - Success
- `500 Internal Server Error` - Restore failed

---

## Kubernetes CSI Integration Guide

### Architecture

**Recommended: One Dataset Per Volume**
```
Kubernetes PVC
    ↓
ZeroFS Dataset (one per PVC)
    ↓
/volumes/pvc-xxx/  (NFS export, root of dataset)
```

**Alternative: All Volumes in Root Dataset**
```
Kubernetes PVC
    ↓
ZeroFS Volume (subdirectory in root dataset)
    ↓
/volumes/pvc-xxx/  (NFS export)
```

### Workflow

#### Option A: Per-Volume Dataset (Recommended - Efficient)

**1. Create Dataset When PVC is Created**

```bash
# Create a dataset for this specific PVC
curl -X POST http://zerofs-server:8080/api/v1/datasets \
  -H "Content-Type: application/json" \
  -d '{
    "name": "pvc-77a7e6ca-e3db-4d51-9920-0ab9d5b76ba2"
  }'

# Response: Dataset created with its own root inode
```

**2. Mount Dataset as Volume**

The dataset has its own root directory (root_inode). You can mount/export this dataset root directly as the volume. No need to create subdirectories - the dataset root IS the volume root.

**3. Create Snapshot of Specific Volume**

```bash
# Snapshot only this volume's dataset
curl -X POST http://zerofs-server:8080/api/v1/snapshots \
  -H "Content-Type: application/json" \
  -d '{
    "source": "pvc-77a7e6ca-e3db-4d51-9920-0ab9d5b76ba2",
    "name": "pvc-xxx-snapshot-2024-12-23",
    "readonly": false
  }'
```

**Benefits:**
- ✅ Only snapshots the volume you need
- ✅ Better isolation between volumes
- ✅ More efficient management
- ✅ COW snapshots still share data blocks (space-efficient)

#### Option B: Root Dataset (Simpler but Less Efficient)

**1. Create Volume Snapshot**

```bash
# List datasets to find the one containing your volume
curl http://zerofs-server:8080/api/v1/datasets

# Create snapshot of the root dataset (contains ALL volumes)
curl -X POST http://zerofs-server:8080/api/v1/snapshots \
  -H "Content-Type: application/json" \
  -d '{
    "source": "root",
    "name": "pvc-xxx-snapshot-2024-12-23",
    "readonly": false
  }'
```

**Note:** This snapshots ALL volumes in the root dataset, not just one. Use only if you need to snapshot multiple volumes together.

#### 2. Restore from Snapshot

**✅ Supports both files and directories**

```bash
# Restore a single file (instant COW)
curl -X POST http://zerofs-server:8080/api/v1/snapshots/restore \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot": "pvc-xxx-snapshot-2024-12-23",
    "source": "/volumes/pvc-xxx/data.txt",
    "destination": "/volumes/pvc-yyy/restored-data.txt"
  }'

# Restore entire directory (instant COW, recursive)
curl -X POST http://zerofs-server:8080/api/v1/snapshots/restore \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot": "pvc-xxx-snapshot-2024-12-23",
    "source": "/volumes/pvc-xxx",
    "destination": "/volumes/pvc-yyy"
  }'
```

#### 3. Restore Specific File

```bash
# Restore a single file (instant COW)
curl -X POST http://zerofs-server:8080/api/v1/snapshots/restore \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot": "pvc-xxx-snapshot-2024-12-23",
    "source": "/volumes/pvc-xxx/data.txt",
    "destination": "/volumes/pvc-yyy/restored-data.txt"
  }'
```

### Configuration

**zerofs.toml:**
```toml
[servers.http]
addresses = ["0.0.0.0:8080"]  # For Kubernetes, bind to all interfaces

[servers.rpc]
addresses = ["0.0.0.0:7000"]  # RPC server (required for REST API)
unix_socket = "/tmp/zerofs.rpc.sock"
```

**Kubernetes Service:**
```yaml
apiVersion: v1
kind: Service
metadata:
  name: zerofs-api
spec:
  selector:
    app: zerofs
  ports:
    - name: http
      port: 8080
      targetPort: 8080
  type: ClusterIP
```

---

## Common Errors and Solutions

### Error: "Dataset 'X' not found"

**Cause:** The `source` field contains a dataset name that doesn't exist.

**Solution:**
1. List available datasets: `GET /api/v1/datasets`
2. Use the exact dataset name (usually `"root"`)
3. Remember: `source` must be a dataset name, not a path

### Error: "Source must be a dataset name, not a path"

**Cause:** The `source` field contains a path (e.g., `/volumes/pvc-xxx`) instead of a dataset name.

**Solution:**
- Use the dataset name (e.g., `"root"`) in the `source` field
- ZeroFS snapshots entire datasets, not paths
- To snapshot a volume, snapshot the dataset that contains it

### Error: "CREATE_SNAPSHOT_FAILED: Not found"

**Cause:** The dataset specified in `source` doesn't exist.

**Solution:**
- Verify dataset exists: `GET /api/v1/datasets`
- Use the exact dataset name (case-sensitive)
- Default dataset is always `"root"`

### Error: "Source path must be a file or directory"

**Cause:** The source path points to something other than a file or directory (e.g., symlink, special file).

**Solution:**
- Ensure source path points to a regular file or directory
- Symlinks and special files are not supported for restore

---

## FAQ

### Q: Can I snapshot just `/volumes/pvc-xxx` without snapshotting the entire dataset?

**A:** No. ZeroFS snapshots entire datasets, not paths. However, you have two options:

**Option 1: Create separate dataset per volume (Recommended)**
- Create a dataset for each PVC: `POST /api/v1/datasets` with `{"name": "pvc-uuid"}`
- Snapshot that specific dataset: `{"source": "pvc-uuid", "name": "snapshot"}`
- **Benefits:** Only snapshots the volume you need, better isolation

**Option 2: Use root dataset**
- All volumes in `"root"` dataset
- Snapshot `"root"` captures all volumes
- Use restore endpoint to restore specific paths
- **Note:** Less efficient if you only need to snapshot one volume

### Q: How do I know which dataset contains my volume?

**A:** 
- By default, all volumes are in the `"root"` dataset
- List datasets: `GET /api/v1/datasets`
- If you created separate datasets, use the appropriate dataset name

### Q: What's the difference between a dataset and a snapshot?

**A:**
- A **dataset** is a writable filesystem container
- A **snapshot** is a point-in-time copy of a dataset (read-only or read-write)
- Snapshots share data blocks with the source dataset (COW)

### Q: Can I create a snapshot of a snapshot?

**A:** Yes, snapshots are datasets too. You can snapshot them like any dataset.

### Q: How do I restore an entire volume?

**A:** ✅ Use the restore endpoint with the directory path:

```json
{
  "snapshot": "snapshot-name",
  "source": "/volumes/pvc-xxx",
  "destination": "/volumes/pvc-yyy"
}
```

This will recursively restore the entire directory tree with instant COW (copy-on-write). All data chunks are shared until modified, making it space-efficient and fast.

---

## Example: Complete Kubernetes Workflow

```bash
# 1. Create a snapshot of a volume's dataset
curl -X POST http://zerofs-server:8080/api/v1/snapshots \
  -H "Content-Type: application/json" \
  -d '{
    "source": "pvc-xxx",
    "name": "pvc-xxx-backup-2024-12-23",
    "readonly": true
  }'

# 2. List snapshots
curl http://zerofs-server:8080/api/v1/snapshots

# 3a. Restore entire volume from snapshot (instant COW)
curl -X POST http://zerofs-server:8080/api/v1/snapshots/restore \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot": "pvc-xxx-backup-2024-12-23",
    "source": "/",
    "destination": "/restored-volume"
  }'

# 3b. Or restore a specific file
curl -X POST http://zerofs-server:8080/api/v1/snapshots/restore \
  -H "Content-Type: application/json" \
  -d '{
    "snapshot": "pvc-xxx-backup-2024-12-23",
    "source": "/database.db",
    "destination": "/database-restored.db"
  }'

# 4. Delete snapshot when done
curl -X DELETE http://zerofs-server:8080/api/v1/snapshots/pvc-xxx-backup-2024-12-23
```

---

## Version Information

- **API Version:** v1
- **ZeroFS Version:** 0.22.8
- **REST API Added:** December 2024
- **Last Updated:** December 23, 2024

