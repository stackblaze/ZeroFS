use anyhow::{Context, Result};
use std::io::BufRead;

mod bucket_identity;
mod cache;
mod checkpoint_manager;
mod cli;
mod config;
mod control;
mod deku_bytes;
mod encryption;
mod fs;
mod key_management;
mod nbd;
mod nfs;
mod ninep;
mod parse_object_store;
mod rpc;
mod storage_compatibility;
mod task;

#[cfg(test)]
mod test_helpers;

#[cfg(test)]
mod posix_tests;

#[cfg(feature = "failpoints")]
mod failpoints;

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse_args();

    match cli.command {
        cli::Commands::Init { path } => {
            println!("Generating configuration file at: {}", path.display());
            config::Settings::write_default_config(&path)?;
            println!("Configuration file created successfully!");
            println!("Edit the file and run: zerofs run -c {}", path.display());
        }
        cli::Commands::ChangePassword { config } => {
            let settings = match config::Settings::from_file(&config) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("✗ Failed to load config: {:#}", e);
                    std::process::exit(1);
                }
            };

            eprintln!("Reading new password from stdin...");
            let mut new_password = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut new_password)
                .context("Failed to read password from stdin")?;
            let new_password = new_password.trim().to_string();
            eprintln!("New password read successfully.");

            eprintln!("Changing encryption password...");
            match cli::password::change_password(&settings, new_password).await {
                Ok(()) => {
                    println!("✓ Encryption password changed successfully!");
                    println!(
                        "ℹ To use the new password, update your config file or environment variable"
                    );
                }
                Err(e) => {
                    eprintln!("✗ Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        cli::Commands::Run {
            config,
            read_only,
            checkpoint,
            no_compactor,
        } => {
            cli::server::run_server(config, read_only, checkpoint, no_compactor).await?;
        }
        cli::Commands::Debug { subcommand } => match subcommand {
            cli::DebugCommands::ListKeys { config } => {
                cli::debug::list_keys(config).await?;
            }
        },
        cli::Commands::Checkpoint { subcommand } => match subcommand {
            cli::CheckpointCommands::Create { config, name } => {
                cli::checkpoint::create_checkpoint(&config, &name).await?;
            }
            cli::CheckpointCommands::List { config } => {
                cli::checkpoint::list_checkpoints(&config).await?;
            }
            cli::CheckpointCommands::Delete { config, name } => {
                cli::checkpoint::delete_checkpoint(&config, &name).await?;
            }
            cli::CheckpointCommands::Info { config, name } => {
                cli::checkpoint::get_checkpoint_info(&config, &name).await?;
            }
        },
        cli::Commands::Nbd { subcommand } => match subcommand {
            cli::NbdCommands::Create { config, name, size } => {
                cli::nbd::create_device(config, name, size).await?;
            }
            cli::NbdCommands::List { config } => {
                cli::nbd::list_devices(config).await?;
            }
            cli::NbdCommands::Delete {
                config,
                name,
                force,
            } => {
                cli::nbd::delete_device(config, name, force).await?;
            }
            cli::NbdCommands::Resize { config, name, size } => {
                cli::nbd::resize_device(config, name, size).await?;
            }
            cli::NbdCommands::Format {
                config,
                name,
                filesystem,
                mkfs_options,
            } => {
                cli::nbd::format_device(config, name, filesystem, mkfs_options).await?;
            }
            cli::NbdCommands::Export {
                config,
                name,
                mount_point,
                nbd_device,
                filesystem,
                nfs_export_path,
                nfs_options,
            } => {
                cli::nbd::export_device(
                    config,
                    name,
                    mount_point,
                    nbd_device,
                    filesystem,
                    nfs_export_path,
                    nfs_options,
                )
                .await?;
            }
            cli::NbdCommands::Unexport {
                config,
                name,
                mount_point,
                nbd_device,
            } => {
                cli::nbd::unexport_device(config, name, mount_point, nbd_device).await?;
            }
            cli::NbdCommands::Snapshot {
                config,
                name,
                mount_point,
                snapshot_name,
                snapshot_path,
                read_only,
            } => {
                cli::nbd::create_snapshot(
                    config,
                    name,
                    mount_point,
                    snapshot_name,
                    snapshot_path,
                    read_only,
                )
                .await?;
            }
            cli::NbdCommands::Snapshots {
                config,
                name,
                mount_point,
            } => {
                cli::nbd::list_snapshots(config, name, mount_point).await?;
            }
            cli::NbdCommands::Restore {
                config,
                name,
                mount_point,
                snapshot_name,
                snapshot_path,
                target_path,
            } => {
                cli::nbd::restore_snapshot(
                    config,
                    name,
                    mount_point,
                    snapshot_name,
                    snapshot_path,
                    target_path,
                )
                .await?;
            }
            cli::NbdCommands::DeleteSnapshot {
                config,
                name,
                mount_point,
                snapshot_name,
                snapshot_path,
            } => {
                cli::nbd::delete_snapshot(config, name, mount_point, snapshot_name, snapshot_path)
                    .await?;
            }
        },
        cli::Commands::Dataset { subcommand } => match subcommand {
            cli::DatasetCommands::Create { config, name } => {
                cli::dataset::create_dataset(&config, &name).await?;
            }
            cli::DatasetCommands::List { config } => {
                cli::dataset::list_datasets(&config).await?;
            }
            cli::DatasetCommands::Delete { config, name } => {
                cli::dataset::delete_dataset(&config, &name).await?;
            }
            cli::DatasetCommands::Info { config, name } => {
                cli::dataset::get_dataset_info(&config, &name).await?;
            }
            cli::DatasetCommands::Snapshot {
                config,
                source,
                name,
                readonly,
            } => {
                cli::dataset::create_snapshot(&config, &source, &name, readonly).await?;
            }
            cli::DatasetCommands::ListSnapshots { config } => {
                cli::dataset::list_snapshots(&config).await?;
            }
            cli::DatasetCommands::DeleteSnapshot { config, name } => {
                cli::dataset::delete_snapshot(&config, &name).await?;
            }
            cli::DatasetCommands::SetDefault { config, name } => {
                cli::dataset::set_default_dataset(&config, &name).await?;
            }
            cli::DatasetCommands::GetDefault { config } => {
                cli::dataset::get_default_dataset(&config).await?;
            }
            cli::DatasetCommands::Restore {
                config,
                snapshot,
                source,
                destination,
            } => {
                cli::dataset::restore_from_snapshot(&config, &snapshot, &source, &destination)
                    .await?;
            }
        },
        cli::Commands::Fatrace { config } => {
            cli::fatrace::run_fatrace(config).await?;
        }
        cli::Commands::Compactor { config } => {
            cli::compactor::run_compactor(config).await?;
        }
    }

    Ok(())
}
