//! The ONLY production TCP connect is here, after live authorization. No DNS,
//! redirects, retries or endpoint fallback. A Wire never exposes its socket.
use super::{exact_id, Selection};
use crate::{config, discovery::Receiver};
use anyhow::{bail, ensure, Context, Result};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub struct Wire {
    socket: TcpStream,
    deadline: Instant,
    seq: u32,
    failed: bool,
    receiver_id: String,
}
impl Wire {
    pub fn connect(
        config_path: &Path,
        id: &str,
        selected: &Selection,
        snapshot: &[Receiver],
    ) -> Result<Self> {
        exact_id(id)?;
        // No hostname resolution exists: only pinned, literal discovery addresses.
        let current = config::load(Some(config_path.to_owned()))?;
        ensure!(
            current.config.allows(id),
            "receiver is not currently approved"
        );
        ensure!(selected.id == id, "receiver is not selected");
        let receiver = snapshot
            .iter()
            .find(|r| r.id == id)
            .context("receiver not discovered")?;
        let observed = Selection::from_receiver(receiver)?;
        ensure!(
            &observed == selected,
            "receiver identity/capability/endpoint changed; review and select again"
        );
        for r in snapshot.iter().filter(|r| r.id != id) {
            ensure!(
                !r.services
                    .iter()
                    .any(|s| s.addresses.iter().any(|ip| selected.addresses.contains(ip))),
                "conflicting/ambiguous receiver identities share an address"
            );
        }
        // Recheck persisted selection and approval immediately before the socket.
        let saved = super::store::Store::new(config_path)?
            .selection()?
            .context("receiver selection absent")?;
        ensure!(
            &saved == selected
                && config::load(Some(config_path.to_owned()))?
                    .config
                    .allows(id),
            "receiver approval/selection changed"
        );
        let ip = selected
            .addresses
            .iter()
            .find(|ip| ip.is_ipv4())
            .or_else(|| selected.addresses.iter().next())
            .context("receiver has no address")?;
        let socket = TcpStream::connect_timeout(
            &SocketAddr::new(*ip, selected.port),
            Duration::from_secs(3),
        )
        .context("approved receiver connection failed (no retry)")?;
        Ok(Self::new(socket, Duration::from_secs(15), id.to_owned()))
    }
    fn new(socket: TcpStream, budget: Duration, receiver_id: String) -> Self {
        Self {
            socket,
            deadline: Instant::now() + budget,
            seq: 0,
            failed: false,
            receiver_id,
        }
    }
    #[cfg(test)]
    pub(super) fn mock(socket: TcpStream, budget: Duration, id: &str) -> Self {
        Self::new(socket, budget, id.to_owned())
    }
    pub fn receiver_id(&self) -> &str {
        &self.receiver_id
    }
    pub fn restart_deadline(&mut self) {
        self.deadline = Instant::now() + Duration::from_secs(15);
    }
    fn timeout(&self) -> Result<()> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .context("authentication handshake timed out")?;
        ensure!(!remaining.is_zero(), "authentication handshake timed out");
        self.socket
            .set_read_timeout(Some(remaining.min(Duration::from_secs(3))))?;
        self.socket
            .set_write_timeout(Some(remaining.min(Duration::from_secs(3))))?;
        Ok(())
    }
    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            self.timeout()?;
            let n = self
                .socket
                .read(&mut bytes[offset..])
                .context("receiver read failed/timed out")?;
            ensure!(n > 0, "truncated receiver response");
            offset += n;
        }
        Ok(())
    }
    pub fn post(&mut self, path: &str, body: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(!self.failed, "receiver transport already failed");
        let result = self.post_inner(path, body);
        if result.is_err() {
            self.failed = true;
            let _ = self.socket.shutdown(Shutdown::Both);
        }
        result
    }
    fn post_inner(&mut self, path: &str, body: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(body.len() <= super::tlv::LIMIT, "request body too large");
        ensure!(
            ["/pair-pin-start", "/pair-setup", "/pair-verify"].contains(&path),
            "unsupported authentication endpoint"
        );
        self.seq = self
            .seq
            .checked_add(1)
            .context("request sequence exhausted")?;
        let header=format!("POST {path} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: PiAir/0.1\r\nContent-Type: application/octet-stream\r\nX-Apple-HKP: 3\r\nContent-Length: {}\r\n\r\n",self.seq,body.len());
        for bytes in [header.as_bytes(), body] {
            let mut offset = 0;
            while offset < bytes.len() {
                self.timeout()?;
                let n = self.socket.write(&bytes[offset..])?;
                ensure!(n > 0, "receiver write failed");
                offset += n;
            }
        }
        let mut raw = Vec::new();
        while !raw.ends_with(b"\r\n\r\n") {
            ensure!(raw.len() < 8192, "receiver headers too large");
            let mut b = [0];
            self.read_exact(&mut b)?;
            raw.push(b[0]);
        }
        let text = std::str::from_utf8(&raw).context("invalid response headers")?;
        let mut lines = text[..text.len() - 4].split("\r\n");
        let status = lines.next().context("missing response status")?;
        let words: Vec<_> = status.splitn(3, ' ').collect();
        ensure!(
            words.len() >= 2 && ["RTSP/1.0", "HTTP/1.1"].contains(&words[0]),
            "invalid receiver response protocol"
        );
        ensure!(
            words[1] == "200",
            "receiver rejected authentication request (no redirects/retry/fallback)"
        );
        let mut headers = BTreeMap::new();
        for line in lines {
            let (key, value) = line.split_once(':').context("malformed receiver header")?;
            ensure!(
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "invalid receiver header name"
            );
            ensure!(
                headers
                    .insert(key.to_ascii_lowercase(), value.trim())
                    .is_none(),
                "duplicate receiver header"
            );
        }
        ensure!(
            !headers.contains_key("transfer-encoding") && !headers.contains_key("location"),
            "receiver transfer encoding/redirect unsupported"
        );
        if let Some(seq) = headers.get("cseq") {
            ensure!(
                !seq.is_empty() && seq.bytes().all(|b| b.is_ascii_digit()),
                "invalid receiver CSeq"
            );
            ensure!(
                seq.parse::<u32>().ok() == Some(self.seq),
                "unexpected receiver CSeq"
            );
        } else if words[0] == "RTSP/1.0" {
            bail!("receiver CSeq absent");
        }
        let raw_length = headers
            .get("content-length")
            .context("receiver Content-Length absent")?;
        ensure!(
            !raw_length.is_empty() && raw_length.bytes().all(|b| b.is_ascii_digit()),
            "invalid receiver Content-Length"
        );
        let length = raw_length
            .parse::<usize>()
            .context("invalid response size")?;
        ensure!(length <= super::tlv::LIMIT, "receiver body too large");
        let mut body = Zeroizing::new(vec![0; length]);
        self.read_exact(&mut body)?;
        Ok(body)
    }
}
