use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod checkpoint;
pub mod compactor;
pub mod dataset;
pub mod debug;
pub mod fatrace;
pub mod nbd;
pub mod password;
pub mod server;

#[derive(Parser)]
#[command(name = "zerofs")]
#[command(author, version, about = "The Filesystem That Makes S3 your Primary Storage", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate a default configuration file
    Init {
        #[arg(default_value = "zerofs.toml")]
        path: PathBuf,
    },
    /// Run the filesystem server
    Run {
        #[arg(short, long)]
        config: PathBuf,
        /// Open the filesystem in read-only mode
        #[arg(long, conflicts_with = "checkpoint")]
        read_only: bool,
        /// Open from a specific checkpoint by name (read-only mode)
        #[arg(long, conflicts_with = "read_only")]
        checkpoint: Option<String>,
        /// Run without the built-in compactor (use with external compactor)
        #[arg(long)]
        no_compactor: bool,
    },
    /// Change the encryption password
    ///
    /// Reads new password from stdin. Examples:
    ///
    /// echo "newpassword" | zerofs change-password -c config.toml
    ///
    /// zerofs change-password -c config.toml < password.txt
    ChangePassword {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Debug commands for inspecting the database
    Debug {
        #[command(subcommand)]
        subcommand: DebugCommands,
    },
    /// Checkpoint management commands
    Checkpoint {
        #[command(subcommand)]
        subcommand: CheckpointCommands,
    },
    /// NBD device management commands
    Nbd {
        #[command(subcommand)]
        subcommand: NbdCommands,
    },
    /// Dataset and snapshot management commands
    Dataset {
        #[command(subcommand)]
        subcommand: DatasetCommands,
    },
    /// Trace file system operations in real-time
    Fatrace {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Run standalone compactor for the database
    ///
    /// Use this to run compaction on a separate instance from the writer.
    /// The writer should be started with --no-compactor flag.
    Compactor {
        #[arg(short, long)]
        config: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum DebugCommands {
    /// List all keys in the database
    ListKeys {
        #[arg(short, long)]
        config: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum CheckpointCommands {
    /// Create a new checkpoint
    Create {
        #[arg(short, long)]
        config: PathBuf,
        /// Name for the checkpoint (must be unique)
        name: String,
    },
    /// List all checkpoints
    List {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Delete a checkpoint by name
    Delete {
        #[arg(short, long)]
        config: PathBuf,
        /// Checkpoint name to delete
        name: String,
    },
    /// Get checkpoint information
    Info {
        #[arg(short, long)]
        config: PathBuf,
        /// Checkpoint name to query
        name: String,
    },
}

#[derive(Subcommand)]
pub enum NbdCommands {
    /// Create a new NBD device
    Create {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name (will be accessible as .nbd/<name>)
        name: String,
        /// Device size (e.g., 10G, 512M, 1T)
        size: String,
    },
    /// List all NBD devices
    List {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Delete an NBD device
    Delete {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name to delete
        name: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
    /// Resize an NBD device
    Resize {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name to resize
        name: String,
        /// New size (e.g., 10G, 512M, 1T)
        size: String,
    },
    /// Format an NBD device with a filesystem
    Format {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name to format
        name: String,
        /// Filesystem type (currently supports: btrfs)
        #[arg(default_value = "btrfs")]
        filesystem: String,
        /// Additional mkfs options (passed directly to mkfs command)
        #[arg(long)]
        mkfs_options: Option<String>,
    },
    /// Export an NBD device via NFS (mounts device and configures NFS export)
    Export {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name to export
        name: String,
        /// Mount point for the device (will be exported via NFS)
        #[arg(long)]
        mount_point: PathBuf,
        /// NBD device path (e.g., /dev/nbd0)
        #[arg(long, default_value = "/dev/nbd0")]
        nbd_device: PathBuf,
        /// Filesystem type (auto-detect if already formatted, otherwise format with this)
        #[arg(long)]
        filesystem: Option<String>,
        /// NFS export path (defaults to mount_point)
        #[arg(long)]
        nfs_export_path: Option<String>,
        /// NFS export options (default: rw,sync,no_subtree_check)
        #[arg(long, default_value = "rw,sync,no_subtree_check")]
        nfs_options: String,
    },
    /// Unexport an NBD device (unmount and remove NFS export)
    Unexport {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name to unexport
        name: String,
        /// Mount point to unmount
        #[arg(long)]
        mount_point: PathBuf,
        /// NBD device path
        #[arg(long, default_value = "/dev/nbd0")]
        nbd_device: PathBuf,
    },
    /// Create a BTRFS snapshot of an exported NBD device
    Snapshot {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name (must be exported/mounted)
        name: String,
        /// Mount point where device is mounted
        #[arg(long)]
        mount_point: PathBuf,
        /// Snapshot name
        snapshot_name: String,
        /// Snapshot path (relative to mount point, defaults to .snapshots/<name>)
        #[arg(long)]
        snapshot_path: Option<String>,
        /// Create read-only snapshot
        #[arg(long)]
        read_only: bool,
    },
    /// List BTRFS snapshots for an exported NBD device
    Snapshots {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name
        name: String,
        /// Mount point where device is mounted
        #[arg(long)]
        mount_point: PathBuf,
    },
    /// Restore from a BTRFS snapshot
    Restore {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name
        name: String,
        /// Mount point where device is mounted
        #[arg(long)]
        mount_point: PathBuf,
        /// Snapshot name to restore from
        snapshot_name: String,
        /// Snapshot path (relative to mount point, defaults to .snapshots/<name>)
        #[arg(long)]
        snapshot_path: Option<String>,
        /// Target path to restore to (defaults to root of filesystem)
        #[arg(long)]
        target_path: Option<String>,
    },
    /// Delete a BTRFS snapshot
    DeleteSnapshot {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name
        name: String,
        /// Mount point where device is mounted
        #[arg(long)]
        mount_point: PathBuf,
        /// Snapshot name to delete
        snapshot_name: String,
        /// Snapshot path (relative to mount point, defaults to .snapshots/<name>)
        #[arg(long)]
        snapshot_path: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum DatasetCommands {
    /// Create a new dataset
    Create {
        #[arg(short, long)]
        config: PathBuf,
        /// Dataset name (must be unique)
        name: String,
    },
    /// List all datasets
    List {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Delete a dataset
    Delete {
        #[arg(short, long)]
        config: PathBuf,
        /// Dataset name to delete
        name: String,
    },
    /// Get dataset information
    Info {
        #[arg(short, long)]
        config: PathBuf,
        /// Dataset name to query
        name: String,
    },
    /// Create a snapshot of a dataset
    Snapshot {
        #[arg(short, long)]
        config: PathBuf,
        /// Source dataset name
        source: String,
        /// Snapshot name (must be unique)
        name: String,
        /// Create read-only snapshot (default: read-write, like btrfs)
        #[arg(long)]
        readonly: bool,
    },
    /// List all snapshots
    ListSnapshots {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Delete a snapshot
    DeleteSnapshot {
        #[arg(short, long)]
        config: PathBuf,
        /// Snapshot name to delete
        name: String,
    },
    /// Set the default dataset
    SetDefault {
        #[arg(short, long)]
        config: PathBuf,
        /// Dataset name to set as default
        name: String,
    },
    /// Get the default dataset
    GetDefault {
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Restore a file from a snapshot
    Restore {
        #[arg(short, long)]
        config: PathBuf,
        /// Snapshot name to restore from
        #[arg(long)]
        snapshot: String,
        /// Path to file/directory within snapshot (e.g., /mnt/my-volume/file.txt)
        #[arg(long)]
        source: String,
        /// Destination path to restore to (e.g., /tmp/restored-file.txt)
        #[arg(long)]
        destination: String,
    },
    /// Clone a file or directory using COW (instant copy, no data duplication)
    Clone {
        #[arg(short, long)]
        config: PathBuf,
        /// Source path within ZeroFS (e.g., /mydir)
        #[arg(long)]
        source: String,
        /// Destination path within ZeroFS (e.g., /mydir-copy)
        #[arg(long)]
        destination: String,
    },
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
