use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod checkpoint;
pub mod debug;
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
    /// Export NBD device as formatted filesystem via NFS
    Export {
        #[arg(short, long)]
        config: PathBuf,
        /// Device name to export
        name: String,
        /// Device size (e.g., 10G, 512M, 1T)
        size: String,
        /// NBD device to use (e.g., /dev/nbd0)
        #[arg(long, default_value = "/dev/nbd0")]
        nbd_device: String,
        /// Filesystem type (btrfs, ext4, xfs, zfs)
        #[arg(long, default_value = "btrfs")]
        filesystem: String,
        /// Mount point for the filesystem
        #[arg(long)]
        mount_point: PathBuf,
        /// ZeroFS server address for NBD connection
        #[arg(long, default_value = "127.0.0.1")]
        nbd_host: String,
        /// ZeroFS NBD port
        #[arg(long, default_value = "10809")]
        nbd_port: u16,
    },
    /// Create a btrfs snapshot
    Snapshot {
        /// Mount point of the btrfs filesystem
        #[arg(long)]
        mount_point: PathBuf,
        /// Snapshot name (optional, defaults to timestamp)
        #[arg(long)]
        name: Option<String>,
        /// Source subvolume to snapshot (default: @)
        #[arg(long, default_value = "@")]
        subvolume: String,
        /// Create read-only snapshot
        #[arg(long)]
        readonly: bool,
    },
    /// List btrfs snapshots
    ListSnapshots {
        /// Mount point of the btrfs filesystem
        #[arg(long)]
        mount_point: PathBuf,
    },
    /// Delete a btrfs snapshot
    DeleteSnapshot {
        /// Mount point of the btrfs filesystem
        #[arg(long)]
        mount_point: PathBuf,
        /// Snapshot name to delete
        name: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
