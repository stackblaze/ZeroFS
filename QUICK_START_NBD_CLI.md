# Quick Start: NBD CLI Commands

## Problem You Had

You correctly identified that using NBD required mounting NFS first - a chicken-and-egg problem!

```bash
# Old way (annoying!)
zerofs run -c zerofs.toml &
sudo mount -t nfs 127.0.0.1:/ /mnt/zerofs  # ← Need NFS just to create NBD!
sudo truncate -s 10G /mnt/zerofs/.nbd/device
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N device
```

## Solution: New CLI Commands

I've added `zerofs nbd` commands that let you manage NBD devices directly!

```bash
# New way (much better!)
zerofs nbd create -c zerofs.toml --name device --size 10G  # ← No NFS needed!
zerofs run -c zerofs.toml &
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N device
```

## Quick Reference

### Create Device
```bash
zerofs nbd create -c zerofs.toml --name my-device --size 10G
```

### List Devices
```bash
zerofs nbd list -c zerofs.toml
```

### Delete Device
```bash
zerofs nbd delete -c zerofs.toml --name my-device --force
```

### Resize Device
```bash
zerofs nbd resize -c zerofs.toml --name my-device --size 20G
```

## Complete Example

```bash
# 1. Create devices (server can be stopped)
zerofs nbd create -c zerofs.toml --name database --size 10G
zerofs nbd create -c zerofs.toml --name storage --size 50G

# 2. List them
zerofs nbd list -c zerofs.toml

# 3. Start server
zerofs run -c zerofs.toml &

# 4. Connect NBD devices
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N database
sudo nbd-client 127.0.0.1 10809 /dev/nbd1 -N storage

# 5. Use them
sudo mkfs.ext4 /dev/nbd0
sudo mount /dev/nbd0 /mnt/database
```

## Size Formats

All these work:
- `10G` or `10GB` - 10 gigabytes
- `512M` or `512MB` - 512 megabytes  
- `1T` or `1TB` - 1 terabyte
- `10.5G` - 10.5 gigabytes
- `1073741824` - plain bytes

## Files Changed

New files:
- `zerofs/src/cli/nbd.rs` - NBD management commands

Modified files:
- `zerofs/src/cli/mod.rs` - Added NbdCommands enum
- `zerofs/src/main.rs` - Added command handlers

## About iSCSI

**Same problem exists for iSCSI!** If you implement iSCSI, you'd want the same CLI approach:

```bash
# Hypothetical (not implemented)
zerofs iscsi create-target -c zerofs.toml \
  --iqn iqn.2024.net.zerofs:storage \
  --lun 0 --size 10G
```

**Quick iSCSI workaround** (no code needed):
```bash
# Use NBD as backend for iSCSI
zerofs nbd create -c zerofs.toml --name storage --size 100G
zerofs run -c zerofs.toml &
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N storage

# Export NBD via iSCSI using tgt
sudo tgtadm --lld iscsi --op new --mode target \
  --tid 1 --targetname iqn.2024.net.zerofs:storage
sudo tgtadm --lld iscsi --op new --mode logicalunit \
  --tid 1 --lun 1 --backing-store /dev/nbd0
```

Now you have iSCSI without implementing it! 🎉

## Testing

To test the new commands (requires Rust/Cargo):

```bash
cd /home/linux/projects/ZeroFS
cargo build --release
./target/release/zerofs nbd --help
```

## Documentation

See detailed docs:
- `NBD_CLI_USAGE.md` - Complete usage guide
- `ISCSI_ANALYSIS.md` - iSCSI implementation analysis

