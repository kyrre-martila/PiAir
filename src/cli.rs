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
    /// Short AirPlay 2 realtime test; choose a receiver if ID omitted. Requires terminal and y/yes approval.
    StreamTest {
        /// Omit to choose interactively; explicit IDs retain the advanced workflow.
        receiver_id: Option<String>,
        /// Receiver volume in integer dB (-60..=-10); may persist after playback.
        #[arg(long, default_value_t = -30, allow_hyphen_values = true, value_parser = clap::value_parser!(i32).range(-60..=-10))]
        volume_db: i32,
        #[arg(long, default_value_t=1, value_parser=clap::value_parser!(u32).range(1..=3))]
        seconds: u32,
        /// Validate initial SETUP and tear down before RECORD or audio transmission.
        #[arg(long, group = "owntone_diagnostic")]
        setup_check: bool,
        /// Validate RECORD after initial NTP SETUP, then tear down without audio.
        #[arg(long, group = "owntone_diagnostic", requires_all = ["experimental_owntone_transient", "receiver_id"], conflicts_with_all = ["setup_check", "volume_db", "seconds", "setup_profile", "setup_timeout_seconds"])]
        record_check: bool,
        /// Validate stream SETUP, then TEARDOWN; zero audio and volume changes.
        #[arg(long, group = "owntone_diagnostic", requires_all = ["experimental_owntone_transient", "receiver_id"], conflicts_with_all = ["volume_db", "seconds", "setup_profile", "setup_timeout_seconds"])]
        stream_setup_check: bool,
        /// One second faded tone at -40 dB receiver volume, which may persist.
        #[arg(long, group = "owntone_diagnostic", requires_all = ["experimental_owntone_transient", "receiver_id"], conflicts_with_all = ["volume_db", "seconds", "setup_profile", "setup_timeout_seconds"])]
        playback_check: bool,
        /// OwnTone-reference transient authentication for explicit diagnostics; no fallback.
        #[arg(long, requires_all = ["receiver_id", "owntone_diagnostic"], conflicts_with = "setup_profile")]
        experimental_owntone_transient: bool,
        /// Diagnostic only: wait up to 15 seconds total for initial SETUP.
        #[arg(long, requires = "setup_check", value_parser = clap::value_parser!(u64).range(15..=15))]
        setup_timeout_seconds: Option<u64>,
        /// Setup-only profile: a=control, b=early timing, c=early timing + pre-verify info.
        #[arg(long, requires = "setup_check", value_enum)]
        setup_profile: Option<crate::receiver::SetupProfile>,
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
    fn setup_timeout_is_explicit_and_scoped() {
        let base = ["piair", "stream-test", "device:aabbccddeeff"];
        let cli = Cli::try_parse_from(base).unwrap();
        assert!(matches!(
            cli.command,
            Command::StreamTest {
                setup_timeout_seconds: None,
                ..
            }
        ));
        assert!(Cli::try_parse_from(base.into_iter().chain([
            "--setup-check",
            "--setup-timeout-seconds",
            "15"
        ]))
        .is_ok());
        assert!(
            Cli::try_parse_from(base.into_iter().chain(["--setup-timeout-seconds", "15"])).is_err()
        );
        for value in ["0", "3", "14", "16"] {
            assert!(Cli::try_parse_from(base.into_iter().chain([
                "--setup-check",
                "--setup-timeout-seconds",
                value
            ]))
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "piair",
            "authenticate",
            "device:aabbccddeeff",
            "--setup-timeout-seconds",
            "15"
        ])
        .is_err());
    }
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

#[cfg(test)]
mod interactive_tests {
    use super::*;
    #[test]
    fn pairing_and_stream_volume_syntax() {
        assert!(matches!(
            Cli::try_parse_from(["piair", "pairing"]).unwrap().command,
            Command::Pairing
        ));
        for id in [None, Some("device:aabbccddeeff")] {
            for value in [None, Some("-15"), Some("-60"), Some("-10")] {
                let mut args = vec!["piair", "stream-test"];
                args.extend(id);
                if let Some(v) = value {
                    args.extend(["--volume-db", v]);
                }
                let Command::StreamTest {
                    receiver_id,
                    volume_db,
                    ..
                } = Cli::try_parse_from(args).unwrap().command
                else {
                    panic!()
                };
                assert_eq!(receiver_id.as_deref(), id);
                assert_eq!(
                    volume_db,
                    value.map(|v| v.parse::<i32>().unwrap()).unwrap_or(-30)
                );
            }
        }
        for v in [
            "-61",
            "-9",
            "NaN",
            "inf",
            "-inf",
            "-15.5",
            "oops",
            "999999999999",
        ] {
            assert!(Cli::try_parse_from(["piair", "stream-test", "--volume-db", v]).is_err());
        }
    }
}

#[cfg(test)]
mod owntone_setup_tests {
    use super::*;
    #[test]
    fn own_setup_requires_exact_id_and_setup_only_without_other_profiles() {
        let base = [
            "piair",
            "stream-test",
            "device:aabbccddeeff",
            "--setup-check",
            "--experimental-owntone-transient",
        ];
        let cli = Cli::try_parse_from(base).unwrap();
        assert!(matches!(
            cli.command,
            Command::StreamTest {
                experimental_owntone_transient: true,
                setup_check: true,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(base.into_iter().chain(["--setup-timeout-seconds", "15"])).is_ok()
        );
        for profile in ["a", "b", "c"] {
            assert!(
                Cli::try_parse_from(base.into_iter().chain(["--setup-profile", profile])).is_err()
            );
        }
        for args in [
            vec![
                "piair",
                "stream-test",
                "device:aabbccddeeff",
                "--experimental-owntone-transient",
            ],
            vec![
                "piair",
                "stream-test",
                "--setup-check",
                "--experimental-owntone-transient",
            ],
            vec![
                "piair",
                "stream-test",
                "device:aabbccddeeff",
                "--setup-check",
                "--experimental-owntone-transient",
                "--experimental-pyatv-transient-preamble",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        for args in [
            vec!["piair", "stream-test"],
            vec!["piair", "stream-test", "device:aabbccddeeff"],
            vec![
                "piair",
                "stream-test",
                "device:aabbccddeeff",
                "--setup-check",
            ],
        ] {
            assert!(matches!(
                Cli::try_parse_from(args).unwrap().command,
                Command::StreamTest {
                    experimental_owntone_transient: false,
                    ..
                }
            ));
        }
        // The existing authentication-only command keeps its separate scope.
        assert!(Cli::try_parse_from([
            "piair",
            "authenticate",
            "device:aabbccddeeff",
            "--experimental-owntone-transient",
            "--control-check"
        ])
        .is_ok());
    }
}

#[cfg(test)]
mod record_tests {
    use super::*;
    #[test]
    fn record_is_explicit_and_excludes_playback_and_other_diagnostics() {
        let base = [
            "piair",
            "stream-test",
            "device:3ae5c9a302ad",
            "--record-check",
            "--experimental-owntone-transient",
        ];
        assert!(Cli::try_parse_from(base).is_ok());
        for extra in [
            vec!["--setup-check"],
            vec!["--seconds", "1"],
            vec!["--volume-db", "-30"],
            vec!["--setup-profile", "a"],
            vec!["--setup-timeout-seconds", "15"],
            vec!["--experimental-pyatv-transient-preamble"],
        ] {
            assert!(Cli::try_parse_from(base.into_iter().chain(extra)).is_err());
        }
        assert!(Cli::try_parse_from([
            "piair",
            "stream-test",
            "device:3ae5c9a302ad",
            "--record-check"
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "piair",
            "stream-test",
            "--record-check",
            "--experimental-owntone-transient"
        ])
        .is_err());
    }
}

#[cfg(test)]
mod playback_diagnostic_tests {
    use super::*;
    #[test]
    fn both_stream_modes_are_explicit_exclusive_and_fixed() {
        for mode in ["--stream-setup-check", "--playback-check"] {
            let base = [
                "piair",
                "stream-test",
                "device:3ae5c9a302ad",
                "--experimental-owntone-transient",
                mode,
            ];
            assert!(Cli::try_parse_from(base).is_ok());
            for extra in [
                vec!["--seconds", "1"],
                vec!["--volume-db", "-40"],
                vec!["--record-check"],
                vec!["--setup-check"],
                vec!["--setup-profile", "a"],
                vec!["--setup-timeout-seconds", "15"],
            ] {
                assert!(Cli::try_parse_from(base.into_iter().chain(extra)).is_err());
            }
            assert!(Cli::try_parse_from([
                "piair",
                "stream-test",
                mode,
                "--experimental-owntone-transient"
            ])
            .is_err());
            assert!(
                Cli::try_parse_from(["piair", "stream-test", "device:3ae5c9a302ad", mode]).is_err()
            );
        }
        assert!(Cli::try_parse_from([
            "piair",
            "stream-test",
            "device:3ae5c9a302ad",
            "--stream-setup-check",
            "--playback-check",
            "--experimental-owntone-transient"
        ])
        .is_err());
    }
}
