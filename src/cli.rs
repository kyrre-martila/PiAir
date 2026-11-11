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
    /// Manage local receiver approval; never discover or contact receivers.
    Allowlist {
        #[command(subcommand)]
        command: AllowlistCommand,
    },
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
    /// Passive capability/topology snapshot and transport plan; no receiver connections or state writes.
    ReceiverPlan {
        /// Explicit authentication intent for planning only; never enrolls or authenticates.
        #[arg(long, value_enum)]
        method: Option<crate::receiver::compatibility::Method>,
    },
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
        /// Playback duration: 1..3 seconds for tone, 1..30 for WAV (stops earlier at EOF).
        #[arg(long, default_value_t=1, value_parser=clap::value_parser!(u32).range(1..=30))]
        seconds: u32,
        /// Bounded PCM WAV playback through the OwnTone realtime diagnostic.
        #[arg(long, group = "owntone_diagnostic", requires = "experimental_owntone_transient", conflicts_with_all = ["setup_check", "record_check", "stream_setup_check", "playback_check", "setup_profile", "setup_timeout_seconds"])]
        audio_file: Option<std::path::PathBuf>,
        /// Validate initial SETUP and tear down before RECORD or audio transmission.
        #[arg(long, group = "owntone_diagnostic")]
        setup_check: bool,
        /// Validate RECORD after initial NTP SETUP, then tear down without audio.
        #[arg(long, group = "owntone_diagnostic", requires = "experimental_owntone_transient", conflicts_with_all = ["setup_check", "volume_db", "seconds", "setup_profile", "setup_timeout_seconds"])]
        record_check: bool,
        /// Validate stream SETUP, then TEARDOWN; zero audio and volume changes.
        #[arg(long, group = "owntone_diagnostic", requires = "experimental_owntone_transient", conflicts_with_all = ["volume_db", "seconds", "setup_profile", "setup_timeout_seconds"])]
        stream_setup_check: bool,
        /// One second faded tone at -40 dB receiver volume, which may persist.
        #[arg(long, group = "owntone_diagnostic", requires = "experimental_owntone_transient", conflicts_with_all = ["volume_db", "seconds", "setup_profile", "setup_timeout_seconds"])]
        playback_check: bool,
        /// OwnTone-reference transient authentication for explicit diagnostics; no fallback.
        #[arg(
            long,
            requires = "owntone_diagnostic",
            conflicts_with = "setup_profile"
        )]
        experimental_owntone_transient: bool,
        /// Diagnostic only: wait up to 15 seconds total for initial SETUP.
        #[arg(long, requires = "setup_check", value_parser = clap::value_parser!(u64).range(15..=15))]
        setup_timeout_seconds: Option<u64>,
        /// Setup-only profile: a=control, b=early timing, c=early timing + pre-verify info.
        #[arg(long, requires = "setup_check", value_enum)]
        setup_profile: Option<crate::receiver::SetupProfile>,
    },
    /// Buffered PTP foundation diagnostic; live audio remains blocked pending clock-adoption evidence.
    BufferedTest {
        receiver_id: String,
        /// Opt in to initial SETUP and one second of PTP observation, with terminal confirmation.
        #[arg(long, conflicts_with = "confirm_audio")]
        setup_check: bool,
        /// Opt in through RECORD, stream/audioMode, or paused-anchor diagnostics; confirms every stage in a terminal.
        #[arg(long, value_enum, conflicts_with_all = ["setup_check", "confirm_audio"])]
        control_check: Option<crate::receiver::lifecycle::Stage>,
        /// Explicit audio intent; currently fails locally because the live timing gate is closed.
        #[arg(long)]
        confirm_audio: bool,
    },
    /// Stored HAP: validate stream SETUP with zero audio/volume changes; omit ID for picker.
    #[command(visible_alias = "stua-stream-setup-check")]
    HapStreamSetupCheck { receiver_id: Option<String> },
    /// Stored HAP: one second faded tone, persistent receiver volume, two seconds silent drain.
    #[command(visible_alias = "stua-playback-check")]
    HapPlaybackCheck {
        /// Canonical ID; omit to use the existing interactive receiver picker.
        receiver_id: Option<String>,
        /// Receiver volume in dB (-40 through -10); may persist after the test.
        #[arg(long, default_value_t = -40, allow_negative_numbers = true, value_parser = clap::value_parser!(i32).range(-40..=-10))]
        volume_db: i32,
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
    fn own_setup_requires_explicit_diagnostic_without_other_profiles() {
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
        .is_ok());
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
            .is_ok());
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

#[cfg(test)]
mod stua_volume_tests {
    use super::*;

    #[test]
    fn stua_playback_volume_defaults_and_valid_values() {
        for (args, expected) in [
            (vec![], -40),
            (vec!["--volume-db", "-20"], -20),
            (vec!["--volume-db", "-40"], -40),
            (vec!["--volume-db", "-10"], -10),
        ] {
            let cli = Cli::try_parse_from(
                ["piair", "stua-playback-check", "device:02d899c1833f"]
                    .into_iter()
                    .chain(args),
            )
            .unwrap();
            assert!(
                matches!(cli.command, Command::HapPlaybackCheck { volume_db, .. } if volume_db == expected)
            );
        }
    }

    #[test]
    fn stua_playback_rejects_invalid_or_missing_volume() {
        for args in [vec!["-41"], vec!["-9"], vec!["loud"], vec!["-20.5"], vec![]] {
            let error = Cli::try_parse_from(
                [
                    "piair",
                    "stua-playback-check",
                    "device:02d899c1833f",
                    "--volume-db",
                ]
                .into_iter()
                .chain(args),
            )
            .err()
            .expect("invalid volume must fail Clap validation");
            assert!(matches!(
                error.kind(),
                clap::error::ErrorKind::ValueValidation | clap::error::ErrorKind::InvalidValue
            ));
        }
    }
}

#[derive(Subcommand)]
#[command(
    after_help = "Examples:\n  piair allowlist list\n  piair allowlist add device:aabbccddeeff\n  piair allowlist remove device:aabbccddeeff\n\nChanges require an interactive terminal and y/yes. Adding requires independent physical identity verification (device label or trusted local settings); mDNS alone is insufficient. Approval grants eligibility for later receiver operations, not authentication."
)]
pub enum AllowlistCommand {
    /// List exact approved IDs and the resolved configuration path.
    List,
    /// Approve an independently physically verified canonical receiver ID.
    Add { receiver_id: String },
    /// Revoke local approval; stored credentials are retained, no receiver contact.
    Remove { receiver_id: String },
}

#[cfg(test)]
mod dynamic_diagnostic_tests {
    use super::*;
    #[test]
    fn diagnostic_commands_accept_ids_or_existing_picker_and_legacy_aliases() {
        for command in [
            "hap-stream-setup-check",
            "hap-playback-check",
            "stua-stream-setup-check",
            "stua-playback-check",
        ] {
            for id in [
                None,
                Some("device:112233445566"),
                Some("device:66778899aabb"),
            ] {
                assert!(Cli::try_parse_from(["piair", command].into_iter().chain(id)).is_ok());
            }
        }
        for mode in [
            "--setup-check",
            "--record-check",
            "--stream-setup-check",
            "--playback-check",
        ] {
            for id in [
                None,
                Some("device:112233445566"),
                Some("device:66778899aabb"),
            ] {
                assert!(Cli::try_parse_from(
                    ["piair", "stream-test"]
                        .into_iter()
                        .chain(id)
                        .chain([mode, "--experimental-owntone-transient"])
                )
                .is_ok());
            }
        }
    }
}

impl Cli {
    pub fn validate(&self) -> Result<(), clap::Error> {
        if let Command::StreamTest {
            seconds,
            audio_file,
            ..
        } = &self.command
        {
            if *seconds > 3 && audio_file.is_none() {
                return Err(clap::Error::raw(
                    clap::error::ErrorKind::ArgumentConflict,
                    "--seconds above 3 requires --audio-file (WAV maximum: 30 seconds)",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod wav_arguments {
    use super::*;
    #[test]
    fn wav_modes_and_bounds() {
        let base = [
            "piair",
            "stream-test",
            "--experimental-owntone-transient",
            "--audio-file",
            "test.wav",
        ];
        for seconds in ["1", "30"] {
            let c = Cli::try_parse_from(base.into_iter().chain([
                "--seconds",
                seconds,
                "--volume-db",
                "-20",
            ]))
            .unwrap();
            assert!(c.validate().is_ok());
        }
        for flag in [
            "--setup-check",
            "--record-check",
            "--stream-setup-check",
            "--playback-check",
        ] {
            assert!(Cli::try_parse_from(base.into_iter().chain([flag])).is_err());
        }
        for seconds in ["0", "31"] {
            assert!(Cli::try_parse_from(base.into_iter().chain(["--seconds", seconds])).is_err());
        }
        assert!(Cli::try_parse_from(["piair", "stream-test", "--audio-file", "x.wav"]).is_err());
        assert!(
            Cli::try_parse_from(["piair", "stream-test", "--seconds", "4"])
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
