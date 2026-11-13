//! Linux kernel-held, per-user exclusion across automatic commands/config paths.
//! No filesystem object, PID, unlock request, or stale-lock cleanup is involved.
use anyhow::Result;
use std::{
    io::ErrorKind,
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixListener},
    },
};

pub(in crate::receiver) struct InstanceLock {
    // Keep the exclusive abstract address bound until all runtime resources join.
    // std creates close-on-exec descriptors; subprocesses cannot retain the lock.
    _socket: UnixListener,
}
impl InstanceLock {
    pub(in crate::receiver) fn acquire() -> Result<Self> {
        let name = format!("piair.automatic.{}", rustix::process::geteuid().as_raw());
        let address = SocketAddr::from_abstract_name(name)?;
        let socket = UnixListener::bind_addr(&address).map_err(|error| {
            if error.kind() == ErrorKind::AddrInUse {
                anyhow::anyhow!("another automatic playback instance is already running for this user; stop that piair run or automatic-ptp-capture process before retrying")
            } else {
                anyhow::Error::new(error).context("cannot acquire automatic playback process lock")
            }
        })?;
        Ok(Self { _socket: socket })
    }
}
