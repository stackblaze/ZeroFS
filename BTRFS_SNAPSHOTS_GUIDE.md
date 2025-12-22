# Btrfs Snapshots on S3 with ZeroFS

## Overview

ZeroFS now supports **btrfs as the default filesystem** for NBD exports, giving you instant snapshots backed by S3 storage. This combines the best of both worlds:

- **Btrfs snapshots** - Instant, space-efficient copy-on-write snapshots
- **S3 backend** - Durable, scalable, cost-effective storage
- **NFS sharing** - Simple client access without special software

## Quick Start

```bash
# 1. Start ZeroFS
zerofs run -c zerofs.toml &

# 2. Export as btrfs (one command!)
sudo zerofs nbd export -c zerofs.toml \
  --name storage \
  --size 100G \
  --mount-point /mnt/storage

# 3. Create a snapshot
sudo btrfs dataset snapshot /mnt/storage /mnt/storage/@snapshots/backup-$(date +%Y%m%d)

# 4. Clients mount via NFS
sudo mount -t nfs server-ip:/mnt/storage /mnt/remote
```

## Why Btrfs?

### ✅ Built-in Snapshots
- Instant snapshots (copy-on-write)
- Space-efficient (only stores changes)
- No performance impact during snapshot
- Can snapshot individual datasets

### ✅ Compression
- Automatic zstd compression
- Saves S3 storage costs (30-70% typically)
- Transparent to applications
- Better performance for compressible data

### ✅ Data Integrity
- Checksums on all data and metadata
- Detects silent corruption
- Self-healing with RAID profiles

### ✅ Flexibility
- Datasets for organization
- Quotas per dataset
- Online defragmentation
- Easy to manage

## Architecture

```
┌──────────┐     ┌──────────┐
│ Client 1 │     │ Client 2 │
│   NFS    │     │   NFS    │
└──────────┘     └──────────┘
     │               │
     └───────┬───────┘
             │ NFS Protocol
      ┌──────▼──────┐
      │   Server    │
      │             │
      │ NFS Export  │
      │     ↓       │
      │   Btrfs     │ ← Snapshots, compression
      │ Datasets  │
      │     ↓       │
      │ /dev/nbd0   │ ← NBD block device
      └─────────────┘
             │ NBD Protocol
      ┌──────▼──────┐
      │   ZeroFS    │
      │      ↓      │
      │     S3      │ ← Ultimate storage
      └─────────────┘
```

## Complete Setup

### Step 1: Export with Btrfs

```bash
# Basic export (btrfs is default)
sudo zerofs nbd export -c zerofs.toml \
  --name mydata \
  --size 100G \
  --mount-point /mnt/mydata

# Or explicitly specify btrfs
sudo zerofs nbd export -c zerofs.toml \
  --name mydata \
  --size 100G \
  --mount-point /mnt/mydata \
  --filesystem btrfs
```

**What gets created:**
- `/mnt/mydata/@` - Main dataset
- `/mnt/mydata/@home` - Home directories dataset
- `/mnt/mydata/@snapshots` - Snapshots directory
- Compression: zstd (automatic)
- NFS export: `/mnt/mydata`

### Step 2: Use the Filesystem

```bash
# Write data to main dataset
echo "test data" > /mnt/mydata/@/file.txt

# Or use @home for user data
mkdir -p /mnt/mydata/@home/user1
echo "user data" > /mnt/mydata/@home/user1/document.txt
```

### Step 3: Create Snapshots

```bash
# Snapshot the main dataset
sudo btrfs dataset snapshot /mnt/mydata/@ \
  /mnt/mydata/@snapshots/root-$(date +%Y%m%d-%H%M)

# Snapshot home directories
sudo btrfs dataset snapshot /mnt/mydata/@home \
  /mnt/mydata/@snapshots/home-$(date +%Y%m%d-%H%M)

# Read-only snapshot (recommended for backups)
sudo btrfs dataset snapshot -r /mnt/mydata/@ \
  /mnt/mydata/@snapshots/root-$(date +%Y%m%d-%H%M)-ro
```

## Snapshot Management

### List Snapshots

```bash
# List all datasets (including snapshots)
sudo btrfs dataset list /mnt/mydata

# Show snapshot details
sudo btrfs dataset show /mnt/mydata/@snapshots/root-20251202-1430
```

### Browse Snapshot Data

```bash
# Snapshots are just directories - browse them directly!
ls -la /mnt/mydata/@snapshots/root-20251202-1430/

# Copy file from snapshot
cp /mnt/mydata/@snapshots/root-20251202-1430/file.txt \
   /mnt/mydata/@/file.txt.restored
```

### Restore from Snapshot

```bash
# Method 1: Copy individual files
cp /mnt/mydata/@snapshots/root-20251202-1430/important.txt \
   /mnt/mydata/@/

# Method 2: Rollback entire dataset
# (Requires unmounting and remounting)
sudo umount /mnt/mydata
sudo mount /dev/nbd0 -o subvol=@snapshots/root-20251202-1430 /mnt/mydata
```

### Delete Snapshots

```bash
# Delete old snapshot
sudo btrfs dataset delete /mnt/mydata/@snapshots/root-20251201-1000

# Delete multiple snapshots
sudo btrfs dataset delete /mnt/mydata/@snapshots/root-*
```

## Automated Snapshots

### Using Cron

```bash
# Create /usr/local/bin/btrfs-snapshot.sh
cat <<'EOF' | sudo tee /usr/local/bin/btrfs-snapshot.sh
#!/bin/bash
MOUNT="/mnt/mydata"
TIMESTAMP=$(date +%Y%m%d-%H%M)

# Create snapshots
btrfs dataset snapshot -r "$MOUNT/@" "$MOUNT/@snapshots/root-$TIMESTAMP"
btrfs dataset snapshot -r "$MOUNT/@home" "$MOUNT/@snapshots/home-$TIMESTAMP"

# Delete snapshots older than 7 days
find "$MOUNT/@snapshots" -maxdepth 1 -type d -mtime +7 -exec btrfs dataset delete {} \;

echo "Snapshots created: root-$TIMESTAMP, home-$TIMESTAMP"
EOF

sudo chmod +x /usr/local/bin/btrfs-snapshot.sh

# Add to crontab (hourly snapshots)
echo "0 * * * * /usr/local/bin/btrfs-snapshot.sh" | sudo crontab -
```

### Using Snapper

```bash
# Install snapper
sudo apt-get install snapper

# Create snapper config
sudo snapper -c mydata create-config /mnt/mydata/@

# Configure automatic snapshots
sudo snapper -c mydata set-config \
  "TIMELINE_CREATE=yes" \
  "TIMELINE_CLEANUP=yes" \
  "TIMELINE_LIMIT_HOURLY=24" \
  "TIMELINE_LIMIT_DAILY=7" \
  "TIMELINE_LIMIT_WEEKLY=4" \
  "TIMELINE_LIMIT_MONTHLY=12"

# Manual snapshot
sudo snapper -c mydata create --description "Before upgrade"

# List snapshots
sudo snapper -c mydata list

# Restore from snapshot
sudo snapper -c mydata undochange 1..2
```

## Client Access

### Mount on Clients

```bash
# Clients see the main dataset via NFS
sudo mount -t nfs server-ip:/mnt/mydata /mnt/remote

# Access data
ls /mnt/remote/@/
ls /mnt/remote/@home/

# Clients can also browse snapshots (read-only)
ls /mnt/remote/@snapshots/
```

### Snapshot Access for Clients

Clients can access snapshots directly via NFS:

```bash
# On client
ls /mnt/remote/@snapshots/root-20251202-1430/

# Restore file on client
cp /mnt/remote/@snapshots/root-20251202-1430/file.txt \
   /mnt/remote/@/file.txt.restored
```

## Compression

### Check Compression Ratio

```bash
# Show compression stats
sudo compsize /mnt/mydata

# Example output:
# Processed 1000 files, 5.0 GiB uncompressed, 2.5 GiB compressed (50% ratio)
```

### Change Compression Algorithm

```bash
# Remount with different compression
sudo mount -o remount,compress=lzo /mnt/mydata

# Or in /etc/fstab:
/dev/nbd0  /mnt/mydata  btrfs  compress=zstd:3  0  0

# Compression levels:
# - zstd:1 (fast, less compression)
# - zstd:3 (balanced - default)
# - zstd:15 (slow, best compression)
```

### Compress Existing Data

```bash
# Defragment and compress existing files
sudo btrfs filesystem defragment -r -czstd /mnt/mydata/@
```

## Quotas

### Enable Quotas

```bash
# Enable quota support
sudo btrfs quota enable /mnt/mydata

# Set quota for @home dataset (50GB limit)
sudo btrfs qgroup limit 50G /mnt/mydata/@home

# Check quota usage
sudo btrfs qgroup show /mnt/mydata
```

## Performance Tips

### 1. Use SSD Mode

```bash
# If using fast storage, enable SSD optimizations
sudo mount -o remount,ssd /mnt/mydata

# In /etc/fstab:
/dev/nbd0  /mnt/mydata  btrfs  compress=zstd,ssd  0  0
```

### 2. Disable Copy-on-Write for Databases

```bash
# For database files, disable COW
mkdir /mnt/mydata/@/databases
sudo chattr +C /mnt/mydata/@/databases

# Now database files in this directory won't use COW
```

### 3. Tune for Network Storage

```bash
# Increase commit interval (less S3 writes)
sudo mount -o remount,commit=120 /mnt/mydata

# In /etc/fstab:
/dev/nbd0  /mnt/mydata  btrfs  compress=zstd,commit=120  0  0
```

## Backup and Replication

### Send/Receive for Backups

```bash
# Create read-only snapshot
sudo btrfs dataset snapshot -r /mnt/mydata/@ \
  /mnt/mydata/@snapshots/backup-$(date +%Y%m%d)

# Send to another server
sudo btrfs send /mnt/mydata/@snapshots/backup-20251202 | \
  ssh backup-server "btrfs receive /backup/mydata/"

# Incremental send (much faster!)
sudo btrfs send -p /mnt/mydata/@snapshots/backup-20251201 \
  /mnt/mydata/@snapshots/backup-20251202 | \
  ssh backup-server "btrfs receive /backup/mydata/"
```

### Backup to S3 (Different Bucket)

```bash
# Send snapshot to file
sudo btrfs send /mnt/mydata/@snapshots/backup-20251202 | \
  gzip > /tmp/backup-20251202.btrfs.gz

# Upload to S3
aws s3 cp /tmp/backup-20251202.btrfs.gz \
  s3://backup-bucket/btrfs-backups/

# Restore from S3
aws s3 cp s3://backup-bucket/btrfs-backups/backup-20251202.btrfs.gz - | \
  gunzip | sudo btrfs receive /mnt/mydata/@snapshots/
```

## Monitoring

### Check Filesystem Health

```bash
# Show filesystem info
sudo btrfs filesystem show /mnt/mydata

# Check usage
sudo btrfs filesystem usage /mnt/mydata

# Device stats
sudo btrfs device stats /mnt/mydata
```

### Scrub for Errors

```bash
# Start scrub (checks all data)
sudo btrfs scrub start /mnt/mydata

# Check scrub status
sudo btrfs scrub status /mnt/mydata

# Schedule monthly scrub
echo "0 2 1 * * btrfs scrub start /mnt/mydata" | sudo crontab -
```

## Troubleshooting

### Error: "mkfs.btrfs: command not found"

```bash
# Install btrfs tools
sudo apt-get install btrfs-progs  # Debian/Ubuntu
sudo yum install btrfs-progs      # RHEL/CentOS
```

### Filesystem Full

```bash
# Check actual usage
sudo btrfs filesystem usage /mnt/mydata

# Balance filesystem (reclaim space)
sudo btrfs balance start -dusage=50 /mnt/mydata

# Delete old snapshots
sudo btrfs dataset delete /mnt/mydata/@snapshots/old-*
```

### Slow Performance

```bash
# Defragment
sudo btrfs filesystem defragment -r /mnt/mydata

# Check for fragmentation
sudo filefrag -v /mnt/mydata/@/large-file
```

## Complete Example: Production Setup

```bash
# 1. Create ZeroFS config
cat > zerofs.toml <<EOF
[storage]
url = "s3://production-bucket/zerofs"
encryption_password = "secure-password"

[cache]
dir = "/var/cache/zerofs"
disk_size_gb = 100.0

[servers.nbd]
addresses = ["0.0.0.0:10809"]
EOF

# 2. Start ZeroFS
zerofs run -c zerofs.toml &

# 3. Export as btrfs with NFS
sudo zerofs nbd export -c zerofs.toml \
  --name production \
  --size 500G \
  --mount-point /mnt/production \
  --nfs-options "rw,sync,no_subtree_check,10.0.0.0/24"

# 4. Set up automated snapshots
cat <<'EOF' | sudo tee /usr/local/bin/snapshot-production.sh
#!/bin/bash
MOUNT="/mnt/production"
TIMESTAMP=$(date +%Y%m%d-%H%M)
btrfs dataset snapshot -r "$MOUNT/@" "$MOUNT/@snapshots/prod-$TIMESTAMP"
find "$MOUNT/@snapshots" -maxdepth 1 -type d -mtime +30 -exec btrfs dataset delete {} \;
EOF

sudo chmod +x /usr/local/bin/snapshot-production.sh
echo "0 */6 * * * /usr/local/bin/snapshot-production.sh" | sudo crontab -

# 5. Clients mount
# On each client:
sudo mount -t nfs production-server:/mnt/production /mnt/data
```

## Comparison: Btrfs vs ZFS

| Feature | Btrfs | ZFS |
|---------|-------|-----|
| **Snapshots** | ✅ Instant, COW | ✅ Instant, COW |
| **Compression** | ✅ zstd, lzo, zlib | ✅ lz4, zstd, gzip |
| **Checksums** | ✅ CRC32C | ✅ SHA256, Fletcher |
| **Setup** | ✅ Single command | ❌ Manual pool creation |
| **RAID** | ✅ Built-in | ✅ Built-in |
| **Dedup** | ⚠️ Experimental | ✅ Stable |
| **Performance** | ✅ Good | ✅ Excellent |
| **Maturity** | ✅ Stable (Linux 5.x+) | ✅ Very mature |
| **License** | ✅ GPL | ⚠️ CDDL (not in kernel) |

**Recommendation:** Use **btrfs** for simplicity and one-command setup. Use **ZFS** if you need deduplication or maximum performance.

## Summary

With btrfs as the default, you get:

✅ **Instant snapshots** - Copy-on-write, space-efficient  
✅ **Automatic compression** - Saves S3 costs (zstd)  
✅ **S3 backend** - Durable, scalable storage  
✅ **NFS sharing** - Simple client access  
✅ **One command setup** - `zerofs nbd export`  
✅ **Datasets** - Organize data logically  
✅ **Data integrity** - Checksums on everything  

Perfect for providing snapshot-capable storage backed by S3! 🚀

