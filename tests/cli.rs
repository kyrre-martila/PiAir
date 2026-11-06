use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_piair"))
        .args(args)
        .env_remove("PIAIR_CONFIG")
        .env_remove("XDG_CONFIG_HOME")
        .env("HOME", "/nonexistent/piair-cli-test")
        .output()
        .unwrap()
}

#[test]
fn help_and_version_ignore_missing_explicit_config() {
    for option in ["--help", "--version"] {
        let output = run(&["--config", "/nonexistent/piair.toml", option]);
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().contains("piair"));
    }
}

#[test]
fn local_commands_use_defaults_without_writing() {
    let output = run(&["status"]);
    assert!(output.status.success());
    let status = String::from_utf8(output.stdout).unwrap();
    assert!(status.contains("built-in defaults; file absent"));
    assert!(status.contains("AirPlay playback: not implemented"));
    assert!(run(&["config", "check"]).status.success());
    let output = run(&["config", "show"]);
    assert!(output.status.success());
    let shown = String::from_utf8(output.stdout).unwrap();
    assert!(shown.contains("discovery_timeout_secs = 5"));
    assert!(shown.contains("receiver_allowlist = []"));
}

#[test]
fn explicit_missing_config_is_an_actionable_failure() {
    let output = run(&["--config", "/nonexistent/piair.toml", "config", "check"]);
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot read /nonexistent/piair.toml"));
    assert!(error.contains("check path and read permissions"));
}
