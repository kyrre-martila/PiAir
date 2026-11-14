use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Send Raspberry Pi audio to your selected AirPlay speaker",
    after_help = "Run piair without a command in a terminal to open the daily menu.\nStart with piair setup. List audio inputs with piair audio devices.\nAdvanced configuration: piair config --help; audio troubleshooting: piair audio --help."
)]
pub struct Cli {
    /// Configuration file (overrides PIAIR_CONFIG and XDG defaults).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Increase diagnostic verbosity (-v debug, -vv trace).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Internal fixed recording operation; invoked only by the requested temporary service.
    #[command(hide = true)]
    ServiceRecording {
        #[arg(long)]
        saved: String,
    },
    /// Check for a stable official release and update the installed application.
    #[command(display_order = 9)]
    Update,
    /// Remove the installed application, preserving personal configuration and credentials.
    #[command(display_order = 10)]
    Uninstall,
    /// Set up PiAir or change settings (requires a terminal).
    #[command(display_order = 0)]
    Setup,
    /// Start PiAir playback.
    #[command(display_order = 2)]
    Start,
    /// Stop PiAir playback.
    #[command(display_order = 3)]
    Stop,
    /// Restart PiAir playback.
    #[command(display_order = 4)]
    Restart,
    /// Show recent service logs.
    #[command(display_order = 5)]
    Logs {
        #[arg(long)]
        follow: bool,
    },
    /// Foreground automatic playback without prompts; requires existing approval and selection.
    #[command(hide = true)]
    Run {
        receiver_id: Option<String>,
        /// Explicit permission for this invocation only; saved settings never authorize playback.
        #[arg(long, required = true)]
        allow_unattended: bool,
        #[command(flatten)]
        playback: PlaybackArgs,
    },
    /// Manage local receiver approval; never discover or contact receivers.
    Allowlist {
        #[command(subcommand)]
        command: AllowlistCommand,
    },
    /// Capture-only audio tools; no receiver discovery or connections.
    #[command(display_order = 7)]
    Audio {
        #[command(subcommand)]
        command: AudioCommand,
    },
    /// Show current service state and saved playback configuration.
    #[command(display_order = 1)]
    Status,
    /// Discover advertisements only; never select or connect to a receiver.
    #[command(display_order = 6)]
    Devices,
    /// Full passive receiver details: IDs, addresses, services and local approval/pairing state.
    Receivers,
    /// Enroll persistent credentials with an approved receiver; requires verified PIN delivery or a genuine HAP setup code.
    Pairing,
    /// Pin an already allowlisted receiver's discovered endpoint; no connection.
    Select {
        receiver_id: String,
        /// Explicit protocol profile, never inferred or automatically changed.
        #[arg(long, value_enum, default_value = "pin-display")]
        method: crate::receiver::compatibility::Method,
    },
    /// Enroll persistent credentials using the selected method and its genuine PIN/setup code.
    Pair { receiver_id: String },
    /// Remove local credentials without contacting the receiver.
    Unpair { receiver_id: String },
    /// Validate or display effective configuration.
    #[command(display_order = 8)]
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
    /// Save validated automatic playback settings locally; no audio or receiver contact.
    SetPlayback {
        receiver_id: Option<String>,
        #[command(flatten)]
        playback: PlaybackArgs,
    },
    Check,
    Show,
}

#[derive(Subcommand)]
#[command(
    after_help = "Examples:\n  piair allowlist list\n  piair allowlist add device:aabbccddeeff\n  piair allowlist remove device:aabbccddeeff\n\nChanges require an interactive terminal and y/yes. Adding asks for one explicit approval of the exact canonical receiver ID. Approval permits later connections; it does not authenticate or cryptographically verify the receiver. Discovered receivers are never automatically approved."
)]
pub enum AllowlistCommand {
    /// List exact approved IDs and the resolved configuration path.
    List,
    /// Approve an exact canonical receiver ID for use with PiAir.
    Add { receiver_id: String },
    /// Revoke local approval; stored credentials are retained, no receiver contact.
    Remove { receiver_id: String },
}

/// Shared patch arguments for run and the scriptable configuration writer.
#[derive(clap::Args, Default)]
pub struct PlaybackArgs {
    #[arg(long)]
    pub capture_device: Option<String>,
    #[arg(long, value_enum)]
    pub authentication: Option<crate::receiver::runtime::Authentication>,
    #[arg(long, allow_negative_numbers = true)]
    pub threshold_dbfs: Option<f64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(20..=9000))]
    pub start_duration_ms: Option<u64>,
    #[arg(long, value_parser = clap::value_parser!(u64).range(20..=120000))]
    pub stop_duration_ms: Option<u64>,
    #[arg(
        long,
        allow_negative_numbers = true,
        conflicts_with = "preserve_volume",
        value_parser = clap::value_parser!(i32).range(-60..=-10)
    )]
    pub volume_db: Option<i32>,
    /// Clear a saved volume for set-playback, or preserve receiver volume for this run.
    #[arg(long)]
    pub preserve_volume: bool,
}
