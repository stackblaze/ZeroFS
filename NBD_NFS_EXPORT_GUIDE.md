# NBD NFS Export Guide

## Overview

The `zerofs nbd export` command automates the complete workflow of creating an NBD device, formatting it, and exporting it via NFS. This enables you to provide clients with formatted filesystems (ext4, xfs, or ZFS) backed by S3 storage.

## Use Case

**Problem:** You want to give NFS clients access to a filesystem that supports snapshots (ZFS/btrfs), but the underlying storage is S3.

**Solution:** Use NBD to create block devices on S3, format them with your desired filesystem, then export via NFS.

```
Clients (NFS) → Server (NFS export) → Formatted FS (ext4/xfs/ZFS) → NBD → ZeroFS → S3
```

## Quick Start

### Simple Example (ext4)

```bash
# 1. Start ZeroFS server
zerofs run -c zerofs.toml &

# 2. Export NBD device as formatted ext4 via NFS (one command!)
sudo zerofs nbd export -c zerofs.toml \
  --name storage \
  --size 100G \
  --mount-point /mnt/storage \
  --filesystem ext4

# 3. On client machines, mount via NFS
sudo mount -t nfs server-ip:/mnt/storage /mnt/remote
```

That's it! The `export` command handles:
- ✅ Creating NBD device in ZeroFS
- ✅ Connecting NBD client
- ✅ Formatting as ext4
- ✅ Mounting filesystem
- ✅ Configuring NFS export

## Command Reference

### Basic Usage

```bash
zerofs nbd export -c <config> \
  --name <device-name> \
  --size <size> \
  --mount-point <path> \
  [options]
```

### Required Arguments

| Argument | Description | Example |
|----------|-------------|---------|
| `-c, --config` | ZeroFS config file | `zerofs.toml` |
| `--name` | NBD device name | `storage` |
| `--size` | Device size | `100G`, `1T` |
| `--mount-point` | Where to mount | `/mnt/storage` |

### Optional Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--nbd-device` | `/dev/nbd0` | NBD device to use |
| `--filesystem` | `ext4` | Filesystem type (ext4, xfs) |
| `--nfs-export` | (mount point) | NFS export path |
| `--nfs-options` | `rw,sync,no_subtree_check` | NFS export options |
| `--nbd-host` | `127.0.0.1` | ZeroFS NBD server address |
| `--nbd-port` | `10809` | ZeroFS NBD server port |

## Examples

### Example 1: Basic ext4 Export

```bash
sudo zerofs nbd export -c zerofs.toml \
  --name mydata \
  --size 50G \
  --mount-point /mnt/mydata
```

**What happens:**
1. Creates `.nbd/mydata` (50GB) in ZeroFS
2. Connects to `/dev/nbd0`
3. Formats as ext4
4. Mounts to `/mnt/mydata`
5. Exports `/mnt/mydata` via NFS

### Example 2: XFS Filesystem

```bash
sudo zerofs nbd export -c zerofs.toml \
  --name projects \
  --size 200G \
  --mount-point /mnt/projects \
  --filesystem xfs
```

### Example 3: Custom NFS Options

```bash
sudo zerofs nbd export -c zerofs.toml \
  --name secure \
  --size 100G \
  --mount-point /mnt/secure \
  --nfs-options "rw,sync,no_subtree_check,no_root_squash"
```

### Example 4: Multiple Devices

```bash
# Device 1
sudo zerofs nbd export -c zerofs.toml \
  --name storage1 --size 100G \
  --mount-point /mnt/storage1 \
  --nbd-device /dev/nbd0

# Device 2
sudo zerofs nbd export -c zerofs.toml \
  --name storage2 --size 100G \
  --mount-point /mnt/storage2 \
  --nbd-device /dev/nbd1

# Device 3
sudo zerofs nbd export -c zerofs.toml \
  --name storage3 --size 100G \
  --mount-point /mnt/storage3 \
  --nbd-device /dev/nbd2
```

## ZFS Setup (Advanced)

For ZFS with snapshots, you need manual setup:

```bash
# 1. Create multiple NBD devices
zerofs nbd create -c zerofs.toml --name zfs-disk1 --size 100G
zerofs nbd create -c zerofs.toml --name zfs-disk2 --size 100G
zerofs nbd create -c zerofs.toml --name zfs-disk3 --size 100G

# 2. Start ZeroFS
zerofs run -c zerofs.toml &

# 3. Connect NBD devices
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N zfs-disk1 -persist
sudo nbd-client 127.0.0.1 10809 /dev/nbd1 -N zfs-disk2 -persist
sudo nbd-client 127.0.0.1 10809 /dev/nbd2 -N zfs-disk3 -persist

# 4. Create ZFS pool
sudo zpool create tank raidz /dev/nbd0 /dev/nbd1 /dev/nbd2
sudo zfs set compression=lz4 tank

# 5. Create datasets
sudo zfs create tank/homes
sudo zfs create tank/projects

# 6. Export via NFS
sudo zfs set sharenfs="rw,sync,no_subtree_check" tank/homes
sudo zfs set sharenfs="rw,sync,no_subtree_check" tank/projects

# 7. Clients mount
sudo mount -t nfs server-ip:/tank/homes /mnt/homes
```

## Client Access

### Mounting on Clients

```bash
# One-time mount
sudo mount -t nfs server-ip:/mnt/storage /mnt/remote

# Persistent mount (add to /etc/fstab)
echo "server-ip:/mnt/storage /mnt/remote nfs defaults,_netdev 0 0" | sudo tee -a /etc/fstab
```

### Verify Export

```bash
# On server
showmount -e localhost

# On client
showmount -e server-ip
```

## Persistence

### Make Mounts Persistent

Add to `/etc/fstab` on the **server**:

```bash
/dev/nbd0  /mnt/storage  ext4  defaults  0  0
```

### Systemd Service

Create `/etc/systemd/system/zerofs-nbd-export.service`:

```ini
[Unit]
Description=ZeroFS NBD Export
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes

# Start ZeroFS (if not running as separate service)
ExecStartPre=/usr/local/bin/zerofs run -c /etc/zerofs/zerofs.toml &

# Wait for ZeroFS to start
ExecStartPre=/bin/sleep 5

# Connect NBD and mount
ExecStart=/usr/sbin/nbd-client 127.0.0.1 10809 /dev/nbd0 -N storage -persist
ExecStart=/bin/mount /dev/nbd0 /mnt/storage

# Cleanup on stop
ExecStop=/bin/umount /mnt/storage
ExecStop=/usr/sbin/nbd-client -d /dev/nbd0

[Install]
WantedBy=multi-user.target
```

Enable it:
```bash
sudo systemctl enable zerofs-nbd-export.service
sudo systemctl start zerofs-nbd-export.service
```

## Troubleshooting

### Error: "nbd-client: command not found"

```bash
# Install NBD client
sudo apt-get install nbd-client  # Debian/Ubuntu
sudo yum install nbd              # RHEL/CentOS
```

### Error: "mkfs.ext4: command not found"

```bash
# Install filesystem tools
sudo apt-get install e2fsprogs    # For ext4
sudo apt-get install xfsprogs     # For xfs
```

### Error: "Failed to open /etc/exports"

The command needs root privileges:
```bash
sudo zerofs nbd export ...
```

### Error: "Device or resource busy"

NBD device already in use:
```bash
# Disconnect first
sudo nbd-client -d /dev/nbd0

# Then retry export
```

### Check What's Mounted

```bash
# Check NBD connections
cat /proc/partitions | grep nbd

# Check mounts
mount | grep nbd

# Check NFS exports
sudo exportfs -v
```

## Performance Tips

### 1. Use Multiple NBD Connections

```bash
# In nbd-client command, add:
-connections 4
```

### 2. Tune Filesystem

```bash
# For ext4 - disable journal for better performance (less safe)
sudo tune2fs -O ^has_journal /dev/nbd0

# For xfs - use larger allocation groups
sudo mkfs.xfs -f -d agcount=16 /dev/nbd0
```

### 3. NFS Performance Options

```bash
--nfs-options "rw,async,no_subtree_check,no_wdelay"
```

### 4. ZeroFS Cache

In `zerofs.toml`:
```toml
[cache]
disk_size_gb = 50.0      # Larger cache
memory_size_gb = 8.0
```

## Security

### Restrict NFS Access

```bash
# Only allow specific subnet
--nfs-options "rw,sync,no_subtree_check,10.0.0.0/24"

# Only allow specific host
--nfs-options "rw,sync,no_subtree_check,client-ip"
```

### Use Kerberos

```bash
--nfs-options "rw,sync,sec=krb5"
```

## Comparison: Direct NFS vs NBD+NFS

### Direct ZeroFS NFS

```bash
# Simple
mount -t nfs server-ip:/ /mnt/zerofs
```

**Pros:**
- ✅ Simplest setup
- ✅ No formatting needed
- ✅ Multiple clients simultaneously
- ✅ Direct S3 access

**Cons:**
- ❌ No filesystem-level snapshots
- ❌ Limited to ZeroFS features

### NBD + Formatted FS + NFS Export

```bash
# Complex but powerful
zerofs nbd export ... (automated!)
```

**Pros:**
- ✅ Full filesystem features (ext4/xfs/ZFS)
- ✅ Filesystem-level snapshots (ZFS)
- ✅ Compression, dedup (ZFS)
- ✅ Quotas, reservations

**Cons:**
- ❌ More complex setup
- ❌ Extra layer (NBD)
- ❌ Need to format first

## Use Cases

### When to Use `nbd export`

✅ Need ZFS/btrfs snapshots  
✅ Need specific filesystem features  
✅ Running databases that need specific FS  
✅ Need filesystem-level quotas  
✅ Want compression/dedup at FS level  

### When to Use Direct NFS

✅ Simple file sharing  
✅ Don't need snapshots  
✅ Want simplest setup  
✅ Multiple concurrent clients  

## Complete Example: Production Setup

```bash
# 1. Create config
cat > zerofs.toml <<EOF
[storage]
url = "s3://my-bucket/zerofs-data"
encryption_password = "your-secure-password"

[cache]
dir = "/var/cache/zerofs"
disk_size_gb = 50.0

[servers.nbd]
addresses = ["0.0.0.0:10809"]
EOF

# 2. Start ZeroFS
zerofs run -c zerofs.toml &

# 3. Export storage
sudo zerofs nbd export -c zerofs.toml \
  --name production-storage \
  --size 500G \
  --mount-point /mnt/production \
  --filesystem xfs \
  --nfs-options "rw,sync,no_subtree_check,10.0.0.0/24"

# 4. Clients mount
# On each client:
sudo mount -t nfs server-ip:/mnt/production /mnt/storage
```

## Summary

The `zerofs nbd export` command provides a **one-command solution** to:
1. Create S3-backed block devices
2. Format with your choice of filesystem
3. Export via NFS to clients

This gives you the best of both worlds:
- **S3 durability and cost-effectiveness**
- **Filesystem features** (snapshots, compression, quotas)
- **Simple NFS access** for clients

Perfect for providing enterprise-grade storage backed by object storage! 🚀

