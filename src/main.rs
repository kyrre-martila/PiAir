#![forbid(unsafe_code)]

mod cli;
mod config;
mod discovery;
mod logging;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command, ConfigCommand};

fn run(cli: Cli) -> Result<()> {
    let loaded = config::load(cli.config)?;
    match cli.command {
        Command::Status => {
            println!("PiAir {}: local CLI ready", env!("CARGO_PKG_VERSION"));
            println!("Configuration: {} ({})", loaded.path.display(), loaded.source);
            println!("Validation: valid");
            println!("Allowlisted receivers: {}", loaded.config.receiver_allowlist.len());
            println!("Audio capture and AirPlay playback: not implemented");
            println!("No service or receiver status was queried.");
        }
        Command::Config { command } => match command {
            ConfigCommand::Check => {
                println!("Valid: {} ({})", loaded.path.display(), loaded.source);
            }
            ConfigCommand::Show => print!("{}", toml::to_string_pretty(&loaded.config)?),
        },
        Command::Devices => {
            let receivers = discovery::discover(loaded.config.discovery_timeout_secs)?;
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
