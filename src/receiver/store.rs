//! Separate from ordinary TOML configuration. Filesystem permissions, not encryption.
use super::Selection;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub version: u8,
    pub receiver_id: String,
    pub pairing_id: String,
    pub controller_id: String,
    pub seed: [u8; 32],
    pub receiver_key: [u8; 32],
}
impl Credential {
    pub fn validate(&self, id: &str) -> Result<()> {
        super::exact_id(id)?;
        ensure!(
            self.version == 1 && self.receiver_id == id,
            "credential version/receiver mismatch"
        );
        ensure!(
            super::valid_pairing_id(&self.pairing_id)
                && uuid::Uuid::parse_str(&self.controller_id).is_ok(),
            "invalid credential identifiers"
        );
        ensure!(self.seed != [0; 32], "invalid credential private key");
        let key = ed25519_dalek::VerifyingKey::from_bytes(&self.receiver_key)
            .context("invalid credential public key")?;
        ensure!(!key.is_weak(), "weak credential public key");
        Ok(())
    }
}
pub struct Store {
    root: PathBuf,
}
impl Store {
    pub fn new(config_path: &Path) -> Result<Self> {
        let parent = config_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(Self {
            root: parent.join("receiver-state"),
        })
    }
    fn directory(&self, create: bool) -> Result<bool> {
        if create {
            // Parent config directory must already exist; never recursively follow/create paths.
            match fs::DirBuilder::new().mode(0o700).create(&self.root) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e).context("cannot create private receiver-state directory"),
            }
        }
        let meta = match fs::symlink_metadata(&self.root) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            meta.is_dir()
                && !meta.file_type().is_symlink()
                && meta.mode() & 0o777 == 0o700
                && meta.uid() == rustix::process::geteuid().as_raw(),
            "receiver-state must be an owned, nonsymlink 0700 directory"
        );
        Ok(true)
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        super::exact_id(id)?;
        Ok(self.root.join(format!("{}.toml", &id[7..])))
    }
    fn read(&self, path: &Path) -> Result<Option<Zeroizing<String>>> {
        if !self.directory(false)? {
            return Ok(None);
        }
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).context("cannot open receiver state safely"),
        };
        let m = file.metadata()?;
        ensure!(
            m.is_file()
                && m.nlink() == 1
                && m.mode() & 0o777 == 0o600
                && m.uid() == rustix::process::geteuid().as_raw()
                && m.len() <= 16384,
            "receiver state must be an owned regular 0600 file, at most 16 KiB"
        );
        let mut text = Zeroizing::new(String::new());
        file.take(16385).read_to_string(&mut text)?;
        ensure!(text.len() <= 16384, "receiver state too large");
        Ok(Some(text))
    }
    fn write(&self, path: &Path, text: &str, replace: bool) -> Result<()> {
        self.directory(true)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root)?;
        tmp.write_all(text.as_bytes())?;
        tmp.as_file().sync_all()?;
        if replace {
            tmp.persist(path)
        } else {
            tmp.persist_noclobber(path)
        }
        .map_err(|_| anyhow::anyhow!("cannot atomically save receiver state"))?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    pub fn credential(&self, id: &str) -> Result<Option<Credential>> {
        let Some(text) = self.read(&self.path(id)?)? else {
            return Ok(None);
        };
        let c: Credential = toml::from_str(&text)
            .map_err(|_| anyhow::anyhow!("corrupt credential; remove explicitly with unpair"))?;
        c.validate(id)?;
        Ok(Some(c))
    }
    pub fn save(&self, c: &Credential) -> Result<()> {
        c.validate(&c.receiver_id)?;
        ensure!(
            self.credential(&c.receiver_id)?.is_none(),
            "credentials already exist; unpair explicitly before replacement"
        );
        self.write(
            &self.path(&c.receiver_id)?,
            &Zeroizing::new(toml::to_string(c)?),
            false,
        )
    }
    pub fn selection(&self) -> Result<Option<Selection>> {
        self.read(&self.root.join("selected.toml"))?
            .map(|t| {
                toml::from_str(&t).map_err(|_| anyhow::anyhow!("corrupt selection; select again"))
            })
            .transpose()
    }
    pub fn select(&self, s: &Selection) -> Result<()> {
        self.write(&self.root.join("selected.toml"), &toml::to_string(s)?, true)
    }
    pub fn remove(&self, id: &str) -> Result<()> {
        let path = self.path(id)?;
        if self.directory(false)? {
            match fs::remove_file(path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
            File::open(&self.root)?.sync_all()?;
        }
        Ok(())
    }
}
