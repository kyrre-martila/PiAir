//! All receiver-directed socket operations live here, after authorization. No DNS,
//! redirects, retries or endpoint fallback. A Wire never exposes its socket.
use super::{compatibility::Method, exact_id, Selection};
use crate::{config, discovery::Receiver};
use anyhow::{bail, ensure, Context, Result};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{IpAddr, Shutdown, SocketAddr, TcpStream, UdpSocket},
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
    events: Option<super::crypto::ControlKeys>,
    last_headers: BTreeMap<String, String>,
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
            events: None,
            last_headers: BTreeMap::new(),
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
    pub(super) fn secure(
        mut self,
        keys: super::crypto::ControlKeys,
        events: super::crypto::ControlKeys,
    ) -> Result<Self> {
        self.reauthorize()?;
        ensure!(
            !self.failed && self.keys.is_none(),
            "invalid control upgrade"
        );
        self.keys = Some(keys);
        self.events = Some(events);
        Ok(self)
    }
    pub(super) fn close(&mut self) {
        self.failed = true;
        self.keys = None;
        self.events = None;
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
    pub(super) fn request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<Reply> {
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_some(),
                "authenticated control required"
            );
            self.reauthorize()?;
            self.restart_deadline();
            ensure!(
                [
                    "GET",
                    "POST",
                    "SETUP",
                    "RECORD",
                    "FLUSH",
                    "TEARDOWN",
                    "SET_PARAMETER"
                ]
                .contains(&method),
                "unsupported RTSP method"
            );
            ensure!(
                !uri.is_empty() && uri.len() < 256 && uri.bytes().all(|b| b > 32 && b < 127),
                "invalid RTSP URI"
            );
            ensure!(body.len() <= 8192, "RTSP body too large");
            self.seq = self.seq.checked_add(1).context("CSeq exhausted")?;
            let mut request=Zeroizing::new(format!("{method} {uri} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: PiAir/0.1\r\nX-Apple-ProtocolVersion: 1\r\nContent-Length: {}\r\n",self.seq,body.len()).into_bytes());
            for (key, value) in headers {
                ensure!(
                    key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && !key.is_empty()
                        && value.bytes().all(|b| (32..=126).contains(&b)),
                    "invalid request header"
                );
                request.extend_from_slice(format!("{key}: {value}\r\n").as_bytes());
            }
            request.extend_from_slice(b"\r\n");
            request.extend_from_slice(body);
            self.write_message(&request)?;
            let body = self.response()?;
            ensure!(
                self.pending_start == self.pending_end,
                "unsolicited control data"
            );
            self.reauthorize()?;
            Ok(Reply {
                body,
                headers: self.last_headers.clone(),
            })
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    pub(super) fn local_ip(&self) -> Result<IpAddr> {
        Ok(self.socket.local_addr()?.ip())
    }
    pub(super) fn lease(&self) -> Lease {
        Lease {
            authorization: self.authorization.clone(),
            id: self.receiver_id.clone(),
        }
    }
    pub(super) fn bind_media(&self) -> Result<Ports> {
        self.reauthorize()?;
        ensure!(self.keys.is_some(), "authentication required");
        let local = self.socket.local_addr()?.ip();
        ensure!(
            local.is_ipv4(),
            "realtime diagnostic currently requires IPv4"
        );
        Ok(Ports {
            data: UdpSocket::bind(SocketAddr::new(local, 0))?,
            control: UdpSocket::bind(SocketAddr::new(local, 0))?,
            timing: Some(UdpSocket::bind(SocketAddr::new(local, 0))?),
            timing_worker: None,
        })
    }
    pub(super) fn start_timing(
        &self,
        ports: &mut Ports,
        port: u16,
        clock: Arc<super::realtime::Clock>,
    ) -> Result<()> {
        self.reauthorize()?;
        ensure!(
            self.keys.is_some() && port > 0,
            "invalid authenticated timing port"
        );
        let socket = ports.timing.take().context("timing socket absent")?;
        socket.connect(SocketAddr::new(self.socket.peer_addr()?.ip(), port))?;
        ports.timing_worker = Some(Worker::timing(socket, self.lease(), clock)?);
        Ok(())
    }
    pub(super) fn media(
        &self,
        ports: Ports,
        data: u16,
        control: u16,
        timing: u16,
    ) -> Result<MediaIo> {
        self.reauthorize()?;
        ensure!(
            self.keys.is_some() && data > 0 && control > 0 && timing > 0,
            "invalid authenticated ports"
        );
        let peer = self.socket.peer_addr()?.ip();
        ports.data.connect(SocketAddr::new(peer, data))?;
        ports.control.connect(SocketAddr::new(peer, control))?;

        ports.control.set_nonblocking(true)?;
        let lease = self.lease();
        let timing = ports.timing_worker.context("NTP service not started")?;
        self.reauthorize()?;
        Ok(MediaIo {
            data: ports.data,
            control: ports.control,
            timing,
            lease,
        })
    }
    pub(super) fn event(&mut self, port: u16) -> Result<Worker> {
        self.reauthorize()?;
        ensure!(
            self.keys.is_some() && port > 0,
            "invalid authenticated event port"
        );
        let addr = SocketAddr::new(self.socket.peer_addr()?.ip(), port);
        let socket = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
        let mut event = Self::new(
            socket,
            Duration::from_secs(3),
            self.receiver_id.clone(),
            self.method,
        );
        event.authorization = self.authorization.clone();
        event.keys = Some(self.events.take().context("event keys already consumed")?);
        self.reauthorize()?;
        Worker::events(event)
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
            words[1].len() == 3 && words[1].bytes().all(|b| b.is_ascii_digit()),
            "invalid RTSP/HTTP status"
        );
        ensure!(
            words[1] == "200",
            "receiver rejected request with status {} (no FairPlay/MFi/password/redirect fallback)",
            words[1]
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
        self.last_headers = headers
            .iter()
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect();
        Ok(body)
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) struct Reply {
    pub body: Zeroizing<Vec<u8>>,
    pub headers: BTreeMap<String, String>,
}
#[derive(Clone)]
pub(super) struct Lease {
    authorization: Option<(PathBuf, Selection)>,
    id: String,
}
impl Lease {
    pub(super) fn check(&self) -> Result<()> {
        if let Some((path, selected)) = &self.authorization {
            ensure!(
                config::load(Some(path.clone()))?.config.allows(&self.id),
                "receiver approval revoked"
            );
            ensure!(
                super::store::Store::new(path)?.selection()?.as_ref() == Some(selected),
                "selection changed during session"
            );
        }
        Ok(())
    }
}
pub(super) struct Ports {
    data: UdpSocket,
    control: UdpSocket,
    timing: Option<UdpSocket>,
    timing_worker: Option<Worker>,
}
impl Ports {
    pub fn control(&self) -> Result<u16> {
        Ok(self.control.local_addr()?.port())
    }
    pub fn timing(&self) -> Result<u16> {
        Ok(self
            .timing
            .as_ref()
            .context("timing already started")?
            .local_addr()?
            .port())
    }
}
pub(super) struct MediaIo {
    data: UdpSocket,
    control: UdpSocket,
    pub timing: Worker,
    lease: Lease,
}
impl MediaIo {
    pub fn send(&self, packet: &[u8]) -> Result<()> {
        self.lease.check()?;
        ensure!(packet.len() <= 2048, "media datagram too large");
        ensure!(
            self.data.send(packet)? == packet.len(),
            "partial media datagram"
        );
        Ok(())
    }
    pub fn sync(&self, packet: &[u8; 20]) -> Result<()> {
        self.lease.check()?;
        ensure!(self.control.send(packet)? == 20, "sync send failed");
        Ok(())
    }
    pub fn retransmit(&self) -> Result<Option<(u16, u16)>> {
        let mut b = [0; 9];
        match self.control.recv_from(&mut b) {
            Ok((n, peer)) => {
                if peer != self.control.peer_addr()? {
                    return Ok(None);
                }
                self.lease.check()?;
                ensure!(
                    n == 8 && b[..2] == [0x80, 0xd5],
                    "invalid retransmission feedback"
                );
                let count = u16::from_be_bytes([b[6], b[7]]);
                ensure!((1..=32).contains(&count), "retransmission request limit");
                Ok(Some((u16::from_be_bytes([b[4], b[5]]), count)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
struct Health {
    stop: AtomicBool,
    failure: Mutex<Option<String>>,
    count: AtomicU64,
    last: Mutex<Instant>,
}
pub(super) struct Worker {
    health: Arc<Health>,
    join: Option<JoinHandle<()>>,
    closer: Option<TcpStream>,
}
impl Worker {
    fn spawn(
        closer: Option<TcpStream>,
        work: impl FnOnce(&Health) -> Result<()> + Send + 'static,
    ) -> Self {
        let health = Arc::new(Health {
            stop: AtomicBool::new(false),
            failure: Mutex::new(None),
            count: AtomicU64::new(0),
            last: Mutex::new(Instant::now()),
        });
        let state = health.clone();
        let join = thread::spawn(move || {
            if let Err(error) = work(&state) {
                if !state.stop.load(Ordering::Relaxed) {
                    *state.failure.lock().unwrap() = Some(format!("{error:#}"));
                }
            }
        });
        Self {
            health,
            join: Some(join),
            closer,
        }
    }
    pub fn check(&self) -> Result<()> {
        let failure = self.health.failure.lock().unwrap();
        ensure!(
            failure.is_none(),
            "receiver worker failed: {}",
            failure.as_deref().unwrap_or("unknown")
        );
        ensure!(
            !self.join.as_ref().is_some_and(|j| j.is_finished()),
            "receiver worker terminated unexpectedly"
        );
        Ok(())
    }
    pub fn count(&self) -> u64 {
        self.health.count.load(Ordering::Relaxed)
    }
    pub fn age(&self) -> Duration {
        self.health.last.lock().unwrap().elapsed()
    }
    fn timing(socket: UdpSocket, lease: Lease, clock: Arc<super::realtime::Clock>) -> Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self::spawn(None, move |health| {
            let mut b = [0; 33];
            while !health.stop.load(Ordering::Relaxed) {
                match socket.recv_from(&mut b) {
                    Ok((n, peer)) => {
                        if peer != socket.peer_addr()? {
                            continue;
                        }
                        let recv = clock.now();
                        let out = super::realtime::timing_reply(&b[..n], recv, clock.now())?;
                        lease.check()?;
                        ensure!(socket.send(&out)? == out.len(), "timing reply send failed");
                        health.count.fetch_add(1, Ordering::Relaxed);
                        *health.last.lock().unwrap() = Instant::now();
                    }
                    Err(e)
                        if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                            .contains(&e.kind()) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        }))
    }
    fn events(mut wire: Wire) -> Result<Self> {
        let closer = Some(wire.socket.try_clone()?);
        Ok(Self::spawn(closer, move |health| {
            wire.socket
                .set_read_timeout(Some(Duration::from_millis(100)))?;
            while !health.stop.load(Ordering::Relaxed) {
                if wire.pending_start == wire.pending_end {
                    let mut first = [0];
                    match wire.socket.peek(&mut first) {
                        Ok(0) => bail!("receiver closed event channel"),
                        Ok(_) => (),
                        Err(e)
                            if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                                .contains(&e.kind()) =>
                        {
                            continue
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
                wire.restart_deadline();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    ensure!(header.len() < 8192, "event header too large");
                    let mut b = [0];
                    wire.read_exact(&mut b)?;
                    header.push(b[0]);
                }
                let text = std::str::from_utf8(&header)?;
                let mut lines = text[..text.len() - 4].split("\r\n");
                ensure!(
                    lines.next() == Some("POST /command RTSP/1.0"),
                    "unsupported receiver event/termination"
                );
                let mut headers = BTreeMap::new();
                for line in lines {
                    let (k, v) = line.split_once(':').context("malformed event header")?;
                    ensure!(
                        headers.insert(k.to_ascii_lowercase(), v.trim()).is_none(),
                        "duplicate event header"
                    );
                }
                ensure!(
                    !headers.contains_key("transfer-encoding") && !headers.contains_key("location"),
                    "unsupported event framing"
                );
                let cseq = headers.get("cseq").context("event CSeq absent")?;
                ensure!(
                    !cseq.is_empty()
                        && cseq.bytes().all(|b| b.is_ascii_digit())
                        && cseq.parse::<u32>().is_ok(),
                    "invalid event CSeq"
                );
                let length = headers
                    .get("content-length")
                    .context("event length absent")?;
                ensure!(
                    length.bytes().all(|b| b.is_ascii_digit()),
                    "invalid event size"
                );
                let n = length.parse::<usize>()?;
                ensure!(n <= 8192, "event body too large");
                ensure!(
                    headers.get("content-type") == Some(&"application/x-apple-binary-plist"),
                    "unsupported event content type"
                );
                let mut body = Zeroizing::new(vec![0; n]);
                wire.read_exact(&mut body)?;
                let value = super::profile::parse(&body)?;
                let d = super::profile::dictionary(&value)?;
                ensure!(
                    d.get("type").and_then(plist::Value::as_string) == Some("updateInfo"),
                    "unsupported receiver event type"
                );
                ensure!(
                    d.get("value").is_some_and(|v| v.as_dictionary().is_some()),
                    "invalid updateInfo value"
                );
                wire.reauthorize()?;
                wire.write_message(format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nContent-Length: 0\r\nAudio-Latency: 0\r\n\r\n").as_bytes())?;
                health.count.fetch_add(1, Ordering::Relaxed);
                *health.last.lock().unwrap() = Instant::now();
                wire.socket
                    .set_read_timeout(Some(Duration::from_millis(100)))?;
            }
            Ok(())
        }))
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.health.stop.store(true, Ordering::Relaxed);
        if let Some(socket) = &self.closer {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
