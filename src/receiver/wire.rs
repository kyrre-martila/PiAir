//! The ONLY production TCP connect is here, after live authorization. No DNS,
//! redirects, retries or endpoint fallback. A Wire never exposes its socket.
use super::{compatibility::Method, exact_id, Selection};
use crate::{config, discovery::Receiver};
use anyhow::{bail, ensure, Context, Result};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub struct Wire {
    socket: TcpStream,
    deadline: Instant,
    budget: Duration,
    seq: u32,
    failed: bool,
    receiver_id: String,
    method: Method,
    authorization: Option<(PathBuf, Selection)>,
    keys: Option<super::crypto::ControlKeys>,
    pending: Zeroizing<[u8; 1024]>,
    pending_start: usize,
    pending_end: usize,
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
        let observed = Selection::from_receiver(receiver, selected.method)?;
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
        let mut wire = Self::new(
            socket,
            Duration::from_secs(15),
            id.to_owned(),
            selected.method,
        );
        wire.authorization = Some((config_path.to_owned(), selected.clone()));
        Ok(wire)
    }
    fn new(socket: TcpStream, budget: Duration, receiver_id: String, method: Method) -> Self {
        Self {
            socket,
            deadline: Instant::now() + budget,
            budget,
            seq: 0,
            failed: false,
            receiver_id,
            method,
            authorization: None,
            keys: None,
            pending: Zeroizing::new([0; 1024]),
            pending_start: 0,
            pending_end: 0,
        }
    }
    #[cfg(test)]
    pub(super) fn mock(socket: TcpStream, budget: Duration, id: &str) -> Self {
        Self::new(socket, budget, id.to_owned(), Method::PinDisplay)
    }
    #[cfg(test)]
    pub(super) fn mock_method(
        socket: TcpStream,
        budget: Duration,
        id: &str,
        method: Method,
    ) -> Self {
        Self::new(socket, budget, id.to_owned(), method)
    }
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn receiver_id(&self) -> &str {
        &self.receiver_id
    }
    pub fn restart_deadline(&mut self) {
        self.deadline = Instant::now() + self.budget;
    }
    fn timeout(&self) -> Result<()> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .context("receiver operation timed out")?;
        ensure!(!remaining.is_zero(), "receiver operation timed out");
        self.socket
            .set_read_timeout(Some(remaining.min(Duration::from_secs(3))))?;
        self.socket
            .set_write_timeout(Some(remaining.min(Duration::from_secs(3))))?;
        Ok(())
    }
    fn read_raw(&mut self, bytes: &mut [u8]) -> Result<()> {
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
    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<()> {
        if self.keys.is_none() {
            return self.read_raw(bytes);
        }
        for byte in bytes {
            if self.pending_start == self.pending_end {
                let mut length = [0; 2];
                self.read_raw(&mut length)?;
                let size = usize::from(u16::from_le_bytes(length));
                ensure!((1..=1024).contains(&size), "invalid encrypted frame length");
                let mut frame = Zeroizing::new(vec![0; size + 18]);
                frame[..2].copy_from_slice(&length);
                self.read_raw(&mut frame[2..])?;
                let plain = self
                    .keys
                    .as_mut()
                    .context("control keys absent")?
                    .decrypt(&frame)?;
                self.pending[..plain.len()].copy_from_slice(&plain);
                self.pending_start = 0;
                self.pending_end = plain.len();
            }
            *byte = self.pending[self.pending_start];
            self.pending[self.pending_start].zeroize();
            self.pending_start += 1;
        }
        Ok(())
    }
    fn write_raw(&mut self, bytes: &[u8]) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            self.timeout()?;
            let n = self.socket.write(&bytes[offset..])?;
            ensure!(n > 0, "receiver write failed");
            offset += n;
        }
        Ok(())
    }
    fn write_message(&mut self, bytes: &[u8]) -> Result<()> {
        if self.keys.is_none() {
            return self.write_raw(bytes);
        }
        for chunk in bytes.chunks(1024) {
            let frame = self
                .keys
                .as_mut()
                .context("control keys absent")?
                .encrypt(chunk)?;
            self.write_raw(&frame)?;
        }
        Ok(())
    }
    pub(super) fn reauthorize(&self) -> Result<()> {
        if let Some((path, selected)) = &self.authorization {
            ensure!(
                config::load(Some(path.clone()))?
                    .config
                    .allows(&self.receiver_id),
                "receiver approval revoked"
            );
            ensure!(
                super::store::Store::new(path)?.selection()?.as_ref() == Some(selected),
                "receiver selection changed"
            );
        }
        Ok(())
    }
    pub(super) fn secure(mut self, keys: super::crypto::ControlKeys) -> Result<Self> {
        self.reauthorize()?;
        ensure!(
            !self.failed && self.keys.is_none(),
            "invalid control upgrade"
        );
        self.keys = Some(keys);
        Ok(self)
    }
    pub(super) fn close(&mut self) {
        self.failed = true;
        self.keys = None;
        self.pending.zeroize();
        self.pending_start = 0;
        self.pending_end = 0;
        let _ = self.socket.shutdown(Shutdown::Both);
    }
    // A deliberately narrow authenticated diagnostic; no arbitrary request API.
    pub(super) fn options(&mut self) -> Result<()> {
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_some(),
                "authenticated control session required"
            );
            self.reauthorize()?;
            self.restart_deadline();
            self.seq = self
                .seq
                .checked_add(1)
                .context("request sequence exhausted")?;
            let request = format!("OPTIONS * RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: PiAir/0.1\r\nContent-Length: 0\r\n\r\n", self.seq);
            self.write_message(request.as_bytes())?;
            let body = self.response()?;
            ensure!(
                body.is_empty() && self.pending_start == self.pending_end,
                "unexpected OPTIONS payload or unsolicited message"
            );
            self.reauthorize()
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    pub fn post(&mut self, path: &str, body: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_none(),
                "plaintext authentication transport unavailable"
            );
            self.post_inner(path, body)
        })();
        if result.is_err() {
            self.close();
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
        let header=format!("POST {path} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\nCSeq: {}\r\nUser-Agent: PiAir/0.1\r\nContent-Type: application/octet-stream\r\nX-Apple-HKP: 3\r\nContent-Length: {}\r\n\r\n",self.socket.peer_addr()?,self.seq,body.len());
        self.write_message(header.as_bytes())?;
        self.write_message(body)?;
        self.response()
    }
    fn response(&mut self) -> Result<Zeroizing<Vec<u8>>> {
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
            self.keys.is_none() || words[0] == "RTSP/1.0",
            "encrypted control response must use RTSP/1.0"
        );
        ensure!(
            words[1] == "200",
            "receiver rejected control/authentication request (no redirects/retry/fallback)"
        );
        let mut headers = BTreeMap::new();
        for line in lines {
            let (key, value) = line.split_once(':').context("malformed receiver header")?;
            ensure!(
                !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "invalid receiver header name"
            );
            ensure!(
                value.bytes().all(|b| b == b'\t' || (32..=126).contains(&b)),
                "invalid receiver header value"
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
        } else if words[0] == "RTSP/1.0" || self.keys.is_some() {
            bail!("receiver CSeq absent");
        }
        ensure!(
            !headers.get("connection").is_some_and(|v| v
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("close"))),
            "receiver terminated connection"
        );
        // RTSP permits an absent Content-Length for an empty response.
        let raw_length = match headers.get("content-length") {
            Some(value) => *value,
            None if self.keys.is_some() && words[0] == "RTSP/1.0" => "0",
            None => bail!("receiver Content-Length absent"),
        };
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

impl Drop for Wire {
    fn drop(&mut self) {
        self.close();
    }
}
