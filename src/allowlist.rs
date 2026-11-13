//! Local-only configuration editing. No discovery or receiver dependencies.
use crate::{
    cli::AllowlistCommand,
    config::{Config, Loaded},
};
use anyhow::{bail, ensure, Context, Result};
use std::{
    fs::{self, File},
    io::{self, BufRead, IsTerminal, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use toml_edit::{DocumentMut, Item, Value};

#[derive(Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) text: String,
    // Detect replacement, chmod and in-place writes, even with identical bytes.
    stamp: Stamp,
}

#[derive(Debug, PartialEq)]
struct Stamp {
    device: u64,
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<&fs::Metadata> for Stamp {
    fn from(m: &fs::Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}

pub(crate) fn snapshot(path: &Path) -> Result<Option<Snapshot>> {
    safe_parent(path, false)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    ensure!(
        metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == rustix::process::geteuid().as_raw(),
        "configuration must be an owned regular file with one link, not a symlink"
    );
    let stamp = Stamp::from(&metadata);
    let mut file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .context("cannot read configuration")?;
    ensure!(
        Stamp::from(&file.metadata()?) == stamp,
        "configuration changed while reading; retry"
    );
    let mut text = String::new();
    file.read_to_string(&mut text)
        .context("cannot read configuration")?;
    ensure!(
        Stamp::from(&file.metadata()?) == stamp
            && Stamp::from(&fs::symlink_metadata(path)?) == stamp,
        "configuration changed while reading; retry"
    );
    Config::parse(&text)?;
    Ok(Some(Snapshot { text, stamp }))
}

pub(crate) fn render(original: &str, id: &str, add: bool) -> Result<String> {
    Config::parse(original)?;
    let mut doc: DocumentMut = original.parse().context("cannot edit TOML")?;
    if !doc.contains_key("receiver_allowlist") {
        doc["receiver_allowlist"] = Item::Value(Value::Array(Default::default()));
    }
    let array = doc["receiver_allowlist"]
        .as_array_mut()
        .context("receiver_allowlist must be an array")?;
    let index = array.iter().position(|v| v.as_str() == Some(id));
    if add {
        array.push(id);
    } else if let Some(index) = index {
        // Retain comments/whitespace attached to the removed value.
        let decor = array.get(index).expect("index exists").decor().clone();
        array.remove(index);
        if let Some(next) = array.get_mut(index) {
            let prefix = format!(
                "{}{}{}",
                decor.prefix().and_then(|s| s.as_str()).unwrap_or(""),
                decor.suffix().and_then(|s| s.as_str()).unwrap_or(""),
                next.decor().prefix().and_then(|s| s.as_str()).unwrap_or("")
            );
            next.decor_mut().set_prefix(prefix);
        } else {
            let trailing = format!(
                "{}{}{}",
                decor.prefix().and_then(|s| s.as_str()).unwrap_or(""),
                decor.suffix().and_then(|s| s.as_str()).unwrap_or(""),
                array.trailing().as_str().unwrap_or("")
            );
            array.set_trailing(trailing);
        }
    }
    let text = doc.to_string();
    Config::parse(&text)?;
    Ok(text)
}

fn confirm(input: &mut impl BufRead, output: &mut impl Write, prompt: &str) -> Result<bool> {
    write!(output, "{prompt} [y/yes to confirm]: ")?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(
        answer.ends_with('\n')
            && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
    )
}

pub(crate) fn save(path: &Path, expected: Option<&Snapshot>, text: &str) -> Result<()> {
    Config::parse(text)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    safe_parent(path, true)?;
    // Lock the stable directory inode, not the configuration inode replaced by rename.
    // All configuration writers hold this lock through comparison and publication.
    let directory = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(parent)?;
    directory
        .try_lock()
        .context("another configuration writer is active; retry later")?;
    ensure!(
        snapshot(path)?.as_ref() == expected,
        "configuration changed; refusing to overwrite; rerun the command"
    );
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("cannot create temporary configuration")?;
    let mut permissions = None;
    if let Some(old) = expected {
        ensure!(old.stamp.mode & 0o222 != 0, "configuration is read-only");
        // Check effective write access too (e.g. filesystem ACLs), without truncating.
        let writable = File::options()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .context("configuration is not writable")?;
        let metadata = writable.metadata()?;
        // Do not silently change ownership when editing a file belonging to another user/group.
        let temp_metadata = temporary.as_file().metadata()?;
        ensure!(
            metadata.uid() == temp_metadata.uid() && metadata.gid() == temp_metadata.gid(),
            "cannot preserve configuration ownership"
        );
        permissions = Some(metadata.permissions());
    }
    temporary.write_all(text.as_bytes())?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    ensure!(
        snapshot(path)?.as_ref() == expected,
        "configuration changed; refusing to overwrite; rerun the command"
    );
    if expected.is_none() {
        temporary
            .persist_noclobber(path)
            .context("configuration appeared concurrently; nothing overwritten")?;
    } else {
        temporary
            .persist(path)
            .context("cannot atomically replace configuration")?;
    }
    directory
        .sync_all()
        .context("configuration saved, but directory sync failed")?;
    Ok(())
}

pub fn run(loaded: &Loaded, command: AllowlistCommand) -> Result<()> {
    let original = snapshot(&loaded.path)?;
    ensure!(
        original.is_some() || loaded.source != "file",
        "configuration disappeared; rerun the command"
    );
    let config = Config::parse(original.as_ref().map_or("", |s| s.text.as_str()))?;
    println!("Configuration: {}", loaded.path.display());
    let (id, add) = match command {
        AllowlistCommand::List => {
            if config.receiver_allowlist.is_empty() {
                println!("No receivers approved.");
            }
            for id in config.receiver_allowlist {
                println!("{id}");
            }
            return Ok(());
        }
        AllowlistCommand::Add { receiver_id } => (receiver_id, true),
        AllowlistCommand::Remove { receiver_id } => (receiver_id, false),
    };
    // Reuse the existing exact canonical ID and zero/broadcast validation.
    if id
        .strip_prefix("device:")
        .and_then(crate::config::device_id)
        .as_deref()
        != Some(&id)
    {
        bail!("receiver ID must be device:<12 lowercase hex digits>, excluding zero/broadcast IDs; names, addresses and aliases are not accepted");
    }
    if config.allows(&id) == add {
        println!(
            "{id} is {}; no changes.",
            if add { "already approved" } else { "absent" }
        );
        return Ok(());
    }
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "allowlist changes require an interactive terminal"
    );
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    if add && !confirm(&mut input, &mut output, &format!("Have you independently physically verified {id} using its device label or trusted local settings? mDNS advertisements alone are insufficient"))? {
        writeln!(output, "Cancelled; no changes.")?;
        return Ok(());
    }
    if !confirm(
        &mut input,
        &mut output,
        &format!(
            "{} {id} in {}?",
            if add {
                "Approve"
            } else {
                "Remove approval for"
            },
            loaded.path.display()
        ),
    )? {
        writeln!(output, "Cancelled; no changes.")?;
        return Ok(());
    }
    let text = render(original.as_ref().map_or("", |s| s.text.as_str()), &id, add)?;
    save(&loaded.path, original.as_ref(), &text)?;
    writeln!(
        output,
        "{} {id}.",
        if add {
            "Approved"
        } else {
            "Removed approval for"
        }
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    const ID: &str = "device:aabbccddeeff";

    #[test]
    fn editing_retains_other_values_entries_and_comments() {
        let source = "# header\nreceiver_allowlist = [\n # first\n 'device:112233445566', # second\n 'device:aabbccddeeff', # end\n] # approval\n[audio]\ndevice = 'default' # audio\n";
        let removed = render(source, ID, false).unwrap();
        for retained in [
            "# header",
            "# first",
            "# second",
            "# end",
            "# approval",
            "[audio]\ndevice = 'default' # audio",
            "'device:112233445566'",
        ] {
            assert!(removed.contains(retained), "{removed}");
        }
        assert!(!Config::parse(&removed).unwrap().allows(ID));
        let added = render(&removed, ID, true).unwrap();
        assert!(Config::parse(&added).unwrap().allows(ID));
        assert!(added.contains("'device:112233445566'"));
        assert!(render("receiver_allowlist=['alias']", ID, true).is_err());
        assert!(render("discovery_timeout_secs=0", ID, true).is_err());
    }

    #[test]
    fn confirmations_require_complete_affirmative_line() {
        for (answer, expected) in [
            ("y\n", true),
            ("yes\n", true),
            ("YES\n", true),
            ("n\n", false),
            ("\n", false),
            ("yes", false),
            ("", false),
            ("yeah\n", false),
        ] {
            assert_eq!(
                confirm(&mut answer.as_bytes(), &mut Vec::new(), "confirm").unwrap(),
                expected
            );
        }
    }

    #[test]
    fn atomic_save_preserves_permissions_and_rejects_stale_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# original\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let original = snapshot(&path).unwrap();
        let text = render("# original\n", ID, true).unwrap();
        save(&path, original.as_ref(), &text).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(save(&path, original.as_ref(), "").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        let current = snapshot(&path).unwrap();
        fs::write(&path, "# newer\n").unwrap();
        assert!(save(&path, current.as_ref(), "").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# newer\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn missing_readonly_symlink_invalid_and_locked_fail_safely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        save(&path, None, "# created\n").unwrap();
        assert!(save(&path, None, "").is_err());
        let original = snapshot(&path).unwrap();
        assert!(save(&path, original.as_ref(), "unknown=1").is_err());
        let lock = File::open(dir.path()).unwrap();
        lock.try_lock().unwrap();
        assert!(save(&path, original.as_ref(), "").is_err());
        drop(lock);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(save(&path, snapshot(&path).unwrap().as_ref(), "").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# created\n");
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert!(snapshot(&alias).is_err());
        let absent = dir.path().join("absent");
        assert!(save(&absent, None, "invalid=1").is_err());
        assert!(!absent.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn two_concurrent_writers_cannot_both_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = ["# one\n", "# two\n"]
            .into_iter()
            .map(|text| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let old = snapshot(&path).unwrap();
                    barrier.wait();
                    save(&path, old.as_ref(), text).is_ok()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|h| h.join().unwrap().then_some(()))
                .count(),
            1
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

// Reject symlink ancestors and directories writable by other users. Sticky /tmp
// is allowed for private temporary directories; the immediate parent must be owned.
fn safe_parent(path: &Path, create: bool) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .context("configuration needs a parent directory")?;
    let mut current = std::path::PathBuf::new();
    for component in parent.components() {
        ensure!(
            !matches!(component, std::path::Component::ParentDir),
            "configuration path must not contain .."
        );
        current.push(component.as_os_str());
        let meta = match fs::symlink_metadata(&current) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(());
                }
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new().mode(0o700).create(&current)?;
                fs::symlink_metadata(&current)?
            }
            Err(e) => return Err(e.into()),
        };
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "configuration parent must not be a symlink"
        );
        ensure!(
            meta.mode() & 0o022 == 0 || meta.mode() & 0o1000 != 0,
            "configuration ancestor is writable by others"
        );
        if current == parent {
            ensure!(
                meta.uid() == rustix::process::geteuid().as_raw() && meta.mode() & 0o022 == 0,
                "configuration directory must be owned and not writable by others"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod parent_security_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn refuses_symlink_ancestors_hardlinks_and_writable_parent() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert!(save(&alias.join("config.toml"), None, "").is_err());
        assert!(!real.join("config.toml").exists());
        let path = real.join("config.toml");
        fs::write(&path, "# original").unwrap();
        let link = real.join("link");
        fs::hard_link(&path, &link).unwrap();
        assert!(snapshot(&path).is_err());
        fs::remove_file(link).unwrap();
        let old = snapshot(&path).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(save(&path, old.as_ref(), "").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# original");
    }
}
