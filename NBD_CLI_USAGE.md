# NBD Device Management CLI

This document describes the new CLI commands for managing NBD devices without requiring NFS/9P to be mounted first.

## Problem Solved

Previously, to use NBD devices with ZeroFS, you had to:
1. Start ZeroFS server
2. Mount via NFS or 9P
3. Create files in `.nbd/` directory using filesystem tools
4. Connect NBD client

This created a chicken-and-egg problem where you needed one protocol (NFS/9P) just to set up another (NBD).

## Solution

The new `zerofs nbd` commands allow you to manage NBD devices directly through the CLI, without needing to mount the filesystem first.

## Commands

### Create a New NBD Device

```bash
zerofs nbd create -c zerofs.toml --name my-device --size 10G
```

Creates a new NBD device that will be available at `.nbd/my-device`.

**Size formats supported:**
- Plain bytes: `1073741824`
- Kilobytes: `10K` or `10KB`
- Megabytes: `512M` or `512MB`
- Gigabytes: `10G` or `10GB`
- Terabytes: `1T` or `1TB`
- Decimals: `10.5G`

**Example output:**
```
✓ Created NBD device 'my-device' (10,737,418,240 bytes)
  Inode: 42
  Size: 10.00 GB

Connect with:
  nbd-client <host> <port> /dev/nbd0 -N my-device
```

### List All NBD Devices

```bash
zerofs nbd list -c zerofs.toml
```

Shows all NBD devices with their sizes and inode numbers.

**Example output:**
```
┌──────────┬───────┬──────────┬─────────────────┐
│ NAME     │ INODE │ SIZE     │ SIZE (bytes)    │
├──────────┼───────┼──────────┼─────────────────┤
│ database │ 42    │ 10.00 GB │ 10,737,418,240  │
│ storage  │ 43    │ 50.00 GB │ 53,687,091,200  │
│ backup   │ 44    │ 100.00 GB│ 107,374,182,400 │
└──────────┴───────┴──────────┴─────────────────┘
```

### Delete an NBD Device

```bash
# With confirmation prompt
zerofs nbd delete -c zerofs.toml --name my-device

# Skip confirmation
zerofs nbd delete -c zerofs.toml --name my-device --force
```

Permanently deletes an NBD device. **This cannot be undone!**

**Example output:**
```
✓ Deleted NBD device 'my-device'
  Inode: 42
```

### Resize an NBD Device

```bash
zerofs nbd resize -c zerofs.toml --name my-device --size 20G
```

Changes the size of an existing NBD device. Can grow or shrink.

**Example output:**
```
✓ Resized NBD device 'my-device'
  Old size: 10.00 GB (10,737,418,240)
  New size: 20.00 GB (21,474,836,480)
```

**Warning when shrinking:**
```
⚠ Warning: Device was shrunk. Make sure no filesystem is using the truncated space!
```

## Complete Workflow Example

### Before (Old Way - Requires NFS)

```bash
# Start ZeroFS
zerofs run -c zerofs.toml &

# Mount via NFS
sudo mount -t nfs -o vers=3,nolock 127.0.0.1:/ /mnt/zerofs

# Create device
sudo mkdir -p /mnt/zerofs/.nbd
sudo truncate -s 10G /mnt/zerofs/.nbd/my-device

# Connect NBD
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N my-device

# Use it
sudo mkfs.ext4 /dev/nbd0
sudo mount /dev/nbd0 /mnt/nbd
```

### After (New Way - No NFS Required)

```bash
# Create device BEFORE starting server
zerofs nbd create -c zerofs.toml --name my-device --size 10G

# Start ZeroFS
zerofs run -c zerofs.toml &

# Connect NBD directly
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N my-device

# Use it
sudo mkfs.ext4 /dev/nbd0
sudo mount /dev/nbd0 /mnt/nbd
```

**Key improvement:** No need to mount NFS/9P first!

## Advanced Usage

### Creating Multiple Devices at Once

```bash
zerofs nbd create -c zerofs.toml --name database --size 10G
zerofs nbd create -c zerofs.toml --name storage --size 50G
zerofs nbd create -c zerofs.toml --name backup --size 100G

# List them
zerofs nbd list -c zerofs.toml
```

### Managing Devices While Server is Running

All commands work whether the ZeroFS server is running or not:

```bash
# Server is running
zerofs run -c zerofs.toml &

# Create a new device on-the-fly
zerofs nbd create -c zerofs.toml --name new-device --size 5G

# It's immediately available
sudo nbd-client 127.0.0.1 10809 /dev/nbd1 -N new-device
```

### Resizing a Live Device

```bash
# Resize (server can be running or stopped)
zerofs nbd resize -c zerofs.toml --name my-device --size 20G

# For NBD clients to see the new size, you may need to:
# 1. Disconnect and reconnect the NBD device, OR
# 2. Use blockdev --rereadpt /dev/nbd0 (if supported)
```

## Implementation Details

### How It Works

The CLI commands directly access the ZeroFS database (SlateDB) to:
1. Create/modify files in the `.nbd/` directory
2. Set file sizes using the same `SetAttributes` API used by NFS/9P
3. Flush changes to ensure persistence

This bypasses the need for protocol servers (NFS/9P) entirely.

### Advantages Over NFS/9P Method

1. **No chicken-and-egg problem** - Create NBD devices without mounting
2. **Simpler workflow** - One command instead of mount + mkdir + truncate
3. **Better error handling** - Clear error messages and validation
4. **Scriptable** - Easy to automate device provisioning
5. **Works offline** - Can create devices before starting the server
6. **Consistent interface** - Same CLI tool for all operations

### Compatibility

- Fully compatible with existing NBD devices created via NFS/9P
- Devices created with CLI are visible via NFS/9P mounts
- Devices created via NFS/9P are visible with `zerofs nbd list`
- No migration needed

## iSCSI Comparison

This same approach could be used for iSCSI:

```bash
# Hypothetical iSCSI commands (not yet implemented)
zerofs iscsi create -c zerofs.toml --target iqn.2024.net.zerofs:storage --lun 0 --size 10G
zerofs iscsi list -c zerofs.toml
```

The implementation would be similar:
- Store iSCSI LUN metadata in a special directory (e.g., `.iscsi/`)
- CLI commands to manage targets and LUNs
- iSCSI server reads from this directory at runtime

**Key difference from NBD:** iSCSI requires more metadata:
- IQN (iSCSI Qualified Name)
- LUN numbers
- Authentication credentials (CHAP)
- Target portal groups

This could be stored as JSON/TOML files in `.iscsi/` directory or in a dedicated metadata store.

## Troubleshooting

### "Failed to find .nbd directory"

This is normal for `delete` and `resize` commands if no devices exist yet. Use `create` first.

### "NBD device 'X' already exists"

Device names must be unique. Use `list` to see existing devices, or `delete` the old one first.

### "Invalid size format"

Make sure to use valid size formats: `10G`, `512M`, `1T`, or plain bytes.

### Changes Not Visible to NBD Client

After creating/resizing devices, NBD clients need to:
1. Disconnect and reconnect to see new devices
2. Some resize operations may require remounting filesystems

## Future Enhancements

Possible additions:
- `zerofs nbd info <name>` - Show detailed device information
- `zerofs nbd clone <source> <dest>` - Clone a device
- `zerofs nbd snapshot <name>` - Create device snapshots
- Batch operations: `zerofs nbd create --batch devices.json`

