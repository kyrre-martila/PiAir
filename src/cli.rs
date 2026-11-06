use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "PiAir: configuration and passive mDNS receiver discovery")]
pub struct Cli {
    /// Configuration file (overrides PIAIR_CONFIG and XDG defaults).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Increase diagnostic verbosity (-v debug, -vv trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Report this invocation's local application and configuration status.
    Status,
    /// Discover advertisements only; never select or connect to a receiver.
    Devices,
    /// Validate or display effective configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    Check,
    Show,
}
