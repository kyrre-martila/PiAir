//! Direct maintenance commands do not load or change playback configuration.
use anyhow::{ensure, Context, Result};
use std::os::unix::{fs::MetadataExt, process::CommandExt};

pub fn run(update: bool) -> Result<()> {
    let helper = std::path::Path::new("/usr/local/share/piair/maintain.py");
    for path in [
        std::path::Path::new("/usr/local"),
        std::path::Path::new("/usr/local/share"),
        std::path::Path::new("/usr/local/share/piair"),
        helper,
    ] {
        let metadata = std::fs::symlink_metadata(path)
            .context("PiAir maintenance is not installed; see docs/INSTALL.md")?;
        ensure!(
            !metadata.file_type().is_symlink()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0,
            "maintenance files must be root-owned and unwritable by other users"
        );
    }
    let _installed = crate::receiver::setup::require_installed_executable()?;
    // Replace this process so maintenance itself is not mistaken for playback.
    // Python isolated mode ignores user modules and PYTHON* environment settings.
    Err(std::process::Command::new("/usr/bin/python3")
        .arg("-I")
        .arg(helper)
        .arg(if update { "update" } else { "uninstall" })
        .exec())
    .context("cannot start the installed maintenance helper (python3 required)")
}
