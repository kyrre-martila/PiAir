use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "PiAir: ALSA audio activity, configuration and passive mDNS discovery"
)]
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
    /// Capture-only audio tools; no receiver discovery or connections.
    Audio {
        #[command(subcommand)]
        command: AudioCommand,
    },
    /// Report this invocation's local application and configuration status.
    Status,
    /// Discover advertisements only; never select or connect to a receiver.
    Devices,
    /// Passive receiver snapshot with local approval/pairing state.
    Receivers,
    /// Pin an already allowlisted receiver's discovered endpoint; no connection.
    Select {
        receiver_id: String,
        /// Explicit protocol profile, never inferred or automatically changed.
        #[arg(long, value_enum, default_value = "pin-display")]
        method: crate::receiver::compatibility::Method,
    },
    /// Interactively request a receiver PIN and establish persistent credentials.
    Pair { receiver_id: String },
    /// Interactively verify stored credentials; close without an audio session.
    Authenticate { receiver_id: String },
    /// Remove local credentials without contacting the receiver.
    Unpair { receiver_id: String },
    /// Validate or display effective configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
pub enum AudioCommand {
    /// List ALSA PCM capture hints without opening a capture stream.
    Devices,
    /// Display levels and activity intent until Ctrl+C or the duration expires.
    Monitor {
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=86400))]
        duration_secs: Option<u64>,
    },
    /// Measure idle noise and propose thresholds, saving only on confirmation.
    Calibrate {
        #[arg(long, value_parser = clap::value_parser!(u64).range(5..=300))]
        duration_secs: Option<u64>,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    Check,
    Show,
}
