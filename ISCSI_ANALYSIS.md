# iSCSI Implementation Analysis for ZeroFS

## Executive Summary

**Question:** How difficult is it to add iSCSI support to ZeroFS (which currently offers NFS)?

**Answer:** Moderately difficult (6-8 weeks) if implementing from scratch, but the **same chicken-and-egg problem exists** as with NBD.

## Current State

ZeroFS currently supports:
- **NFS** (Network File System) - File-level access
- **9P** (Plan 9 Protocol) - File-level access  
- **NBD** (Network Block Device) - Block-level access

All protocols share the same ZeroFS backend (SlateDB on S3).

## The Chicken-and-Egg Problem

### Problem Description

Both NBD and iSCSI have a device discovery/provisioning issue:

**NBD (Before Fix):**
```bash
# Step 1: Start ZeroFS
zerofs run -c zerofs.toml &

# Step 2: Mount NFS (chicken-and-egg!)
sudo mount -t nfs 127.0.0.1:/ /mnt/zerofs

# Step 3: Create NBD device
sudo truncate -s 10G /mnt/zerofs/.nbd/my-device

# Step 4: Connect NBD
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N my-device
```

**Problem:** You need NFS mounted to create NBD devices!

**iSCSI (Would Have Same Issue):**
```bash
# Step 1: Start ZeroFS with iSCSI
zerofs run -c zerofs.toml &

# Step 2: Mount NFS (chicken-and-egg!)
sudo mount -t nfs 127.0.0.1:/ /mnt/zerofs

# Step 3: Create iSCSI target/LUN metadata
sudo mkdir -p /mnt/zerofs/.iscsi/
sudo cat > /mnt/zerofs/.iscsi/target1.toml <<EOF
[target]
iqn = "iqn.2024.net.zerofs:storage"
lun = 0
size = "10G"
EOF

# Step 4: Discover and connect
sudo iscsiadm --mode discovery --type sendtargets --portal 127.0.0.1
sudo iscsiadm --mode node --targetname iqn.2024.net.zerofs:storage --login
```

**Problem:** Same issue - need NFS/9P to provision iSCSI targets!

## Solution: CLI-Based Device Management

### NBD Solution (Implemented)

Added `zerofs nbd` commands that directly access the database:

```bash
# Create device WITHOUT mounting NFS
zerofs nbd create -c zerofs.toml --name my-device --size 10G

# Start server
zerofs run -c zerofs.toml &

# Connect directly
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N my-device
```

**Benefits:**
- No NFS dependency
- Works before server starts
- Simple, scriptable interface
- Better error handling

### iSCSI Solution (Proposed)

Same approach would work for iSCSI:

```bash
# Create iSCSI target WITHOUT mounting NFS
zerofs iscsi create-target \
  -c zerofs.toml \
  --iqn iqn.2024.net.zerofs:storage \
  --lun 0 \
  --size 10G

# Start server
zerofs run -c zerofs.toml &

# Discover and connect
sudo iscsiadm --mode discovery --type sendtargets --portal 127.0.0.1
sudo iscsiadm --mode node --targetname iqn.2024.net.zerofs:storage --login
```

## iSCSI Implementation Difficulty

### Complexity Comparison

| Aspect | NBD | iSCSI | Difficulty Increase |
|--------|-----|-------|---------------------|
| **Protocol Spec** | ~50 pages | ~257 pages | 5x more complex |
| **Commands** | 7 basic commands | 100+ SCSI commands | 14x more commands |
| **Authentication** | None | CHAP, Kerberos | Significant |
| **Discovery** | Simple list | iSNS, SendTargets | Moderate |
| **Session Mgmt** | Single connection | Multiple sessions | Complex |
| **Error Handling** | Basic error codes | SCSI sense codes | Complex |
| **Multipath** | Limited | Full multipath I/O | Very complex |

### Implementation Options

#### Option 1: From Scratch (High Difficulty)
- **Time:** 6-8 weeks for experienced developer
- **Code:** ~8,000-12,000 lines
- **Difficulty:** 8/10
- **Pros:** Full control, pure Rust
- **Cons:** High effort, maintenance burden

#### Option 2: Use Existing Rust Library (Moderate Difficulty)
- **Time:** 2-4 weeks
- **Code:** ~2,000-3,000 lines (integration)
- **Difficulty:** 5/10
- **Pros:** Faster development
- **Cons:** Limited mature Rust iSCSI libraries

#### Option 3: C Library Bindings (Moderate Difficulty)
- **Time:** 2-3 weeks
- **Code:** ~1,500-2,500 lines
- **Difficulty:** 4/10
- **Pros:** Mature libraries (tgt, LIO)
- **Cons:** FFI complexity, less "Rusty"

#### Option 4: NBD → iSCSI Bridge (Low Difficulty)
- **Time:** 1-2 hours (configuration only)
- **Code:** 0 lines (use existing tools)
- **Difficulty:** 2/10
- **Pros:** Immediate solution, no code
- **Cons:** Extra layer, not native

### Option 4 Example: NBD → iSCSI Bridge

Use existing Linux tools to bridge NBD to iSCSI:

```bash
# 1. Create NBD device via CLI
zerofs nbd create -c zerofs.toml --name storage --size 100G

# 2. Start ZeroFS
zerofs run -c zerofs.toml &

# 3. Connect NBD locally
sudo nbd-client 127.0.0.1 10809 /dev/nbd0 -N storage

# 4. Export NBD device as iSCSI target using tgt
sudo tgtadm --lld iscsi --op new --mode target \
  --tid 1 --targetname iqn.2024.net.zerofs:storage

sudo tgtadm --lld iscsi --op new --mode logicalunit \
  --tid 1 --lun 1 --backing-store /dev/nbd0

sudo tgtadm --lld iscsi --op bind --mode target \
  --tid 1 --initiator-address ALL

# 5. Now accessible via iSCSI!
sudo iscsiadm --mode discovery --type sendtargets --portal 127.0.0.1
sudo iscsiadm --mode node --targetname iqn.2024.net.zerofs:storage --login
```

**Result:** iSCSI support with zero code changes!

## Recommended Approach

### For Most Users: NBD is Sufficient

NBD provides:
- ✅ Block device access
- ✅ TRIM/discard support
- ✅ Good performance
- ✅ Simple protocol
- ✅ Works on Linux
- ✅ CLI management (new!)

**Use NBD unless you specifically need:**
- Windows compatibility (Windows has better iSCSI support)
- VMware/Hyper-V integration
- Enterprise features (persistent reservations, multipath)
- Existing iSCSI infrastructure

### For Enterprise Features: Implement Native iSCSI

If you need true iSCSI features, implement it properly:

**Phase 1: Basic iSCSI (4 weeks)**
- SCSI command set (READ, WRITE, INQUIRY, etc.)
- iSCSI PDU handling
- Session/connection management
- SendTargets discovery
- CLI device management (like NBD)

**Phase 2: Advanced Features (4 weeks)**
- CHAP authentication
- Multiple initiators
- Error recovery
- Performance optimization

**Phase 3: Enterprise Features (4+ weeks)**
- Persistent reservations
- Multipath I/O
- iSNS integration
- Advanced SCSI commands

### For Quick iSCSI: Use Bridge

If you just need iSCSI compatibility:
1. Use NBD CLI to create devices
2. Bridge NBD → iSCSI with `tgt` or `targetcli`
3. Done in hours, not weeks

## Implementation Roadmap (If Building Native iSCSI)

### Week 1-2: Core Protocol
- [ ] iSCSI PDU parsing/generation
- [ ] Basic SCSI commands (READ, WRITE, INQUIRY)
- [ ] Session negotiation
- [ ] Connection management

### Week 3-4: Device Management
- [ ] CLI commands (`zerofs iscsi create/list/delete`)
- [ ] Target/LUN metadata storage
- [ ] Discovery (SendTargets)
- [ ] Integration with ZeroFS backend

### Week 5-6: Authentication & Polish
- [ ] CHAP authentication
- [ ] Error handling
- [ ] Documentation
- [ ] Testing

### Week 7-8: Advanced Features (Optional)
- [ ] Multiple sessions per target
- [ ] Persistent reservations
- [ ] Multipath support
- [ ] Performance tuning

## Code Structure (Proposed)

Similar to NBD implementation:

```
zerofs/src/
├── iscsi/
│   ├── mod.rs
│   ├── server.rs          # iSCSI target server
│   ├── protocol.rs        # iSCSI PDU definitions
│   ├── scsi.rs           # SCSI command handling
│   ├── session.rs        # Session/connection management
│   ├── auth.rs           # CHAP authentication
│   ├── discovery.rs      # SendTargets, iSNS
│   └── error.rs          # Error types
├── cli/
│   ├── iscsi.rs          # CLI commands (NEW)
│   └── ...
└── ...
```

## Key Differences: NBD vs iSCSI Backend Integration

Both would use the same ZeroFS backend:

```rust
// NBD (current)
async fn handle_read(&mut self, inode: u64, offset: u64, length: u32) {
    self.filesystem.read_file(&auth, inode, offset, length).await
}

// iSCSI (proposed) - SAME backend calls!
async fn handle_scsi_read(&mut self, lun: u64, lba: u64, blocks: u32) {
    let offset = lba * BLOCK_SIZE;
    let length = blocks * BLOCK_SIZE;
    self.filesystem.read_file(&auth, lun, offset, length).await
}
```

**Key insight:** The hard part is the protocol, not the storage integration!

## Conclusion

### Difficulty Assessment

**Adding iSCSI to ZeroFS:**
- **Protocol Implementation:** 7/10 difficulty
- **Backend Integration:** 2/10 difficulty (already done for NBD)
- **CLI Management:** 1/10 difficulty (same as NBD CLI)
- **Overall:** 6/10 difficulty

### Recommendations

1. **For most users:** Stick with NBD + new CLI management
2. **For Windows/VMware users:** Use NBD → iSCSI bridge (immediate)
3. **For enterprise features:** Implement native iSCSI (6-8 weeks)

### The Real Win: CLI Management

The NBD CLI commands solve the **real usability problem**:
- No more NFS dependency
- Simpler workflows
- Better automation
- Same approach works for iSCSI

**Bottom line:** The chicken-and-egg problem is now solved for NBD, and the same solution pattern applies to iSCSI if you implement it.

