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
    /// Interactively authenticate the selected method; close without an audio session.
    Authenticate {
        receiver_id: String,
        /// Explicitly test encrypted RTSP OPTIONS on the same verified socket.
        #[arg(long)]
        control_check: bool,
        /// Controlled hardware experiment: use pinned pyatv HTTP framing for preamble, M1 and M3.
        #[arg(long)]
        experimental_pyatv_transient_preamble: bool,
        /// Authentication-only OwnTone-reference RTSP transient experiment; no fallback.
        #[arg(long, conflicts_with = "experimental_pyatv_transient_preamble")]
        experimental_owntone_transient: bool,
    },
    /// Explicit short AirPlay 2 realtime ALAC/44100/NTP test; interactive approval.
    StreamTest {
        receiver_id: String,
        #[arg(long, default_value_t=1, value_parser=clap::value_parser!(u32).range(1..=3))]
        seconds: u32,
        /// Validate initial SETUP and tear down before RECORD or audio transmission.
        #[arg(long)]
        setup_check: bool,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preamble_requires_explicit_flag() {
        for (args, expected) in [
            (vec!["piair", "authenticate", "device:aabbccddeeff"], false),
            (
                vec![
                    "piair",
                    "authenticate",
                    "device:aabbccddeeff",
                    "--experimental-pyatv-transient-preamble",
                ],
                true,
            ),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::Authenticate {
                experimental_pyatv_transient_preamble,
                ..
            } = cli.command
            else {
                panic!("authenticate expected")
            };
            assert_eq!(experimental_pyatv_transient_preamble, expected);
        }
    }
}
