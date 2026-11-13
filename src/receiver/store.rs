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
    pub advertised_pi: Option<String>,
    pub controller_id: String,
    pub seed: [u8; 32],
    pub receiver_key: [u8; 32],
}

impl Credential {
    /// Version 2 is exclusively the signed persistent HAP enrollment result.
    /// PIN delivery is not stored, and is irrelevant to existing-key pair-verify.
    pub(super) fn authentication_method(&self, id: &str) -> Result<super::compatibility::Method> {
        self.validate(id)?;
        Ok(super::compatibility::Method::StoredHap)
    }
    pub(super) fn fingerprint(&self) -> Result<Zeroizing<[u8; 64]>> {
        let serialized = Zeroizing::new(toml::to_string(self)?);
        Ok(super::crypto::hash(&[serialized.as_bytes()]))
    }

    pub fn validate(&self, id: &str) -> Result<()> {
        super::exact_id(id)?;
        ensure!(
            self.version == 2 && self.receiver_id == id,
            "credential version/receiver mismatch; old PR #3 credentials require explicit unpair and re-enrollment"
        );
        ensure!(
            super::valid_pairing_id(&self.pairing_id)
                && self
                    .advertised_pi
                    .as_deref()
                    .is_none_or(super::valid_pairing_id)
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
pub(super) struct CredentialSnapshot {
    pub value: Credential,
    pub fingerprint: Zeroizing<[u8; 64]>,
}
pub struct Store {
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    #[serde(with = "super::hex_u64")]
    client: u64,
    remote: u32,
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
    pub fn client_identity(&self) -> Result<(u64, u32)> {
        self.client_identity_with(|| (rand::random::<u64>().max(1), rand::random::<u32>().max(1)))
    }
    fn client_identity_with(&self, generate: impl FnOnce() -> (u64, u32)) -> Result<(u64, u32)> {
        let path = self.root.join("client-identity.toml");
        if let Some(text) = self.read(&path)? {
            let identity: Identity =
                toml::from_str(&text).context("invalid PiAir client identity")?;
            ensure!(
                identity.client != 0 && identity.remote != 0,
                "zero client identity"
            );
            return Ok((identity.client, identity.remote));
        }
        let (client, remote) = generate();
        ensure!(client != 0 && remote != 0, "zero client identity");
        let identity = Identity { client, remote };
        self.write(&path, &toml::to_string(&identity)?, false)?;
        Ok((identity.client, identity.remote))
    }
    pub fn credential(&self, id: &str) -> Result<Option<Credential>> {
        Ok(self.credential_snapshot(id)?.map(|snapshot| snapshot.value))
    }
    /// Parse and fingerprint the same protected read, avoiding a validation/snapshot race.
    pub(super) fn credential_snapshot(&self, id: &str) -> Result<Option<CredentialSnapshot>> {
        let Some(text) = self.read(&self.path(id)?)? else {
            return Ok(None);
        };
        let c: Credential = toml::from_str(&text)
            .map_err(|_| anyhow::anyhow!("corrupt credential; remove explicitly with unpair"))?;
        c.validate(id)?;
        Ok(Some(CredentialSnapshot {
            value: c,
            fingerprint: super::crypto::hash(&[text.as_bytes()]),
        }))
    }
    /// Hot-path revocation check: validate filesystem protections and exact bytes,
    /// without repeatedly parsing TOML/Ed25519 keys on the audio sample timeline.
    pub(super) fn credential_fingerprint(&self, id: &str) -> Result<Option<Zeroizing<[u8; 64]>>> {
        Ok(self
            .read(&self.path(id)?)?
            .map(|text| super::crypto::hash(&[text.as_bytes()])))
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
    // Serialize all application selection writers across the compare and atomic rename.
    // Lock the private directory, so cancellation creates no lock/state artifacts.
    fn selection_lock(&self) -> Result<File> {
        self.directory(true)?;
        let lock = File::open(&self.root)?;
        lock.try_lock()
            .context("receiver selection is being changed; retry explicitly")?;
        Ok(lock)
    }
    pub fn select(&self, s: &Selection) -> Result<()> {
        let _lock = self.selection_lock()?;
        self.write(&self.root.join("selected.toml"), &toml::to_string(s)?, true)
    }
    pub fn select_if_unchanged(&self, s: &Selection, expected: Option<&Selection>) -> Result<()> {
        let _lock = self.selection_lock()?;
        ensure!(
            self.selection()?.as_ref() == expected,
            "receiver selection changed during confirmation; review and retry explicitly"
        );
        if expected != Some(s) {
            self.write(&self.root.join("selected.toml"), &toml::to_string(s)?, true)?;
        }
        Ok(())
    }
    /// Hold the selection writer lock across publication and the checked playback save.
    /// A crash between files leaves a mismatch, which Runtime::prepare rejects.
    pub(super) fn change_with_playback(
        &self,
        selected: &Selection,
        expected: &Selection,
        save: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let _lock = self.selection_lock()?;
        ensure!(
            self.selection()?.as_ref() == Some(expected),
            "selection changed; retry"
        );
        let path = self.root.join("selected.toml");
        let publish = self.write(&path, &toml::to_string(selected)?, true);
        let result = publish.and_then(|()| save());
        if let Err(error) = result {
            if self.selection().is_ok_and(|s| s.as_ref() == Some(expected)) {
                return Err(error.context(
                    "receiver update failed before selection changed; previous selection preserved",
                ));
            }
            if let Err(rollback) = self.write(&path, &toml::to_string(expected)?, true) {
                anyhow::bail!("{error:#}; selection rollback failed: {rollback:#}; partially saved selection; playback remains blocked until setup repairs the mismatch");
            }
            return Err(error.context("receiver update failed; previous selection restored; playback settings may be partially persisted; run piair setup to review consistency before starting"));
        }
        Ok(())
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

#[cfg(test)]
mod identity_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn full_width_identity_creation_and_restart() {
        for client in [1, i64::MAX as u64, (i64::MAX as u64) + 1, u64::MAX] {
            let dir = tempfile::tempdir().unwrap();
            let config = dir.path().join("config.toml");
            let store = Store::new(&config).unwrap();
            let expected = (client, u32::MAX);
            assert_eq!(store.client_identity_with(|| expected).unwrap(), expected);
            let path = store.root.join("client-identity.toml");
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.contains(&format!("client = \"{client:016X}\"")));
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
            assert_eq!(fs::metadata(&store.root).unwrap().mode() & 0o777, 0o700);
            drop(store);
            assert_eq!(
                Store::new(&config)
                    .unwrap()
                    .client_identity_with(|| panic!("must reuse persisted identity"))
                    .unwrap(),
                expected
            );
            assert_eq!(fs::read_to_string(path).unwrap(), text);
        }
    }

    #[test]
    fn legacy_numeric_identity_is_reused_without_rewriting() {
        for (client, expected) in [
            ("1", 1),
            ("9223372036854775807", i64::MAX as u64),
            ("0x1234_ABCD", 0x1234_ABCD),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(&dir.path().join("config.toml")).unwrap();
            let path = store.root.join("client-identity.toml");
            let text = format!("client = {client}\nremote = 4294967295\n");
            store.write(&path, &text, false).unwrap();
            assert_eq!(
                store
                    .client_identity_with(|| panic!("must reuse legacy identity"))
                    .unwrap(),
                (expected, u32::MAX)
            );
            assert_eq!(fs::read_to_string(path).unwrap(), text);
        }
    }

    #[test]
    fn invalid_identity_is_never_replaced() {
        for text in [
            "client = -1\nremote = 1",
            "client = 9223372036854775808\nremote = 1",
            "client = 18446744073709551615\nremote = 1",
            "client = 0\nremote = 1",
            "client = \"0000000000000000\"\nremote = 1",
            "client = \"FFFFFFFFFFFFFFFF\"\nremote = 0",
            "client = \"FFFFFFFFFFFFFFFF\"\nremote = -1",
            "client = \"FFFFFFFFFFFFFFFF\"\nremote = 4294967296",
            "client = \"ffffffffffffffff\"\nremote = 1",
            "client = \"FFFFFFFFFFFFFFF\"\nremote = 1",
            "client = \"0FFFFFFFFFFFFFFFF\"\nremote = 1",
            "client = \"0xFFFFFFFFFFFFFFFF\"\nremote = 1",
            "client = \"FFFFFFFFFFFFFFFG\"\nremote = 1",
            "client = \" FFFFFFFFFFFFFFF\"\nremote = 1",
            "client = \"+FFFFFFFFFFFFFFF\"\nremote = 1",
            "client = \"FFFF_FFFFFFFFFFF\"\nremote = 1",
            "client = 1.0\nremote = 1",
            "client = true\nremote = 1",
            "client = []\nremote = 1",
            "remote = 1",
            "client = 1",
            "client = 1\nclient = 2\nremote = 1",
            "client = 1\nremote = 1\nunknown = 1",
            "not TOML",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(&dir.path().join("config.toml")).unwrap();
            let path = store.root.join("client-identity.toml");
            store.write(&path, text, false).unwrap();
            assert!(
                store
                    .client_identity_with(|| panic!("must not regenerate corrupt identity"))
                    .is_err(),
                "{text}"
            );
            assert_eq!(fs::read_to_string(path).unwrap(), text);
        }
    }

    #[test]
    fn identity_keeps_protected_reads_and_no_clobber_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("config.toml")).unwrap();
        let path = store.root.join("client-identity.toml");
        let original = "client = 123\nremote = 456\n";
        store.write(&path, original, false).unwrap();
        assert!(store.write(&path, "replacement", false).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.client_identity().is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_file(&path).unwrap();
        let target = dir.path().join("target");
        fs::write(&target, original).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(store.client_identity().is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), original);
    }
}
