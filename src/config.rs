use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, env, fs, io::ErrorKind, path::PathBuf};

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub audio: crate::audio::Settings,
    pub playback: Playback,
    pub discovery_timeout_secs: u64,
    pub receiver_allowlist: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            audio: crate::audio::Settings::default(),
            playback: Playback::default(),
            discovery_timeout_secs: 5,
            receiver_allowlist: Vec::new(),
        }
    }
}

/// Canonical IDs are device:<12 lowercase hex digits>. Discovery accepts either
/// compact hex or six colon-separated octets, but config requires canonical IDs.
pub fn device_id(value: &str) -> Option<String> {
    let compact = if value.len() == 17 {
        let octets: Vec<_> = value.split(':').collect();
        if octets.len() != 6 || octets.iter().any(|part| part.len() != 2) {
            return None;
        }
        octets.concat()
    } else {
        value.to_string()
    };
    if compact.len() != 12 || !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let compact = compact.to_ascii_lowercase();
    if compact == "000000000000" || compact == "ffffffffffff" {
        return None;
    }
    Some(format!("device:{compact}"))
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        // Do not echo TOML parser errors: they can include source values.
        let config: Self = toml::from_str(text).map_err(|_| {
            anyhow::anyhow!(
                "invalid TOML or unknown/wrongly typed setting; compare config.example.toml"
            )
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.audio.validate()?;
        self.playback.validate()?;
        if !(1..=60).contains(&self.discovery_timeout_secs) {
            bail!("discovery_timeout_secs must be between 1 and 60");
        }
        let mut seen = BTreeSet::new();
        for (index, id) in self.receiver_allowlist.iter().enumerate() {
            let canonical = id.strip_prefix("device:").and_then(device_id);
            if canonical.as_ref() != Some(id) {
                bail!("receiver_allowlist entry {index} must be device:<12 lowercase hex digits>, excluding zero/broadcast IDs");
            }
            if !seen.insert(id) {
                bail!("receiver_allowlist entry {index} is duplicated; remove the duplicate");
            }
        }
        Ok(())
    }

    pub fn allows(&self, id: &str) -> bool {
        id.strip_prefix("device:").and_then(device_id).as_deref() == Some(id)
            && self.receiver_allowlist.iter().any(|allowed| allowed == id)
    }
}

pub struct Loaded {
    pub config: Config,
    pub path: PathBuf,
    pub source: &'static str,
}

fn resolve(
    cli: Option<PathBuf>,
    override_path: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<(PathBuf, bool)> {
    if let Some(path) = cli.or(override_path) {
        if path.as_os_str().is_empty() {
            bail!("explicit config path is empty; set --config or PIAIR_CONFIG to a TOML file");
        }
        return Ok((path, true));
    }
    let base = match xdg.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) if path.is_absolute() => path,
        Some(_) => bail!("XDG_CONFIG_HOME must be absolute; or supply --config"),
        None => home
            .filter(|path| path.is_absolute())
            .context("no absolute HOME or XDG_CONFIG_HOME; supply --config")?
            .join(".config"),
    };
    Ok((base.join("piair/config.toml"), false))
}

pub fn load(cli: Option<PathBuf>) -> Result<Loaded> {
    let (path, explicit) = resolve(
        cli,
        env::var_os("PIAIR_CONFIG").map(PathBuf::from),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )?;
    let (config, source) = match fs::read_to_string(&path) {
        Ok(text) => (
            Config::parse(&text)
                .with_context(|| format!("check configuration {}", path.display()))?,
            "file",
        ),
        Err(error) if error.kind() == ErrorKind::NotFound && !explicit => {
            tracing::debug!(path = %path.display(), "default file absent; using built-in defaults");
            (Config::default(), "built-in defaults; file absent")
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot read {}; check path and read permissions",
                    path.display()
                )
            });
        }
    };
    tracing::debug!(source, "configuration validated");
    Ok(Loaded {
        config,
        path,
        source,
    })
}

/// Re-read every authorization boundary, including atomic file replacements.
/// Cache only successful parsing of identical bytes; no metadata/time-based lease.
pub(crate) fn approval(path: &std::path::Path, id: &str) -> Result<bool> {
    thread_local! {
        static PARSED: std::cell::RefCell<Option<(PathBuf, String, Config)>> = const { std::cell::RefCell::new(None) };
    }
    let text = fs::read_to_string(path).context("cannot read authorization configuration")?;
    PARSED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if !cache
            .as_ref()
            .is_some_and(|(old_path, old_text, _)| old_path == path && old_text == &text)
        {
            let parsed = Config::parse(&text).context("authorization configuration invalid")?;
            *cache = Some((path.to_owned(), text, parsed));
        }
        Ok(cache.as_ref().expect("parsed above").2.allows(id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_example() {
        let defaults = Config::parse("").unwrap();
        assert_eq!(defaults.discovery_timeout_secs, 5);
        assert!(!defaults.allows("device:aabbccddeeff"));
        Config::parse(include_str!("../config.example.toml")).unwrap();
    }

    #[test]
    fn invalid_config() {
        for text in [
            "broken = [",
            "unknown = 1",
            "discovery_timeout_secs = 0",
            "discovery_timeout_secs = 61",
            "discovery_timeout_secs = -1",
            "discovery_timeout_secs = '5'",
            "receiver_allowlist = ['device:AABBCCDDEEFF']",
            "receiver_allowlist = ['device:000000000000']",
            "receiver_allowlist = ['service:living-room']",
            "receiver_allowlist = ['device:aabbccddeeff', 'device:aabbccddeeff']",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn allowlist_is_exact_and_default_deny() {
        let config = Config::parse("receiver_allowlist = ['device:aabbccddeeff']").unwrap();
        assert!(config.allows("device:aabbccddeeff"));
        assert!(!config.allows("device:112233445566"));
        assert!(!config.allows("device:AABBCCDDEEFF"));
        assert!(!config.allows("service:room._airplay._tcp.local."));
    }

    #[test]
    fn path_precedence_without_environment_mutation() {
        let p = |s: &str| Some(PathBuf::from(s));
        assert_eq!(
            resolve(p("cli"), p("env"), None, None).unwrap(),
            (PathBuf::from("cli"), true)
        );
        assert_eq!(
            resolve(None, p("env"), None, None).unwrap(),
            (PathBuf::from("env"), true)
        );
        assert_eq!(
            resolve(None, None, p("/xdg"), p("/home/u")).unwrap().0,
            PathBuf::from("/xdg/piair/config.toml")
        );
        assert_eq!(
            resolve(None, None, None, p("/home/u")).unwrap().0,
            PathBuf::from("/home/u/.config/piair/config.toml")
        );
        assert!(resolve(None, None, p("relative"), p("/home/u")).is_err());
        assert!(resolve(None, None, None, None).is_err());
    }
}

#[cfg(test)]
mod authorization_cache_tests {
    use super::*;
    #[test]
    fn identical_content_cache_never_hides_revocation_invalidity_or_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let id = "device:3ae5c9a302ad";
        fs::write(&path, format!("receiver_allowlist=['{id}']")).unwrap();
        assert!(approval(&path, id).unwrap());
        assert!(approval(&path, id).unwrap());
        let replacement = dir.path().join("new.toml");
        fs::write(&replacement, "receiver_allowlist=[]").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(!approval(&path, id).unwrap());
        fs::write(&path, "receiver_allowlist=3").unwrap();
        assert!(approval(&path, id).is_err());
        fs::remove_file(&path).unwrap();
        assert!(approval(&path, id).is_err());
    }
}

/// Operational settings only. Approval, selection, endpoint pins and secrets remain separate.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Playback {
    pub capture_device: Option<String>,
    pub receiver_id: Option<String>,
    pub authentication: Option<crate::receiver::runtime::Authentication>,
    pub threshold_dbfs: f64,
    pub start_duration_ms: u64,
    pub stop_duration_ms: u64,
    pub volume_db: Option<i32>,
}
impl Default for Playback {
    fn default() -> Self {
        Self {
            capture_device: None,
            receiver_id: None,
            authentication: None,
            threshold_dbfs: -45.0,
            start_duration_ms: 1000,
            stop_duration_ms: 30000,
            volume_db: None,
        }
    }
}
impl Playback {
    fn detection(&self) -> crate::audio::activity::Settings {
        crate::audio::activity::Settings {
            threshold_dbfs: self.threshold_dbfs,
            start_ms: self.start_duration_ms,
            stop_ms: self.stop_duration_ms,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.detection().validate()?;
        if let Some(id) = &self.receiver_id {
            anyhow::ensure!(
                id.strip_prefix("device:").and_then(device_id).as_ref() == Some(id),
                "playback.receiver_id must be a canonical device:<12 lowercase hex digits>"
            );
        }
        if let Some(device) = &self.capture_device {
            validate_capture_device(device)?;
        }
        anyhow::ensure!(
            self.volume_db.is_none_or(|v| (-60..=-10).contains(&v)),
            "volume must be -60..=-10 dB"
        );
        Ok(())
    }
    pub fn patched(
        &self,
        receiver_id: Option<String>,
        args: crate::cli::PlaybackArgs,
    ) -> Result<Self> {
        let mut value = self.clone();
        if let Some(id) = receiver_id {
            value.receiver_id = Some(id);
        }
        if let Some(device) = args.capture_device {
            value.capture_device = Some(device);
        }
        if let Some(auth) = args.authentication {
            value.authentication = Some(auth);
        }
        if let Some(threshold) = args.threshold_dbfs {
            value.threshold_dbfs = threshold;
        }
        if let Some(start) = args.start_duration_ms {
            value.start_duration_ms = start;
        }
        if let Some(stop) = args.stop_duration_ms {
            value.stop_duration_ms = stop;
        }
        if args.preserve_volume {
            value.volume_db = None;
        } else if args.volume_db.is_some() {
            value.volume_db = args.volume_db;
        }
        value.effective()?;
        Ok(value)
    }
    pub fn effective(&self) -> Result<crate::receiver::runtime::Settings> {
        let capture_device = self.capture_device.as_ref().context("missing playback.capture_device; use config set-playback --capture-device or run --capture-device")?;
        let receiver_id = self.receiver_id.as_ref().context(
            "missing playback.receiver_id; supply the approved, persistently selected receiver ID",
        )?;
        let authentication = self.authentication.context("missing playback.authentication; explicitly specify --authentication stored-hap or own-tone-transient")?;
        self.validate()?;
        Ok(crate::receiver::runtime::Settings {
            capture_device: capture_device.clone(),
            receiver_id: receiver_id.clone(),
            authentication,
            detection: self.detection(),
            volume_db: self.volume_db,
        })
    }
}

/// Patch only playback values, using the same snapshot/lock/publication as approval writers.
pub fn save_playback(
    loaded: &Loaded,
    id: Option<String>,
    args: crate::cli::PlaybackArgs,
) -> Result<()> {
    save_playback_checked(loaded, id, args, None)
}

/// Optional setup snapshot pins the settings reviewed by the user across confirmation.
pub(crate) fn save_playback_checked(
    loaded: &Loaded,
    id: Option<String>,
    args: crate::cli::PlaybackArgs,
    expected: Option<&Option<crate::allowlist::Snapshot>>,
) -> Result<()> {
    save_playback_impl(loaded, id, args, expected, false)
}

/// Selection changes span two files. On an uncertain config publication, restore
/// the reviewed previous config only if its bytes still equal our attempted write.
pub(crate) fn save_playback_for_selection(
    loaded: &Loaded,
    id: String,
    args: crate::cli::PlaybackArgs,
    expected: &Option<crate::allowlist::Snapshot>,
) -> Result<()> {
    save_playback_impl(loaded, Some(id), args, Some(expected), true)
}
fn save_playback_impl(
    loaded: &Loaded,
    id: Option<String>,
    args: crate::cli::PlaybackArgs,
    expected: Option<&Option<crate::allowlist::Snapshot>>,
    recover: bool,
) -> Result<()> {
    let original = crate::allowlist::snapshot(&loaded.path)?;
    if let Some(expected) = expected {
        anyhow::ensure!(
            &original == expected,
            "configuration changed during setup; rerun and review before saving"
        );
    }
    anyhow::ensure!(
        original.is_some() || loaded.source != "file",
        "configuration disappeared; rerun command"
    );
    let source = original.as_ref().map_or("", |s| s.text.as_str());
    let config = Config::parse(source)?;
    let playback = config.playback.patched(id, args)?;
    // This read-only preparation checks approval, persistent selection, profile and
    // credentials. It neither acquires a playback lock nor discovers/opens audio.
    crate::receiver::runtime::Runtime::prepare(loaded, playback.effective()?)?;
    let mut doc: toml_edit::DocumentMut = source.parse()?;
    if !doc.contains_key("playback") {
        doc["playback"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let serialized = toml::to_string(&playback)?;
    let values: toml_edit::DocumentMut = serialized.parse()?;
    for key in [
        "capture_device",
        "receiver_id",
        "authentication",
        "threshold_dbfs",
        "start_duration_ms",
        "stop_duration_ms",
        "volume_db",
    ] {
        if let Some(item) = values.get(key) {
            let mut item = item.clone();
            if let (Some(old), Some(new)) = (
                doc["playback"].get(key).and_then(toml_edit::Item::as_value),
                item.as_value_mut(),
            ) {
                *new.decor_mut() = old.decor().clone();
            }
            doc["playback"][key] = item;
        } else if let Some(table) = doc["playback"].as_table_like_mut() {
            // Preserve decorations when clearing volume by retaining them on the table.
            if let Some(old) = table.remove(key).and_then(|i| i.as_value().cloned()) {
                let comments = format!(
                    "{}{}",
                    old.decor().prefix().and_then(|s| s.as_str()).unwrap_or(""),
                    old.decor().suffix().and_then(|s| s.as_str()).unwrap_or("")
                );
                let trailing = format!("{}\n{}", doc.trailing().as_str().unwrap_or(""), comments);
                doc.set_trailing(trailing);
            }
        }
    }
    let text = doc.to_string();
    if let Err(error) = crate::allowlist::save(&loaded.path, original.as_ref(), &text) {
        if recover {
            if let Err(rollback) =
                restore_failed_playback_save(&loaded.path, original.as_ref(), &text)
            {
                return Err(error.context(format!("{rollback:#}")));
            }
        }
        return Err(error);
    }
    println!(
        "Saved playback settings to {}; restart PiAir to apply.",
        loaded.path.display()
    );
    Ok(())
}

fn restore_failed_playback_save(
    path: &std::path::Path,
    previous: Option<&crate::allowlist::Snapshot>,
    attempted: &str,
) -> Result<()> {
    let current = crate::allowlist::snapshot(path)
        .context("playback save uncertain; cannot inspect current config; run piair setup")?;
    if current.as_ref().map(|s| s.text.as_str()) == previous.map(|s| s.text.as_str()) {
        return Ok(());
    }
    let previous = previous.context(
        "playback config partially saved; no previous config to restore; run piair setup",
    )?;
    anyhow::ensure!(current.as_ref().is_some_and(|s| s.text == attempted),
        "playback config changed concurrently after uncertain save; not overwritten; run piair setup");
    crate::allowlist::save(path, current.as_ref(), &previous.text).context(
        "playback config rollback failed; settings may be partially saved; run piair setup",
    )
}

pub(crate) fn validate_capture_device(device: &str) -> Result<()> {
    anyhow::ensure!(!device.trim().is_empty() && device.len() <= 256 && !device.chars().any(char::is_control),
        "invalid ALSA device: use a nonempty identifier of at most 256 bytes without control characters");
    Ok(())
}

#[cfg(test)]
mod playback_tests {
    use super::*;
    use crate::{
        cli::{Cli, Command, PlaybackArgs},
        receiver::runtime::Authentication,
    };
    use clap::Parser;

    const STORED: &str = "[playback]\ncapture_device='synthetic'\nreceiver_id='device:aabbccddeeff'\nauthentication='own-tone-transient'\nthreshold_dbfs=-50.0\nstart_duration_ms=2000\nstop_duration_ms=40000\nvolume_db=-20\n";

    #[test]
    fn legacy_is_independent_and_required_information_is_never_inferred() {
        let legacy = Config::parse("[audio]\ndevice='legacy-device'\nstart_threshold_dbfs=-40.0\nstop_threshold_dbfs=-60.0").unwrap();
        assert!(legacy.playback.effective().is_err());
        assert_eq!(legacy.playback.threshold_dbfs, -45.0);
        assert_eq!(legacy.audio.start_threshold_dbfs, -40.0);
        for missing in ["capture_device", "receiver_id", "authentication"] {
            let text = STORED
                .lines()
                .filter(|line| !line.starts_with(missing))
                .collect::<Vec<_>>()
                .join("\n");
            let error = Config::parse(&text)
                .unwrap()
                .playback
                .effective()
                .err()
                .unwrap();
            assert!(error.to_string().contains(missing));
        }
    }

    #[test]
    fn cli_overrides_stored_values_and_defaults_without_clap_defaults_masking_them() {
        let stored = Config::parse(STORED).unwrap().playback;
        let Command::Run {
            receiver_id,
            playback,
            ..
        } = Cli::parse_from(["piair", "run", "--allow-unattended"])
            .command
            .unwrap()
        else {
            panic!()
        };
        let effective = stored
            .patched(receiver_id, playback)
            .unwrap()
            .effective()
            .unwrap();
        assert_eq!(effective.capture_device, "synthetic");
        assert_eq!(effective.detection.threshold_dbfs, -50.0);
        assert_eq!(effective.detection.start_ms, 2000);
        assert_eq!(effective.detection.stop_ms, 40000);
        assert_eq!(effective.volume_db, Some(-20));
        let Command::Run {
            receiver_id,
            playback,
            ..
        } = Cli::parse_from([
            "piair",
            "run",
            "device:112233445566",
            "--allow-unattended",
            "--capture-device",
            "override",
            "--authentication",
            "stored-hap",
            "--threshold-dbfs",
            "-44",
            "--start-duration-ms",
            "500",
            "--stop-duration-ms",
            "10000",
            "--preserve-volume",
        ])
        .command
        .unwrap()
        else {
            panic!()
        };
        let effective = stored
            .patched(receiver_id, playback)
            .unwrap()
            .effective()
            .unwrap();
        assert_eq!(effective.receiver_id, "device:112233445566");
        assert_eq!(effective.capture_device, "override");
        assert_eq!(effective.authentication, Authentication::StoredHap);
        assert_eq!(effective.detection.threshold_dbfs, -44.0);
        assert_eq!(effective.detection.start_ms, 500);
        assert_eq!(effective.detection.stop_ms, 10000);
        assert_eq!(effective.volume_db, None);
        assert!(Cli::try_parse_from(["piair", "run"]).is_err());
        assert!(Cli::try_parse_from([
            "piair",
            "run",
            "--allow-unattended",
            "--volume-db",
            "-20",
            "--preserve-volume"
        ])
        .is_err());
        let defaults = Playback::default()
            .patched(
                Some("device:aabbccddeeff".into()),
                PlaybackArgs {
                    capture_device: Some("synthetic".into()),
                    authentication: Some(Authentication::StoredHap),
                    ..Default::default()
                },
            )
            .unwrap()
            .effective()
            .unwrap();
        assert_eq!(defaults.detection.start_ms, 1000);
        assert_eq!(defaults.detection.stop_ms, 30000);
        assert_eq!(defaults.volume_db, None);
    }

    #[test]
    fn invalid_stored_values_are_rejected_before_runtime() {
        for setting in [
            "capture_device=''",
            "capture_device='  '",
            "capture_device=1",
            "receiver_id='alias'",
            "receiver_id='device:AABBCCDDEEFF'",
            "authentication='transient'",
            "authentication='legacy'",
            "threshold_dbfs=nan",
            "threshold_dbfs=inf",
            "threshold_dbfs=-121.0",
            "threshold_dbfs=0.1",
            "start_duration_ms=19",
            "start_duration_ms=9001",
            "stop_duration_ms=19",
            "stop_duration_ms=120001",
            "volume_db=-61",
            "volume_db=-9",
            "volume_db=-20.5",
            "secret='never'",
            "allow_unattended=true",
        ] {
            assert!(
                Config::parse(&format!("[playback]\n{setting}")).is_err(),
                "{setting}"
            );
        }
        assert!(validate_capture_device(&"a".repeat(257)).is_err());
        assert!(validate_capture_device("bad\ndevice").is_err());
    }
}

#[cfg(test)]
mod daily_persistence_tests {
    use super::*;
    #[test]
    fn uncertain_publication_restores_reviewed_config_but_preserves_concurrent_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let old = "# previous usable config\n";
        let attempted = "# attempted new config\n";
        fs::write(&path, old).unwrap();
        let previous = crate::allowlist::snapshot(&path).unwrap();
        // Model a directory-sync error after the atomic config rename.
        crate::allowlist::save(&path, previous.as_ref(), attempted).unwrap();
        restore_failed_playback_save(&path, previous.as_ref(), attempted).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), old);
        restore_failed_playback_save(&path, previous.as_ref(), attempted).unwrap();
        fs::write(&path, "# concurrent edit\n").unwrap();
        let error = restore_failed_playback_save(&path, previous.as_ref(), attempted).unwrap_err();
        assert!(error.to_string().contains("not overwritten"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "# concurrent edit\n");
    }
}
