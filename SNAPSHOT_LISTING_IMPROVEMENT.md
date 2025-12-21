# Snapshot Listing Improvement

## Issue

The `list-snapshots` command showed a confusing "Parent ID" column with values like "0", which wasn't immediately clear to users.

```
│ ID ┆ Name                    ┆ Parent ID ┆ UUID              ┆ Created At       │
│ 3  ┆ root-backup-20251220    ┆ 0         ┆ 52fc6463-015a... ┆ 2025-12-20 15:21 │
```

User question: "is that file size?"

## Explanation

**No, it's not file size!** The column shows the **source subvolume ID** that the snapshot was created from.

- **Parent ID = 0** means the snapshot was created from the **root subvolume** (which has ID 0)
- If you create a snapshot from a different subvolume, you'll see a different ID

## Solution

Changed the column to display the **source subvolume name** instead of just the ID, and renamed it to "Source" for clarity.

### Before
```
│ Parent ID ┆
│ 0         ┆  ← What does this mean?
```

### After
```
│ Source    ┆
│ root      ┆  ← Clear! Snapshot is from "root" subvolume
```

## Implementation

Modified `zerofs/src/cli/subvolume.rs` in the `list_snapshots` function:

1. Fetch all subvolumes to build an ID→name mapping
2. Look up the source subvolume name for each snapshot
3. Display the name instead of the ID

```rust
// Get all subvolumes to map IDs to names
let subvolumes = client.list_subvolumes().await?;
let mut id_to_name: HashMap<u64, String> = subvolumes
    .into_iter()
    .map(|s| (s.id, s.name))
    .collect();

// Display source name instead of ID
let source = snapshot.parent_id
    .and_then(|id| id_to_name.get(&id).cloned())
    .unwrap_or_else(|| "-".to_string());
```

## New Output

```
┌────┬───────────────────────────────┬───────────┬──────────────────────────────────────┬─────────────────────────┐
│ ID ┆ Name                          ┆ Source    ┆ UUID                                 ┆ Created At              │
╞════╪═══════════════════════════════╪═══════════╪══════════════════════════════════════╪═════════════════════════╡
│ 3  ┆ root-backup-20251220          ┆ root      ┆ 52fc6463-015a-43b8-a0fa-fe8c3ce7079d ┆ 2025-12-20 15:21:14 UTC │
│ 29 ┆ snapshot-of-my-subvol         ┆ my-subvol ┆ e3125b6b-d7fe-458d-8d5f-8088eb0031b5 ┆ 2025-12-21 18:07:59 UTC │
└────┴───────────────────────────────┴───────────┴──────────────────────────────────────┴─────────────────────────┘
```

Now it's immediately clear:
- Most snapshots are from the `root` subvolume
- One snapshot (`snapshot-of-my-subvol`) is from a different subvolume (`my-subvol`)

## Example Usage

```bash
# Create a subvolume
zerofs subvolume create -c zerofs.toml my-data

# Create a snapshot of it
zerofs subvolume snapshot -c zerofs.toml my-data backup-of-my-data

# List snapshots - now shows "my-data" in Source column
zerofs subvolume list-snapshots -c zerofs.toml
```

## Files Changed

- `zerofs/src/cli/subvolume.rs`:
  - Lines 160-192: `list_snapshots` function
  - Changed column header from "Parent ID" to "Source"
  - Added ID→name mapping lookup
  - Display source subvolume name instead of raw ID

## Status: ✅ Complete

The snapshot listing is now more user-friendly and self-explanatory.

