//! Conservative activation of the installed stock system template via polkit.
//! Custom units/drop-ins stay a documented manual operation.
use crate::config::Loaded;
use anyhow::{ensure, Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug)]
pub(super) struct Service {
    pub unit: String,
    pub status: String,
    pub home: PathBuf,
    pub user: String,
}
fn output(args: &[&str]) -> Result<String> {
    let result = Command::new("systemctl")
        .args(["--system", "--no-pager", "--no-ask-password"])
        .args(args)
        .output()
        .context("systemctl unavailable")?;
    ensure!(
        result.status.success(),
        "systemd query failed; see docs/SYSTEMD.md"
    );
    Ok(String::from_utf8(result.stdout)?)
}
fn properties(text: &str) -> BTreeMap<&str, &str> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .collect()
}
fn trusted_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && m.uid() == 0 && m.mode() & 0o022 == 0,
        "service files must be root-owned regular files, unwritable by other users"
    );
    Ok(())
}
pub(super) fn inspect(loaded: &Loaded) -> Result<Service> {
    ensure!(
        Path::new("/run/systemd/system").is_dir(),
        "systemd is optional and is not running on this host"
    );
    let uid = rustix::process::geteuid().as_raw();
    ensure!(
        uid != 0,
        "activation requires the intended non-root account; run setup as that account"
    );
    let passwd = Command::new("getent")
        .args(["passwd", &uid.to_string()])
        .output()?;
    ensure!(
        passwd.status.success(),
        "cannot resolve current account through NSS"
    );
    let passwd = String::from_utf8(passwd.stdout)?;
    let rows: Vec<_> = passwd.lines().collect();
    ensure!(rows.len() == 1, "ambiguous current account");
    let fields: Vec<_> = rows[0].split(':').collect();
    ensure!(
        fields.len() == 7 && fields[2].parse::<u32>()? == uid,
        "invalid current account"
    );
    let user = fields[0];
    ensure!(
        !user.is_empty()
            && user
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            && !user.starts_with('-'),
        "account requires manual systemd escaping; see docs/SYSTEMD.md"
    );
    let unit = format!("piair@{user}.service");
    let text = output(&[
        "show",
        &unit,
        "-p",
        "LoadState",
        "-p",
        "ActiveState",
        "-p",
        "UnitFileState",
        "-p",
        "User",
        "-p",
        "Environment",
        "-p",
        "EnvironmentFiles",
        "-p",
        "DropInPaths",
        "-p",
        "FragmentPath",
        "-p",
        "ExecStart",
        "-p",
        "WorkingDirectory",
    ])?;
    let p = properties(&text);
    validate_properties(&p, user)?;
    let fragment = Path::new(p.get("FragmentPath").context("missing unit path")?);
    trusted_file(fragment)?;
    ensure!(
        std::fs::read_to_string(fragment)? == include_str!("../../../systemd/piair@.service"),
        "custom service template requires manual review; see docs/SYSTEMD.md"
    );
    trusted_file(Path::new("/usr/local/bin/piair"))?;
    let home = PathBuf::from(fields[5]);
    validate_config_path(loaded, &home)?;
    Ok(Service {
        unit,
        status: format!("{}, {}", p["ActiveState"], p["UnitFileState"]),
        home,
        user: user.into(),
    })
}
fn validate_properties(p: &BTreeMap<&str, &str>, user: &str) -> Result<()> {
    ensure!(p.get("LoadState")==Some(&"loaded"), "PiAir systemd template is not installed; installer is planned for PR #74; manual procedure: docs/SYSTEMD.md");
    ensure!(
        p.get("User") == Some(&user),
        "service account differs from current user"
    );
    for key in ["Environment", "EnvironmentFiles", "DropInPaths"] {
        ensure!(
            p.get(key) == Some(&""),
            "custom service environment/drop-ins require manual review: docs/SYSTEMD.md"
        );
    }
    ensure!(
        p.get("ExecStart")
            .is_some_and(|v| v.contains("argv[]=/usr/local/bin/piair run --allow-unattended ;")),
        "unexpected service executable or arguments"
    );
    ensure!(
        p.get("WorkingDirectory") == Some(&"~"),
        "unexpected service working directory"
    );
    ensure!(
        p.get("ActiveState")
            .is_some_and(|s| matches!(*s, "inactive" | "failed")),
        "service is already active or transitioning; restart it explicitly after review"
    );
    ensure!(p.contains_key("UnitFileState"), "missing unit state");
    Ok(())
}
fn validate_config_path(loaded: &Loaded, home: &Path) -> Result<()> {
    ensure!(home.is_absolute(), "account home must be absolute");
    let saved = std::fs::canonicalize(&loaded.path)?;
    let expected = std::fs::canonicalize(home.join(".config/piair/config.toml"))?;
    ensure!(saved==expected, "service resolves HOME/.config/piair/config.toml, which differs from saved config; configure a reviewed drop-in manually (docs/SYSTEMD.md)");
    Ok(())
}
pub(super) fn activate(loaded: &Loaded, expected: &Service) -> Result<()> {
    let current = inspect(loaded)?;
    ensure!(
        current.unit == expected.unit
            && current.user == expected.user
            && current.home == expected.home,
        "service identity changed; retry setup"
    );
    super::super::runtime::Runtime::prepare(loaded, loaded.config.playback.effective()?)?;
    // Release immediately before systemd starts the runtime, which acquires the same lock.
    drop(super::super::runtime::InstanceLock::acquire()?);
    let status = Command::new("systemctl")
        .args(["--system", "enable", "--now", &current.unit])
        .status()
        .context("systemctl activation failed")?;
    ensure!(status.success(), "service activation was not completed; authorization may have been denied. Check systemctl status and docs/SYSTEMD.md; enablement may already be saved");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_unit_wrong_identity_and_overrides_fail_closed() {
        let text="LoadState=loaded\nUser=pi\nActiveState=inactive\nUnitFileState=disabled\nEnvironment=\nEnvironmentFiles=\nDropInPaths=\nWorkingDirectory=~\nExecStart={ path=/usr/local/bin/piair ; argv[]=/usr/local/bin/piair run --allow-unattended ; }";
        validate_properties(&properties(text), "pi").unwrap();
        for (from, to) in [
            ("LoadState=loaded", "LoadState=not-found"),
            ("User=pi", "User=root"),
            ("Environment=\n", "Environment=PIAIR_CONFIG=/other\n"),
            ("DropInPaths=\n", "DropInPaths=/override\n"),
            ("ActiveState=inactive", "ActiveState=active"),
            (
                "--allow-unattended ;",
                "--allow-unattended --config other ;",
            ),
        ] {
            assert!(validate_properties(&properties(&text.replace(from, to)), "pi").is_err());
        }
    }
    #[test]
    fn config_path_must_match_account_home() {
        let d = tempfile::tempdir().unwrap();
        let default = d.path().join(".config/piair");
        std::fs::create_dir_all(&default).unwrap();
        let path = default.join("config.toml");
        std::fs::write(&path, "").unwrap();
        let loaded = crate::config::load(Some(path)).unwrap();
        validate_config_path(&loaded, d.path()).unwrap();
        assert!(validate_config_path(&loaded, Path::new("/other-account")).is_err());
    }
}
