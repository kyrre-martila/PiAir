use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, env, fs, io::ErrorKind, path::PathBuf};

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub discovery_timeout_secs: u64,
    pub receiver_allowlist: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
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
