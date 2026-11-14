#![forbid(unsafe_code)]

mod allowlist;
mod audio;
mod cli;
mod config;
mod discovery;
mod logging;
mod maintenance;
mod receiver;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, ConfigCommand};

fn run(cli: Cli) -> Result<()> {
    run_with_discovery(cli, discovery::discover)
}

// A pure discovery seam lets read-only dispatch tests use synthetic snapshots.
fn run_with_discovery(
    cli: Cli,
    discover: impl Fn(u64) -> Result<Vec<discovery::Receiver>>,
) -> Result<()> {
    if matches!(cli.command, Some(Command::Update | Command::Uninstall)) {
        return maintenance::run(matches!(cli.command, Some(Command::Update)));
    }
    let loaded = config::load(cli.config)?;
    let Some(command) = cli.command else {
        return receiver::setup::daily(loaded);
    };
    match command {
        Command::Update | Command::Uninstall => {
            unreachable!("maintenance dispatched before configuration")
        }
        Command::Setup => receiver::setup::run(loaded)?,
        Command::Run {
            receiver_id,
            allow_unattended,
            playback,
        } => {
            anyhow::ensure!(allow_unattended, "run requires explicit --allow-unattended");
            let settings = loaded
                .config
                .playback
                .patched(receiver_id, playback)?
                .effective()?;
            receiver::runtime::unattended(&loaded, settings, allow_unattended)?;
        }
        Command::Allowlist { command } => allowlist::run(&loaded, command)?,
        Command::Receivers => {
            receiver::list(&loaded, &discover(loaded.config.discovery_timeout_secs)?)?
        }
        Command::ReceiverPlan { method } => {
            let snapshot = discover(loaded.config.discovery_timeout_secs)?;
            print!(
                "{}",
                receiver::planning::diagnostic(&snapshot, |id| loaded.config.allows(id), method)
            );
        }
        Command::Pairing => receiver::interactive::pairing(&loaded)?,
        Command::Select {
            receiver_id,
            method,
        } => receiver::select(&loaded, &receiver_id, method)?,
        Command::Pair { receiver_id } => receiver::pair(
            &loaded,
            &receiver_id,
            false,
            false,
            receiver::AuthProfile::Legacy,
        )?,
        Command::Authenticate {
            receiver_id,
            control_check,
            experimental_pyatv_transient_preamble,
            experimental_owntone_transient,
        } => receiver::pair(
            &loaded,
            &receiver_id,
            true,
            control_check,
            if experimental_owntone_transient {
                receiver::AuthProfile::OwnToneTransient
            } else if experimental_pyatv_transient_preamble {
                receiver::AuthProfile::PyatvDiagnostic
            } else {
                receiver::AuthProfile::Legacy
            },
        )?,
        Command::StreamTest {
            receiver_id,
            seconds,
            volume_db,
            setup_check,
            record_check,
            stream_setup_check,
            playback_check,
            audio_file,
            setup_timeout_seconds,
            setup_profile,
            experimental_owntone_transient,
        } => receiver::stream_test(
            &loaded,
            receiver_id.as_deref(),
            seconds,
            volume_db,
            setup_check,
            record_check,
            stream_setup_check,
            playback_check,
            audio_file.as_deref(),
            setup_timeout_seconds,
            setup_profile,
            experimental_owntone_transient,
        )?,
        Command::PtpNegotiationCheck {
            receiver_id,
            experimental_owntone_transient,
            observation_seconds,
        } => receiver::ptp_negotiation_check(
            &loaded,
            &receiver_id,
            experimental_owntone_transient,
            observation_seconds,
        )?,
        Command::AutomaticPtpCapture {
            receiver_id,
            capture_device,
            threshold_dbfs,
            start_duration_ms,
            stop_duration_ms,
            volume_db,
            experimental_owntone_transient,
        } => receiver::automatic_capture(
            &loaded,
            &receiver_id,
            &capture_device,
            volume_db,
            experimental_owntone_transient,
            audio::activity::Settings {
                threshold_dbfs,
                start_ms: start_duration_ms,
                stop_ms: stop_duration_ms,
            },
        )?,
        Command::ExperimentalPtpCapture {
            receiver_id,
            capture_device,
            seconds,
            volume_db,
            experimental_owntone_transient,
        } => receiver::ptp_capture(
            &loaded,
            &receiver_id,
            &capture_device,
            seconds,
            volume_db,
            experimental_owntone_transient,
        )?,
        Command::PtpWavPlayback {
            receiver_id,
            audio_file,
            seconds,
            volume_db,
            experimental_owntone_transient,
        } => receiver::ptp_wav_playback(
            &loaded,
            &receiver_id,
            &audio_file,
            seconds,
            volume_db,
            experimental_owntone_transient,
        )?,
        Command::BufferedTest {
            receiver_id,
            setup_check,
            control_check,
            confirm_audio,
        } => {
            receiver::buffered_test(
                &loaded,
                &receiver_id,
                setup_check,
                confirm_audio,
                control_check,
            )?;
        }
        Command::HapStreamSetupCheck { receiver_id } => {
            receiver::hap_check(&loaded, receiver_id.as_deref(), false, -40)?
        }
        Command::HapPlaybackCheck {
            receiver_id,
            volume_db,
        } => receiver::hap_check(&loaded, receiver_id.as_deref(), true, volume_db)?,
        Command::Unpair { receiver_id } => receiver::unpair(&loaded, &receiver_id)?,
        Command::Audio { command } => audio::run(command, &loaded)?,
        Command::Status => receiver::setup::daily_status(&loaded)?,
        Command::Start => receiver::setup::daily_control(&loaded, "start")?,
        Command::Stop => receiver::setup::daily_control(&loaded, "stop")?,
        Command::Restart => receiver::setup::daily_control(&loaded, "restart")?,
        Command::Logs { follow } => receiver::setup::daily_logs(&loaded, follow, cli.verbose > 0)?,
        Command::Config { command } => match command {
            ConfigCommand::SetPlayback {
                receiver_id,
                playback,
            } => {
                config::save_playback(&loaded, receiver_id, playback)?;
            }
            ConfigCommand::Check => {
                println!("Valid: {} ({})", loaded.path.display(), loaded.source);
            }
            ConfigCommand::Show => print!("{}", toml::to_string_pretty(&loaded.config)?),
        },
        Command::Devices => {
            let receivers = discover(loaded.config.discovery_timeout_secs)?;
            println!("Passive mDNS snapshot (advertisements are unverified):");
            for receiver in &receivers {
                let approval = if loaded.config.allows(&receiver.id) {
                    "approved"
                } else {
                    "UNAPPROVED"
                };
                println!("{}  {}  {:?}", receiver.id, approval, receiver.name);
                for service in &receiver.services {
                    println!(
                        "  {} {:?} port={} IPs={:?} capabilities={:?}",
                        service.service_type,
                        service.fullname,
                        service.port,
                        service.addresses,
                        service.capabilities
                    );
                }
            }
            if receivers.is_empty() {
                println!("No resolved advertisements observed within the timeout.");
            }
        }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    if let Err(error) = cli.validate() {
        error.exit();
    }
    let detailed = cli.verbose > 0;
    logging::init(
        cli.verbose,
        matches!(cli.command, Some(Command::Run { .. })),
    );
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %receiver::setup::command_error_message(&error, detailed), "command failed");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod allowlist_dispatch_tests {
    use super::*;
    #[test]
    fn local_allowlist_dispatch_never_invokes_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "receiver_allowlist=['device:aabbccddeeff']").unwrap();
        for args in [
            vec!["list"],
            vec!["add", "device:aabbccddeeff"],
            vec!["remove", "device:112233445566"],
        ] {
            let cli = Cli::parse_from(
                ["piair", "--config", path.to_str().unwrap(), "allowlist"]
                    .into_iter()
                    .chain(args),
            );
            run_with_discovery(cli, |_| panic!("allowlist must never discover receivers")).unwrap();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
