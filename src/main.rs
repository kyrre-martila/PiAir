#![forbid(unsafe_code)]

mod audio;
mod cli;
mod config;
mod discovery;
mod logging;
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
    let loaded = config::load(cli.config)?;
    match cli.command {
        Command::Receivers => {
            receiver::list(&loaded, &discover(loaded.config.discovery_timeout_secs)?)?
        }
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
            setup_check,
        } => receiver::stream_test(&loaded, &receiver_id, seconds, setup_check)?,
        Command::Unpair { receiver_id } => receiver::unpair(&loaded, &receiver_id)?,
        Command::Audio { command } => audio::run(command, &loaded)?,
        Command::Status => {
            println!("PiAir {}: local CLI ready", env!("CARGO_PKG_VERSION"));
            println!(
                "Configuration: {} ({})",
                loaded.path.display(),
                loaded.source
            );
            println!("Validation: valid");
            println!(
                "Allowlisted receivers: {}",
                loaded.config.receiver_allowlist.len()
            );
            println!("Audio capture: available via audio monitor; not opened by status");
            println!("AirPlay playback: explicit stream-test available; hardware unverified; no active session");
            println!("No service or receiver status was queried.");
        }
        Command::Config { command } => match command {
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
    logging::init(cli.verbose);
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "command failed");
            std::process::ExitCode::FAILURE
        }
    }
}
