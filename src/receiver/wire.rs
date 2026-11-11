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

const OWNTONE_USER_AGENT: &str = "AirPlay/999.0.0";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthProfile {
    Legacy,
    PyatvDiagnostic,
    OwnToneTransient,
}

// Typed transport outcomes keep cleanup reporting independent of error wording.
#[derive(Debug)]
pub(super) enum ResponseFailure {
    Timeout,
    Closed,
    Rejected(u16),
}
impl ResponseFailure {
    pub(super) fn outcome(&self) -> &'static str {
        match self {
            Self::Timeout => "timed_out",
            Self::Closed => "connection_closed",
            Self::Rejected(_) => "rejected",
        }
    }
}
impl std::fmt::Display for ResponseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("receiver read timeout"),
            Self::Closed => f.write_str("truncated receiver response"),
            Self::Rejected(status) => write!(f, "receiver rejected request with status {status} (no FairPlay/MFi/password/redirect fallback)"),
        }
    }
}
impl std::error::Error for ResponseFailure {}

pub struct Wire {
    socket: TcpStream,
    // Scoped by Session to the initial SETUP diagnostic only.
    pub(super) setup_response_wait: Option<(Duration, Arc<AtomicBool>)>,
    polling_response: bool,
    pub(super) feedback_budget: Option<Duration>,
    pub(super) diagnostic_cancel: Option<Arc<AtomicBool>>,
    deadline: Instant,
    budget: Duration,
    seq: u32,
    response_bytes: usize,
    response_frames: usize,
    auth_profile: AuthProfile,
    auth_started: Instant,
    auth_phase: &'static str,
    client_instance: u64,
    active_remote: u32,
    failed: bool,
    receiver_id: String,
    method: Method,
    authorization: Option<(PathBuf, Authorization)>,
    keys: Option<super::crypto::ControlKeys>,
    events: Option<super::crypto::ControlKeys>,
    audio_key: Option<Zeroizing<[u8; 32]>>,
    last_headers: BTreeMap<String, String>,
    pending: Zeroizing<[u8; 1024]>,
    pending_start: usize,
    pending_end: usize,
}
/// Persisted operations retain exact global-selection checks. A HAP diagnostic operation
/// instead owns an immutable endpoint/capability pin and credential fingerprint.
#[derive(Clone)]
enum Authorization {
    Persisted(Selection),
    HapOperation {
        selected: Selection,
        credential: Zeroizing<[u8; 64]>,
    },
}
impl Authorization {
    fn check(&self, path: &Path, id: &str) -> Result<()> {
        ensure!(config::approval(path, id)?, "receiver approval revoked");
        let store = super::store::Store::new(path)?;
        match self {
            Self::Persisted(selected) => ensure!(
                store.selection()?.as_ref() == Some(selected),
                "receiver selection changed"
            ),
            Self::HapOperation {
                selected,
                credential,
            } => {
                ensure!(selected.id == id, "operation receiver identity changed");
                let current = store
                    .credential_fingerprint(id)?
                    .context("operation credentials removed")?;
                use subtle::ConstantTimeEq;
                ensure!(
                    bool::from(current.as_ref().ct_eq(credential.as_ref())),
                    "operation credentials changed or revoked"
                );
            }
        }
        Ok(())
    }
}

impl Wire {
    pub fn connect(
        config_path: &Path,
        id: &str,
        selected: &Selection,
        snapshot: &[Receiver],
    ) -> Result<Self> {
        Self::connect_authorized(
            config_path,
            id,
            selected,
            snapshot,
            Authorization::Persisted(selected.clone()),
        )
    }
    /// Pin this operation to its selected receiver and stored credential fingerprint.
    pub(super) fn connect_hap_operation(
        config_path: &Path,
        selected: &Selection,
        snapshot: &[Receiver],
        credential: &super::store::Credential,
    ) -> Result<Self> {
        let id = selected.id.as_str();
        ensure!(
            selected.method == Method::StoredHap
                && selected.method == credential.authentication_method(id)?,
            "HAP operation requires exact identity and stored HAP authentication"
        );
        let snapshot_credential = super::store::Store::new(config_path)?
            .credential_snapshot(id)?
            .context("existing receiver HAP credentials required")?;
        use subtle::ConstantTimeEq;
        ensure!(
            credential.advertised_pi == selected.advertised_pi
                && bool::from(
                    snapshot_credential
                        .value
                        .fingerprint()?
                        .as_ref()
                        .ct_eq(credential.fingerprint()?.as_ref())
                ),
            "operation credential binding changed or differs from confirmation"
        );
        selected.capabilities.decide(selected.method)?;
        ensure!(
            selected.capabilities.features & ((1 << 9) | (1 << 19)) == ((1 << 9) | (1 << 19)),
            "receiver must advertise audio and ALAC"
        );
        let authorization = Authorization::HapOperation {
            selected: selected.clone(),
            credential: snapshot_credential.fingerprint,
        };
        Self::connect_authorized(config_path, id, selected, snapshot, authorization)
    }
    fn connect_authorized(
        config_path: &Path,
        id: &str,
        selected: &Selection,
        snapshot: &[Receiver],
        authorization: Authorization,
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
            selected.matches_observation(&observed),
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
        // Recheck the appropriate authorization immediately before the socket.
        authorization.check(config_path, id)?;
        let ip = selected
            .addresses
            .iter()
            .find(|ip| ip.is_ipv4())
            .or_else(|| selected.addresses.iter().next())
            .context("receiver has no address")?;
        tracing::debug!(target: "piair::auth", method = ?selected.method,
            flags = ?observed.capabilities.flags, features = observed.capabilities.features,
            acl = ?observed.capabilities.access_control, act = ?observed.capabilities.access_control_type,
            password_required = ?observed.capabilities.password,
            protocol = ?observed.capabilities.protocol_version, "authentication receiver policy");
        for service in &receiver.services {
            for key in ["model", "am", "osvers", "srcvers", "fv"] {
                if let Some(value) = service.capabilities.get(key) {
                    tracing::debug!(target: "piair::auth", field = key, value = %super::diagnostics::metadata(value),
                        "advertised receiver metadata");
                }
            }
        }
        super::diagnostics::endpoint(*ip, selected.port);
        let connect_started = Instant::now();
        let socket = TcpStream::connect_timeout(
            &SocketAddr::new(*ip, selected.port),
            Duration::from_secs(3),
        )
        .inspect_err(|error| {
            tracing::debug!(target: "piair::auth", phase = "connect", elapsed_ms = connect_started.elapsed().as_millis() as u64,
                outcome = ?error.kind(), "authentication connection failed");
        })
        .context("approved receiver connection failed (no retry)")?;
        let mut wire = Self::new(
            socket,
            Duration::from_secs(15),
            id.to_owned(),
            selected.method,
        );
        (wire.client_instance, wire.active_remote) =
            super::store::Store::new(config_path)?.client_identity()?;
        wire.authorization = Some((config_path.to_owned(), authorization));
        Ok(wire)
    }
    fn new(socket: TcpStream, budget: Duration, receiver_id: String, method: Method) -> Self {
        Self {
            socket,
            setup_response_wait: None,
            polling_response: false,
            feedback_budget: None,
            diagnostic_cancel: None,
            deadline: Instant::now() + budget,
            budget,
            seq: 0,
            response_bytes: 0,
            response_frames: 0,
            auth_profile: AuthProfile::Legacy,
            auth_started: Instant::now(),
            auth_phase: "connect",
            client_instance: rand::random(),
            active_remote: rand::random(),
            failed: false,
            receiver_id,
            method,
            authorization: None,
            keys: None,
            events: None,
            audio_key: None,
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
    pub(super) fn auth_phase(&mut self, phase: &'static str) {
        self.auth_phase = phase;
        tracing::debug!(target: "piair::auth", phase, method = ?self.method, "authentication phase");
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
            .ok_or(ResponseFailure::Timeout)
            .context("receiver operation timed out")?;
        if remaining.is_zero() {
            return Err(anyhow::Error::new(ResponseFailure::Timeout)
                .context("receiver operation timed out"));
        }
        self.socket
            .set_read_timeout(Some(remaining.min(if self.polling_response {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(3)
            })))?;
        self.socket
            .set_write_timeout(Some(remaining.min(Duration::from_secs(3))))?;
        Ok(())
    }
    pub(super) fn is_invalidated(&self) -> bool {
        self.failed
    }
    pub(super) fn limit_cleanup(&mut self) {
        self.budget = Duration::from_millis(250);
    }
    fn read_raw(&mut self, bytes: &mut [u8]) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            if let Some(cancel) = &self.diagnostic_cancel {
                self.reauthorize()?;
                ensure!(
                    self.auth_phase == "encrypted TEARDOWN" || !cancel.load(Ordering::Relaxed),
                    "diagnostic interrupted"
                );
            }
            if self.polling_response {
                if let Some((_, cancel)) = &self.setup_response_wait {
                    ensure!(!cancel.load(Ordering::Relaxed), "diagnostic interrupted");
                }
            }
            self.timeout().inspect_err(|_| {
                tracing::debug!(target: "piair::auth", phase = self.auth_phase, "connection deadline expired");
            })?;
            let read = self.socket.read(&mut bytes[offset..]);
            if self.polling_response
                && read.as_ref().is_err_and(|error| {
                    matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    )
                })
            {
                continue; // Keep offsets and the fixed overall deadline, including across frames.
            }
            let n = read.map_err(|error| {
                    tracing::debug!(target: "piair::auth", phase = self.auth_phase, outcome = ?error.kind(), "connection read failed");
                    match error.kind() {
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                        anyhow::Error::new(ResponseFailure::Timeout)
                    }
                    _ => anyhow::anyhow!("receiver read failed: kind={:?}", error.kind()),
                    }
                })?;
            if n == 0 {
                tracing::debug!(target: "piair::auth", phase = self.auth_phase, "peer closed connection");
            }
            if n == 0 {
                return Err(ResponseFailure::Closed.into());
            }
            offset += n;
            self.response_bytes += n;
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
                self.response_frames += 1;
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
            if let Some(cancel) = &self.diagnostic_cancel {
                self.reauthorize()?;
                ensure!(
                    self.auth_phase == "encrypted TEARDOWN" || !cancel.load(Ordering::Relaxed),
                    "diagnostic interrupted"
                );
            }
            self.timeout()?;
            let n = self.socket.write(&bytes[offset..]).inspect_err(|error| {
                tracing::debug!(target: "piair::auth", phase = self.auth_phase, outcome = ?error.kind(), "connection write failed");
            })?;
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
        if let Some((path, authorization)) = &self.authorization {
            authorization.check(path, &self.receiver_id)?;
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
    pub(super) fn install_audio_key(&mut self, shared: &[u8]) -> Result<()> {
        ensure!(
            self.keys.is_some() && shared.len() >= 32,
            "verified pairing required"
        );
        if self.uses_owntone_transient_profile() || self.method != Method::Transient {
            self.audio_key = Some(Zeroizing::new(shared[..32].try_into()?));
        }
        Ok(())
    }
    pub(super) fn take_audio_key(&mut self) -> Result<Zeroizing<[u8; 32]>> {
        self.reauthorize()?;
        self.audio_key.take().context("verified audio key absent")
    }
    pub(super) fn close(&mut self) {
        if !self.failed && self.keys.is_none() {
            tracing::debug!(target: "piair::auth", phase = self.auth_phase,
                elapsed_ms = self.auth_started.elapsed().as_millis() as u64, "authentication connection closed locally");
        }
        self.failed = true;
        self.keys = None;
        self.events = None;
        self.audio_key = None;
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
            self.auth_phase("encrypted OPTIONS");
            self.restart_deadline();
            let mut request = self.authenticated_header("OPTIONS", "*", 0)?;
            request.extend_from_slice(b"\r\n");
            self.write_message(&request)?;
            let body = self.response()?;
            ensure!(
                body.is_empty() && self.pending_start == self.pending_end,
                "unexpected OPTIONS payload or unsolicited message"
            );
            self.reauthorize()?;
            tracing::debug!(target: "piair::auth", phase = self.auth_phase, status = 200, cseq = self.seq,
                "encrypted control check validated");
            Ok(())
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    // Common authenticated RTSP envelope. Retain the connection identity chosen
    // before authentication (persisted for OwnTone) through the secure upgrade.
    fn authenticated_header(
        &mut self,
        method: &str,
        uri: &str,
        length: usize,
    ) -> Result<Zeroizing<Vec<u8>>> {
        self.seq = self.seq.checked_add(1).context("CSeq exhausted")?;
        let agent = if self.auth_profile == AuthProfile::OwnToneTransient {
            OWNTONE_USER_AGENT
        } else {
            "PiAir/0.1"
        };
        Ok(Zeroizing::new(format!(
            "{method} {uri} RTSP/1.0\r\nCSeq: {}\r\n\
             User-Agent: {agent}\r\nX-Apple-ProtocolVersion: 1\r\n\
             Content-Length: {length}\r\n\
             Client-Instance: {:X}\r\nDACP-ID: {:X}\r\nActive-Remote: {}\r\nX-Apple-Client-Name: PiAir\r\n",
            self.seq, self.client_instance, self.client_instance, self.active_remote
        ).into_bytes()))
    }
    pub(super) fn request(
        &mut self,
        method: &str,
        uri: &str,
        headers: &[(&str, String)],
        body: &[u8],
    ) -> Result<Reply> {
        let started = Instant::now();
        self.response_bytes = 0;
        self.response_frames = 0;
        self.auth_phase = match method {
            "GET" => "encrypted GET",
            "SETUP" => "encrypted SETUP",
            "RECORD" => "encrypted RECORD",
            "TEARDOWN" => "encrypted TEARDOWN",
            "FLUSH" => "encrypted FLUSH",
            "SET_PARAMETER" => "encrypted SET_PARAMETER",
            "SETPEERS" => "encrypted SETPEERS",
            "SETRATEANCHORTIME" => "encrypted SETRATEANCHORTIME",
            _ => "encrypted POST",
        };
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_some(),
                "authenticated control required"
            );
            self.reauthorize()?;
            self.restart_deadline();
            if let Some(budget) = self.feedback_budget {
                ensure!(
                    method == "POST" && uri == "/feedback",
                    "feedback budget scope"
                );
                self.deadline = started + self.budget.min(budget);
            }
            let request = (|| -> Result<Zeroizing<Vec<u8>>> {
                ensure!(
                    [
                        "GET",
                        "POST",
                        "SETUP",
                        "RECORD",
                        "FLUSH",
                        "TEARDOWN",
                        "SET_PARAMETER",
                        "SETPEERS",
                        "SETRATEANCHORTIME"
                    ]
                    .contains(&method),
                    "unsupported RTSP method"
                );
                ensure!(
                    !uri.is_empty() && uri.len() < 256 && uri.bytes().all(|b| b > 32 && b < 127),
                    "invalid RTSP URI"
                );
                ensure!(body.len() <= 8192, "RTSP body too large");
                let mut request = self.authenticated_header(method, uri, body.len())?;
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
                Ok(request)
            })()
            .context("request construction")?;
            tracing::debug!(target: "piair::rtsp", phase = self.auth_phase, method,
                cseq = self.seq, body_length = body.len(),
                headers = "CSeq,User-Agent,X-Apple-ProtocolVersion,Content-Length,Client-Instance,DACP-ID,Active-Remote,X-Apple-Client-Name",
                extra_header_names = ?headers.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
                operation_budget_ms = self.feedback_budget.map_or(self.budget, |budget| self.budget.min(budget)).as_millis() as u64,
                socket_wait_cap_ms = if self.setup_response_wait.is_some() || self.feedback_budget.is_some() { 100u64 } else { 3000u64 },
                setup_response_budget_ms = self.setup_response_wait.as_ref().map(|(budget, _)| budget.as_millis() as u64),
                uri_kind = if uri.starts_with("rtsp://") { "session" } else { "endpoint" },
                "authenticated request start");
            self.write_message(&request)
                .context("encrypted request write")?;
            if let Some((budget, _)) = &self.setup_response_wait {
                ensure!(
                    method == "SETUP",
                    "SETUP response wait used for another method"
                );
                self.deadline = Instant::now() + *budget;
                self.polling_response = true;
            }
            if self.diagnostic_cancel.is_some()
                && self.setup_response_wait.is_none()
                && self.feedback_budget.is_none()
            {
                self.deadline = Instant::now() + self.budget.min(Duration::from_secs(3));
                self.polling_response = true;
            }
            if self.feedback_budget.is_some() {
                // Keep the original total feedback deadline across partial frames,
                // checking cancellation/authorization between bounded read waits.
                self.polling_response = true;
            }
            let response = self.response().context("response read/validation");
            self.polling_response = false;
            let body = response?;
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
        tracing::debug!(target: "piair::rtsp", phase = self.auth_phase, method,
            cseq = self.seq, elapsed_ms = started.elapsed().as_millis() as u64,
            response_bytes = self.response_bytes, authenticated_frames = self.response_frames,
            success = result.is_ok(), "authenticated request completed");
        let result = result.with_context(|| format!(
            "encrypted RTSP method={method} phase={} elapsed_ms={} response_bytes={} authenticated_frames={}",
            self.auth_phase, started.elapsed().as_millis(), self.response_bytes, self.response_frames));
        if result.is_err() {
            self.close();
        }
        result
    }
    /// Deliberately narrow plaintext diagnostic: no caller-controlled method/path/body.
    pub(super) fn preverify_info(&mut self) -> Result<Reply> {
        let started = Instant::now();
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_none() && self.seq == 0,
                "pre-verify info requires a fresh authorized connection"
            );
            self.reauthorize()?;
            self.restart_deadline();
            self.auth_phase = "pre-verify GET /info";
            let mut header = self.authenticated_header("GET", "/info", 0)?;
            header.extend_from_slice(b"\r\n");
            self.write_raw(&header)?;
            let body = self.response()?;
            self.reauthorize()?;
            Ok(Reply {
                body,
                headers: self.last_headers.clone(),
            })
        })();
        tracing::debug!(target: "piair::rtsp", phase = "pre-verify GET /info",
            cseq = self.seq, elapsed_ms = started.elapsed().as_millis() as u64,
            success = result.is_ok(), "pre-verify info completed");
        if result.is_err() {
            self.close();
        }
        result
    }
    pub(super) fn start_timing(
        &self,
        ports: &mut Ports,
        clock: Arc<super::realtime::Clock>,
    ) -> Result<()> {
        self.reauthorize()?;
        ensure!(
            self.keys.is_some(),
            "authenticated control required for timing"
        );
        ensure!(ports.timing_worker.is_none(), "NTP service already started");
        let socket = ports.timing.take().context("timing socket absent")?;
        ports.timing_worker = Some(Worker::timing_for_peer(
            socket,
            self.lease(),
            clock,
            self.socket.peer_addr()?.ip(),
        )?);
        Ok(())
    }
    pub(super) fn sender_device_id(&self) -> String {
        // OwnTone serializes its stable u64 sender ID as eight colon-separated
        // octets. Keep all 64 bits of PiAir's persisted identity, without folding
        // it into a MAC or borrowing the receiver's pairing identifier.
        self.client_instance
            .to_be_bytes()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
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
    pub(super) fn media(
        &self,
        pending: &mut Option<Ports>,
        data: u16,
        control: u16,
    ) -> Result<MediaIo> {
        self.reauthorize()?;
        ensure!(
            self.keys.is_some() && data > 0 && control > 0,
            "invalid authenticated ports"
        );
        let ports = pending.as_ref().context("media sockets absent")?;
        ports.check_timing()?;
        let peer = self.socket.peer_addr()?.ip();
        ports.data.connect(SocketAddr::new(peer, data))?;
        ports.control.connect(SocketAddr::new(peer, control))?;

        ports.control.set_nonblocking(true)?;
        let lease = self.lease();
        self.reauthorize()?;
        let ports = pending.take().context("media sockets absent")?;
        let timing = ports.timing_worker.context("NTP service not started")?;
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
        if self.auth_profile == AuthProfile::OwnToneTransient {
            ensure!(
                path == "/pair-setup",
                "OwnTone transient endpoint forbidden"
            );
            return self.owntone_exchange("POST", path, body);
        }
        self.auth_post(
            path,
            body,
            self.auth_profile == AuthProfile::PyatvDiagnostic,
        )
    }
    /// Buffered diagnostics cannot use the legacy optional mock authorization.
    pub(super) fn buffered_authorization(&self) -> Result<()> {
        ensure!(
            !self.failed && self.authorization.is_some(),
            "explicit buffered receiver authorization required"
        );
        self.reauthorize()
    }
    /// Buffered transient diagnostics use the hardware-validated RTSP profile.
    /// Policy comes from the pinned authorization, never caller-supplied evidence.
    pub(super) fn buffered_transient(&mut self) -> Result<()> {
        self.buffered_authorization()?;
        let selected = match &self
            .authorization
            .as_ref()
            .context("authorization absent")?
            .1
        {
            Authorization::Persisted(selected) => selected.capabilities.clone(),
            _ => bail!("buffered transient requires persisted receiver selection"),
        };
        self.owntone_transient(&selected)
    }
    /// Issue a PTP capability only from a live authenticated, authorized Wire.
    /// The legacy Lease's optional mock authorization is never accepted here.
    pub(super) fn ptp_lease(&self) -> Result<PtpLease> {
        ensure!(
            !self.failed && self.keys.is_some(),
            "authenticated control required for PTP"
        );
        let (path, authorization) = self
            .authorization
            .as_ref()
            .context("PTP authorization absent")?;
        let lease = PtpLease {
            path: path.clone(),
            authorization: authorization.clone(),
            id: self.receiver_id.clone(),
            peer: self.socket.peer_addr()?.ip(),
        };
        lease.check()?;
        Ok(lease)
    }
    /// Explicit loopback fixture with real allowlist/selection revocation checks.
    #[cfg(test)]
    pub(super) fn ptp_test_authorization(&mut self) -> Result<tempfile::TempDir> {
        let peer = self.socket.peer_addr()?;
        ensure!(peer.ip().is_loopback(), "PTP fixture requires loopback");
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!("receiver_allowlist=['{}']", self.receiver_id),
        )?;
        let selected = Selection {
            id: self.receiver_id.clone(),
            advertised_pi: None,
            method: self.method,
            addresses: [peer.ip()].into_iter().collect(),
            port: peer.port(),
            capabilities: super::compatibility::Capabilities {
                features: (1 << 9) | (1 << 19) | (1 << 40) | (1 << 41) | (1 << 48),
                flags: Some(0),
                password: None,
                access_control: None,
                access_control_type: None,
                protocol_version: None,
            },
        };
        super::store::Store::new(&path)?.select(&selected)?;
        self.authorization = Some((path, Authorization::Persisted(selected)));
        self.reauthorize()?;
        Ok(dir)
    }
    /// Initial diagnostic only: attach after authenticated timingPeerInfo validation.
    /// Binding standard PTP ports may fail without local bind permission: fail closed.
    pub(super) fn start_ptp(
        &self,
        clock_id: u64,
        peer_clock_id: u64,
        selected: super::lifecycle::TimingPeer,
        timeout: Duration,
    ) -> Result<super::ptp::Service> {
        let mut lease = self.ptp_lease()?;
        let peer = selected.ip();
        // Delegate only the timing destination. The authorization retains the
        // original persisted receiver and control selection for revocation.
        lease.peer = peer;
        let local = self.socket.local_addr()?.ip();
        validate_timing_interface(local, peer).context("timing interface validation failed")?;
        let event = UdpSocket::bind(SocketAddr::new(local, 319))?;
        event.connect(SocketAddr::new(peer, 319))?;
        let general = UdpSocket::bind(SocketAddr::new(local, 320))?;
        general.connect(SocketAddr::new(peer, 320))?;
        self.reauthorize()?;
        super::ptp::Service::attach(
            super::ptp::Grandmaster::new(clock_id, peer_clock_id, Instant::now(), timeout)?,
            event,
            general,
            lease,
        )
    }
    /// Foundation only: exclusive same-receiver authenticated timing control.
    #[allow(dead_code)]
    pub(super) fn buffered_timing(
        &mut self,
        uri: String,
        clock: &super::ptp::Grandmaster,
    ) -> Result<super::ptp::control::Control<'_>> {
        self.reauthorize()?;
        ensure!(
            !self.failed && self.keys.is_some(),
            "authenticated control required"
        );
        ensure!(
            uri.starts_with("rtsp://") && uri.len() < 256 && uri.bytes().all(|b| b > 32 && b < 127),
            "invalid session URI"
        );
        super::ptp::control::Control::new(self, uri, clock)
    }
    pub(super) fn uses_owntone_transient_profile(&self) -> bool {
        self.auth_profile == AuthProfile::OwnToneTransient
    }
    pub(super) fn uses_pyatv_transient_profile(&self) -> bool {
        self.auth_profile == AuthProfile::PyatvDiagnostic
    }
    pub(super) fn pyatv_transient_preamble(&mut self) -> Result<()> {
        ensure!(
            self.method == Method::Transient,
            "explicit transient selection required; no fallback"
        );
        self.auth_phase("pyatv transient preamble");
        self.auth_post("/pair-pin-start", &[], true)?;
        self.auth_profile = AuthProfile::PyatvDiagnostic;
        Ok(())
    }
    /// Select once, fetch policy on this socket, then allow only transient setup.
    pub(super) fn owntone_transient(
        &mut self,
        selected: &super::compatibility::Capabilities,
    ) -> Result<()> {
        let result = (|| {
            ensure!(
                self.method == Method::Transient
                    && self.seq == 0
                    && self.auth_profile == AuthProfile::Legacy,
                "explicit fresh transient selection required; no fallback"
            );
            selected.decide(Method::Transient)?;
            if let Some((path, _)) = &self.authorization {
                (self.client_instance, self.active_remote) =
                    super::store::Store::new(path)?.client_identity()?;
            }
            self.auth_profile = AuthProfile::OwnToneTransient;
            self.auth_phase("OwnTone transient /info");
            let body = self.owntone_exchange("GET", "/info", &[])?;
            super::profile::transient_info(&body, selected)
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    fn owntone_exchange(
        &mut self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        let started = Instant::now();
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_none(),
                "plaintext authentication transport unavailable"
            );
            self.reauthorize()?;
            self.seq = self.seq.checked_add(1).context("CSeq exhausted")?;
            let header = owntone_header(
                method,
                path,
                self.seq,
                self.client_instance,
                self.active_remote,
                body.len(),
            )?;
            super::diagnostics::request(&header, body.len(), self.auth_phase);
            self.write_message(header.as_bytes())?;
            self.write_message(body)?;
            let response = self.response()?;
            self.reauthorize()?;
            Ok(response)
        })();
        tracing::debug!(target: "piair::auth", phase = self.auth_phase, method = ?self.method,
            elapsed_ms = started.elapsed().as_millis() as u64,
            total_elapsed_ms = self.auth_started.elapsed().as_millis() as u64,
            success = result.is_ok(), "authentication exchange completed");
        if result.is_err() {
            self.close();
        }
        result
    }
    fn auth_post(
        &mut self,
        path: &str,
        body: &[u8],
        pyatv_profile: bool,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let started = Instant::now();
        let result = (|| {
            ensure!(
                !self.failed && self.keys.is_none(),
                "plaintext authentication transport unavailable"
            );
            self.post_inner(path, body, pyatv_profile)
        })();
        tracing::debug!(target: "piair::auth", phase = self.auth_phase, method = ?self.method,
            elapsed_ms = started.elapsed().as_millis() as u64,
            total_elapsed_ms = self.auth_started.elapsed().as_millis() as u64,
            success = result.is_ok(), "authentication exchange completed");
        if result.is_err() {
            self.close();
        }
        result
    }
    fn post_inner(
        &mut self,
        path: &str,
        body: &[u8],
        pyatv_profile: bool,
    ) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(body.len() <= super::tlv::LIMIT, "request body too large");
        ensure!(
            ["/pair-pin-start", "/pair-setup", "/pair-verify"].contains(&path),
            "unsupported authentication endpoint"
        );
        if path != "/pair-pin-start" || !pyatv_profile {
            self.seq = self
                .seq
                .checked_add(1)
                .context("request sequence exhausted")?;
        }
        let header=format!("POST {path} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\nCSeq: {}\r\nUser-Agent: PiAir/0.1\r\nContent-Type: application/octet-stream\r\nX-Apple-HKP: {}\r\nContent-Length: {}\r\n\r\n",self.socket.peer_addr()?,self.seq,if self.method == Method::Transient { 4 } else { 3 },body.len());
        let header = if pyatv_profile {
            // Pinned pyatv _format_message adds length before supplied headers,
            // and does not synthesize Host or CSeq. Keep SRP/TLV bytes untouched.
            self.reauthorize()?;
            let length = if body.is_empty() {
                String::new()
            } else {
                format!("Content-Length: {}\r\n", body.len())
            };
            format!("POST {path} HTTP/1.1\r\n{length}user-agent: AirPlay/320.20\r\nconnection: keep-alive\r\nx-apple-hkp: 4\r\ncontent-type: application/octet-stream\r\n\r\n")
        } else {
            tracing::debug!(target: "piair::auth", host = %self.socket.peer_addr()?, "authentication numeric Host");
            header
        };
        super::diagnostics::request(&header, body.len(), self.auth_phase);
        self.write_message(header.as_bytes())?;
        self.write_message(body)?;
        self.response()
    }
    fn response(&mut self) -> Result<Zeroizing<Vec<u8>>> {
        let mut raw = Vec::new();
        while !raw.ends_with(b"\r\n\r\n") {
            ensure!(raw.len() < 8192, "receiver headers too large");
            let mut b = [0];
            self.read_exact(&mut b).context("response headers")?;
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
            (self.keys.is_none()
                && self.auth_profile != AuthProfile::OwnToneTransient
                && self.auth_phase != "pre-verify GET /info")
                || words[0] == "RTSP/1.0",
            "selected transport requires RTSP/1.0 response"
        );
        ensure!(
            words[1].len() == 3 && words[1].bytes().all(|b| b.is_ascii_digit()),
            "invalid RTSP/HTTP status"
        );
        if self.keys.is_none() {
            super::diagnostics::response(text, self.auth_phase);
        } else {
            // Log only the validated numeric status, never decrypted headers or payloads.
            tracing::debug!(target: "piair::auth", phase = self.auth_phase, status = %words[1],
                "encrypted control response status");
        }
        if words[1] != "200" {
            return Err(ResponseFailure::Rejected(words[1].parse()?).into());
        }
        (|| {
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
            self.read_exact(&mut body).context("response body")?;
            self.last_headers = headers
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect();
            Ok(body)
        })()
        .context("RTSP/HTTP status=200 malformed or incomplete response")
    }
}

#[cfg(test)]
#[test]
fn full_width_client_identifiers_keep_uppercase_wire_format() {
    for (client, expected) in [
        (1, "1"),
        (0xabcdef, "ABCDEF"),
        ((i64::MAX as u64) + 1, "8000000000000000"),
        (u64::MAX, "FFFFFFFFFFFFFFFF"),
    ] {
        let header = owntone_header("GET", "/info", 1, client, u32::MAX, 0).unwrap();
        assert!(header.contains(&format!("\r\nClient-Instance: {expected}\r\n")));
        assert!(header.contains(&format!("\r\nDACP-ID: {expected}\r\n")));
        assert!(header.contains("\r\nActive-Remote: 4294967295\r\n"));
    }
}

/// Protocol serialization is independent of the reference implementation.
fn owntone_header(
    method: &str,
    path: &str,
    seq: u32,
    client: u64,
    remote: u32,
    length: usize,
) -> Result<String> {
    ensure!(
        (method == "GET" && path == "/info" && length == 0)
            || (method == "POST" && path == "/pair-setup" && length <= super::tlv::LIMIT),
        "unsupported OwnTone authentication request"
    );
    // Pinned OwnTone conffile.c default: HomePod OS 27 validates this agent
    // before /info. Keep it scoped to this explicitly selected experiment.
    let mut header = format!("{method} {path} RTSP/1.0\r\nCSeq: {seq}\r\nUser-Agent: {OWNTONE_USER_AGENT}\r\nX-Apple-Client-Name: PiAir\r\nClient-Instance: {client:X}\r\nDACP-ID: {client:X}\r\nActive-Remote: {remote}\r\n");
    if method == "POST" {
        header.push_str(&format!("Content-Type: application/octet-stream\r\nX-Apple-HKP: 4\r\nContent-Length: {length}\r\n"));
    }
    header.push_str("\r\n");
    Ok(header)
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
/// Mandatory PTP network capability. Only Wire can construct it, after checking
/// authentication and a concrete persisted/operation authorization. No None form.
pub(super) struct PtpLease {
    path: PathBuf,
    authorization: Authorization,
    id: String,
    peer: IpAddr,
}
impl PtpLease {
    pub(super) fn check(&self) -> Result<()> {
        self.authorization.check(&self.path, &self.id)
    }
    pub(super) fn peer(&self) -> IpAddr {
        self.peer
    }
}
#[derive(Clone)]
pub(super) struct Lease {
    authorization: Option<(PathBuf, Authorization)>,
    id: String,
}
impl Lease {
    pub(super) fn check(&self) -> Result<()> {
        if let Some((path, authorization)) = &self.authorization {
            authorization.check(path, &self.id)?;
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
    pub fn timing_replies(&self) -> u64 {
        self.timing_worker.as_ref().map_or(0, Worker::count)
    }
    pub fn has_timing_worker(&self) -> bool {
        self.timing_worker.is_some()
    }
    pub fn check_timing(&self) -> Result<()> {
        self.timing_worker
            .as_ref()
            .context("NTP service absent")?
            .check()
    }
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
                control_diagnostic(&b[..n]);
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
// Only classify the bounded datagram header. Never emit receiver payload bytes.
fn control_diagnostic(packet: &[u8]) {
    let kind = if packet.starts_with(&[0x80, 0xd5]) {
        "retransmission"
    } else {
        "unrecognized"
    };
    tracing::debug!(target: "piair::rtsp", phase = "receiver control",
        kind, received_bytes_capped = packet.len(), length_at_cap = packet.len() == 9,
        "peer-bound control datagram observed; parser policy unchanged");
}

#[cfg(test)]
mod control_diagnostic_tests {
    use super::*;
    #[test]
    fn control_diagnostics_classify_without_disclosing_payloads() {
        for (packet, kind) in [
            (b"secret!!!".as_slice(), "unrecognized"),
            (&[0x80, 0xd5, 0, 1, 0xff, 0xfe, 0, 2][..], "retransmission"),
            (&[][..], "unrecognized"),
        ] {
            let emit = || control_diagnostic(packet);
            assert!(
                super::super::diagnostics::tests::capture(tracing::Level::INFO, emit).is_empty()
            );
            let log = super::super::diagnostics::tests::capture(tracing::Level::DEBUG, emit);
            assert!(log.contains(kind));
            assert!(!log.contains("secret"));
            assert!(log.contains(&format!("received_bytes_capped\":{}", packet.len())));
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
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&state)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("worker panicked")));
            if let Err(error) = result {
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
    fn timing_for_peer(
        socket: UdpSocket,
        lease: Lease,
        clock: Arc<super::realtime::Clock>,
        peer_ip: IpAddr,
    ) -> Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        socket.set_write_timeout(Some(Duration::from_millis(100)))?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let worker = Self::spawn(None, move |health| {
            lease.check()?;
            let mut b = [0; 33];
            ready_tx
                .send(())
                .context("timing readiness receiver closed")?;
            while !health.stop.load(Ordering::Relaxed) {
                match socket.recv_from(&mut b) {
                    Ok((n, peer)) => {
                        // OwnTone replies to recvfrom's source throughout the session.
                        // Restrict unauthenticated UDP probes to our verified peer IP.
                        if peer.ip() != peer_ip {
                            continue;
                        }
                        let recv = clock.now();
                        let reply = super::realtime::timing_reply(&b[..n], recv, clock.now());
                        let out = if let Ok(out) = reply {
                            out
                        } else {
                            continue;
                        };
                        lease.check()?;
                        tracing::debug!(target: "piair::rtsp", phase = "NTP", outcome = "request",
                            "NTP request from approved control peer");
                        ensure!(
                            socket.send_to(&out, peer)? == out.len(),
                            "timing reply send failed"
                        );
                        let replies = health.count.fetch_add(1, Ordering::Relaxed) + 1;
                        tracing::debug!(target: "piair::rtsp", phase = "NTP", outcome = "reply", replies,
                            "NTP reply sent to approved control peer");
                        *health.last.lock().unwrap() = Instant::now();
                    }
                    Err(e)
                        if [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut]
                            .contains(&e.kind()) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        });
        ready_rx
            .recv_timeout(Duration::from_secs(1))
            .context("timing readiness timeout")?;
        worker.check()?;
        Ok(worker)
    }
    fn events(mut wire: Wire) -> Result<Self> {
        wire.reauthorize()?;
        ensure!(wire.keys.is_some(), "encrypted event keys required");
        wire.socket
            .set_read_timeout(Some(Duration::from_millis(100)))?;
        let closer = Some(wire.socket.try_clone()?);
        tracing::debug!(target: "piair::rtsp", "encrypted event channel established; idle permitted");
        Ok(Self::spawn(closer, move |health| {
            while !health.stop.load(Ordering::Relaxed) {
                wire.reauthorize()?;
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
                let kind = d
                    .get("type")
                    .and_then(plist::Value::as_string)
                    .context("invalid receiver event type")?;
                ensure!(!kind.is_empty(), "empty receiver event type");
                match kind {
                    "updateInfo" => {
                        ensure!(
                            d.get("value").is_some_and(|v| v.as_dictionary().is_some()),
                            "invalid updateInfo value"
                        );
                        tracing::debug!(target: "piair::rtsp", "updateInfo event acknowledged");
                    }
                    "sendMediaRemoteCommand" => {
                        ensure!(
                            d.get("value").and_then(plist::Value::as_string).is_some(),
                            "invalid sendMediaRemoteCommand value"
                        );
                        // PiAir implements no receiver-driven playback operations.
                        tracing::debug!(target: "piair::rtsp", "remote command ignored and acknowledged");
                    }
                    _ => {
                        // Never dispatch unknown types or log receiver-supplied values.
                        tracing::debug!(target: "piair::rtsp", "unsupported event ignored and acknowledged");
                    }
                }
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
            tracing::debug!(target: "piair::rtsp", exchanges = self.count(), "receiver worker joined; socket released");
        }
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn timing_worker_transfer_rejects_duplicates_and_retains_failed_initialization() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let _peer = listener.accept().unwrap();
        let mut wire = Wire::mock(client, Duration::from_secs(1), "test");
        wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
        let mut ports = wire.bind_media().unwrap();
        let addr = ports.timing.as_ref().unwrap().local_addr().unwrap();
        let clock = Arc::new(super::super::realtime::Clock::new().unwrap());
        wire.start_timing(&mut ports, clock.clone()).unwrap();
        assert!(wire.start_timing(&mut ports, clock).is_err());
        ports.check_timing().unwrap();
        let mut pending = Some(ports);
        // Invalid media initialization must not consume the live responder.
        assert!(wire.media(&mut pending, 0, 7001).is_err());
        pending.as_ref().unwrap().check_timing().unwrap();
        let media = wire.media(&mut pending, 7000, 7001).unwrap();
        assert!(pending.is_none());
        media.timing.check().unwrap();
        drop(media);
        drop(UdpSocket::bind(addr).unwrap());
        // Exercise the same health propagation with a failed socket owner.
        let mut ports = wire.bind_media().unwrap();
        let socket = ports.timing.take().unwrap();
        let addr = socket.local_addr().unwrap();
        ports.timing_worker = Some(Worker::spawn(None, move |_| {
            let _socket = socket;
            bail!("injected timing failure")
        }));
        let worker = ports.timing_worker.as_ref().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !worker.join.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let mut pending = Some(ports);
        let error = wire.media(&mut pending, 7000, 7001).err().unwrap();
        assert!(format!("{error:#}").contains("injected timing failure"));
        assert!(pending.is_some());
        drop(pending);
        UdpSocket::bind(addr).unwrap();
    }

    #[test]
    fn early_timing_ready_filters_packets_and_joins_exactly_one_worker() {
        for _ in 0..8 {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let addr = socket.local_addr().unwrap();
            let lease = Lease {
                authorization: None,
                id: "test".into(),
            };
            let worker = Worker::timing_for_peer(
                socket,
                lease,
                Arc::new(super::super::realtime::Clock::new().unwrap()),
                "127.0.0.1".parse().unwrap(),
            )
            .unwrap();
            let health = worker.health.clone();
            assert_eq!(Arc::strong_count(&health), 3);
            let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
            peer.set_read_timeout(Some(Duration::from_millis(250)))
                .unwrap();
            let mut query = [0; 32];
            query[..2].copy_from_slice(&[0x80, 0xd2]);
            peer.send_to(b"malformed", addr).unwrap();
            let other = UdpSocket::bind("127.0.0.2:0").unwrap();
            other
                .set_read_timeout(Some(Duration::from_millis(150)))
                .unwrap();
            other.send_to(&query, addr).unwrap();
            let mut reply = [0; 33];
            assert!(other.recv_from(&mut reply).is_err());
            peer.send_to(&query, addr).unwrap();
            assert_eq!(peer.recv_from(&mut reply).unwrap().0, 32);
            let started = Instant::now();
            drop(worker);
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(health.count.load(Ordering::Relaxed), 1);
            assert_eq!(
                Arc::strong_count(&health),
                1,
                "worker thread must be joined"
            );
            let rebound = UdpSocket::bind(addr).unwrap();
            drop(rebound);
        }
    }

    #[test]
    fn independent_get_then_setup_preserves_directional_frame_state() {
        // Python cryptography/plistlib oracle; no mock use of ControlKeys.
        // These are synthetic protocol fixtures, not receiver acceptance evidence.
        for (reply, failure) in [
            (
                include_bytes!("../../tests/fixtures/sequence-setup.rx").as_slice(),
                None,
            ),
            (
                include_bytes!("../../tests/fixtures/sequence-reset.rx").as_slice(),
                Some("authentication tag invalid"),
            ),
            (
                include_bytes!("../../tests/fixtures/sequence-wrong-cseq.rx").as_slice(),
                Some("unexpected receiver CSeq"),
            ),
            (
                include_bytes!("../../tests/fixtures/sequence-duplicate-length.rx").as_slice(),
                Some("duplicate receiver header"),
            ),
            (
                include_bytes!("../../tests/fixtures/sequence-extra.rx").as_slice(),
                Some("unsolicited control data"),
            ),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let server = thread::spawn(move || {
                for (expected, response) in [
                    (
                        include_bytes!("../../tests/fixtures/sequence-get.tx").as_slice(),
                        include_bytes!("../../tests/fixtures/sequence-info.rx").as_slice(),
                    ),
                    (
                        include_bytes!("../../tests/fixtures/sequence-setup.tx").as_slice(),
                        reply,
                    ),
                ] {
                    let mut actual = vec![0; expected.len()];
                    peer.read_exact(&mut actual).unwrap();
                    assert_eq!(actual, expected);
                    // Split length prefixes, ciphertext, tags, headers and body.
                    for fragment in response.chunks(7) {
                        peer.write_all(fragment).unwrap();
                    }
                }
            });
            let mut wire = Wire::mock(socket, Duration::from_secs(3), "fixture");
            wire.seq = 2; // Completed plaintext pair-verify, no encrypted frames yet.
            wire.client_instance = 0x123456789ABCDEF0;
            wire.active_remote = 123456789;
            let secret: Vec<u8> = (0..32).collect();
            let mut wire = wire
                .secure(
                    super::super::crypto::ControlKeys::new(&secret),
                    super::super::crypto::ControlKeys::events(&secret),
                )
                .unwrap();
            let info = wire.request("GET", "/info", &[], &[]).unwrap();
            assert_eq!(
                info.body.as_slice(),
                include_bytes!("../../tests/fixtures/sequence-info.plist")
            );
            assert_eq!(wire.response_frames, 2);
            assert_eq!(wire.pending_start, wire.pending_end);
            let result = wire.request(
                "SETUP",
                "rtsp://127.0.0.1/305419896",
                &[("Content-Type", "application/x-apple-binary-plist".into())],
                include_bytes!("../../tests/fixtures/sequence-setup.plist"),
            );
            if let Some(message) = failure {
                assert!(format!("{:#}", result.err().unwrap()).contains(message));
                assert!(wire.failed && wire.keys.is_none());
            } else {
                assert!(result.unwrap().body.is_empty());
                assert_eq!(wire.response_frames, 1); // Metric resets; RX nonce continues at 2.
                assert_eq!(wire.seq, 4);
                assert!(!wire.failed);
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn independent_outbound_three_frame_message() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut peer = listener.accept().unwrap().0;
        peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut wire = Wire::mock(socket, Duration::from_secs(3), "fixture");
        let secret: Vec<u8> = (0..32).collect();
        wire.keys = Some(super::super::crypto::ControlKeys::new(&secret));
        let plain: Vec<u8> = (0..=255).cycle().take(2304).collect();
        wire.write_message(&plain).unwrap();
        let expected = include_bytes!("../../tests/fixtures/sequence-bulk.tx");
        let mut actual = vec![0; expected.len()];
        peer.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn diagnostic_setup_wait_has_one_deadline_for_silence_and_partial_frames() {
        for partial in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            let mut wire = Wire::mock(socket, Duration::from_secs(1), "test");
            wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
            wire.setup_response_wait =
                Some((Duration::from_millis(350), Arc::new(AtomicBool::new(false))));
            let server = thread::spawn(move || {
                if partial {
                    let mut keys = super::super::crypto::ControlKeys::new(&[7; 32]);
                    keys.swap_directions_for_test();
                    let frame = keys
                        .encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                    // Dribble ciphertext across poll timeouts without completing a frame.
                    for chunk in frame[..6].chunks(2) {
                        peer.write_all(chunk).unwrap();
                        thread::sleep(Duration::from_millis(120));
                    }
                }
                let mut bytes = Vec::new();
                peer.read_to_end(&mut bytes).unwrap();
            });
            let start = Instant::now();
            let error = wire
                .request("SETUP", "rtsp://127.0.0.1/1", &[], &[])
                .err()
                .unwrap();
            assert!(format!("{error:#}").contains("timed out"));
            assert!(start.elapsed() >= Duration::from_millis(300));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(wire.response_frames, 0);
            assert_eq!(wire.response_bytes > 0, partial);
            assert!(wire.failed && wire.keys.is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn feedback_deadline_covers_silence_and_partial_frames_without_reset() {
        for partial in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            let mut wire = Wire::mock(socket, Duration::from_secs(15), "test");
            wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
            wire.diagnostic_cancel = Some(Arc::new(AtomicBool::new(false)));
            wire.feedback_budget = Some(Duration::from_millis(50));
            let server = thread::spawn(move || {
                peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                if partial {
                    let mut keys = super::super::crypto::ControlKeys::new(&[7; 32]);
                    keys.swap_directions_for_test();
                    let frame = keys
                        .encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                    peer.write_all(&frame[..3]).unwrap();
                }
                let mut received = Vec::new();
                peer.read_to_end(&mut received).unwrap();
                assert!(!received.is_empty());
            });
            let began = Instant::now();
            let error = wire.request("POST", "/feedback", &[], &[]).err().unwrap();
            assert!(matches!(
                error.downcast_ref::<ResponseFailure>(),
                Some(ResponseFailure::Timeout)
            ));
            assert!(began.elapsed() < Duration::from_millis(500));
            assert!(wire.failed && wire.keys.is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn diagnostic_setup_wait_accepts_delayed_fragmented_response_and_preserves_default() {
        for diagnostic in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            let mut wire = Wire::mock(socket, Duration::from_secs(15), "test");
            wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
            if diagnostic {
                wire.setup_response_wait =
                    Some((Duration::from_secs(15), Arc::new(AtomicBool::new(false))));
            }
            let server = thread::spawn(move || {
                thread::sleep(Duration::from_millis(3300));
                if diagnostic {
                    let mut keys = super::super::crypto::ControlKeys::new(&[7; 32]);
                    keys.swap_directions_for_test();
                    let frame = keys
                        .encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                        .unwrap();
                    peer.write_all(&frame[..1]).unwrap();
                    thread::sleep(Duration::from_millis(150));
                    peer.write_all(&frame[1..]).unwrap();
                }
            });
            let start = Instant::now();
            let result = wire.request("SETUP", "rtsp://127.0.0.1/1", &[], &[]);
            assert_eq!(result.is_ok(), diagnostic);
            if diagnostic {
                assert_eq!(wire.response_frames, 1);
                assert!(!wire.polling_response);
                wire.setup_response_wait = None;
                wire.restart_deadline();
                wire.timeout().unwrap();
                assert_eq!(
                    wire.socket.read_timeout().unwrap(),
                    Some(Duration::from_secs(3))
                );
            } else {
                assert!(format!("{:#}", result.err().unwrap()).contains("receiver read timeout"));
                assert!(start.elapsed() < Duration::from_millis(3300));
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn diagnostic_setup_wait_cancels_silence_and_partial_response_promptly() {
        for partial in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            let mut wire = Wire::mock(socket, Duration::from_secs(15), "test");
            wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
            let cancel = Arc::new(AtomicBool::new(false));
            wire.setup_response_wait = Some((Duration::from_secs(15), cancel.clone()));
            if partial {
                peer.write_all(&[1]).unwrap();
            }
            let trigger = thread::spawn(move || {
                thread::sleep(Duration::from_millis(150));
                cancel.store(true, Ordering::Relaxed);
            });
            let start = Instant::now();
            let error = wire
                .request("SETUP", "rtsp://127.0.0.1/1", &[], &[])
                .err()
                .unwrap();
            assert!(format!("{error:#}").contains("diagnostic interrupted"));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert_eq!(wire.response_bytes, usize::from(partial));
            assert!(wire.failed && wire.keys.is_none());
            trigger.join().unwrap();
        }
    }

    #[test]
    fn sender_identity_retains_all_persistent_bits_across_connections() {
        let temp = tempfile::tempdir().unwrap();
        let store = super::super::store::Store::new(&temp.path().join("config.toml")).unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut wire = Wire::mock(socket, Duration::from_secs(1), "test");
            (wire.client_instance, wire.active_remote) = store.client_identity().unwrap();
            let id = wire.sender_device_id();
            assert_eq!(
                id.replace(':', ""),
                format!("{:016X}", wire.client_instance)
            );
            ids.push(id);
        }
        assert_eq!(ids[0], ids[1]);
    }

    #[test]
    fn setup_response_progress_distinguishes_silence_partial_frames_and_closure() {
        for mode in 0..4 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            let mut wire = Wire::mock(socket, Duration::from_millis(80), "test");
            wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
            wire.auth_phase = "pair-verify M3/M4";
            if mode == 1 {
                peer.write_all(&[1]).unwrap();
            } else if mode == 2 {
                let mut keys = super::super::crypto::ControlKeys::new(&[7; 32]);
                keys.swap_directions_for_test();
                peer.write_all(&keys.encrypt(b"RTSP/1.0 200").unwrap())
                    .unwrap();
            } else if mode == 3 {
                peer.shutdown(Shutdown::Write).unwrap();
            }
            let mut result = None;
            let log = super::super::diagnostics::tests::capture(tracing::Level::DEBUG, || {
                result = Some(wire.request("SETUP", "rtsp://127.0.0.1/1", &[], &[]));
            });
            let error = result.unwrap().err().unwrap();
            assert!(log.contains("authenticated request completed"));
            assert!(log.contains("response_bytes") && log.contains("authenticated_frames"));
            assert!(!log.contains("pair-verify"));
            let text = format!("{error:#}");
            assert!(
                text.contains("method=SETUP phase=encrypted SETUP"),
                "{text}"
            );
            assert!(!text.contains("pair-verify"));
            assert_eq!(wire.response_frames, usize::from(mode == 2));
            assert_eq!(wire.response_bytes > 0, mode == 1 || mode == 2);
            assert!(text.contains(if mode == 3 {
                "truncated receiver response"
            } else {
                "timeout"
            }));
            assert!(wire.failed && wire.keys.is_none());
        }
    }

    #[test]
    fn options_sequence_exhaustion_closes_without_sending_or_wrapping() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut peer = listener.accept().unwrap().0;
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut wire = Wire::mock(socket, Duration::from_secs(1), "test");
        wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
        wire.seq = u32::MAX;
        assert!(wire
            .options()
            .unwrap_err()
            .to_string()
            .contains("CSeq exhausted"));
        assert_eq!(wire.seq, u32::MAX);
        assert!(wire.failed && wire.keys.is_none());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn setup_write_failure_is_attributed_and_closes_channel() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut peer = listener.accept().unwrap().0;
        let mut wire = Wire::mock(socket, Duration::from_secs(1), "test");
        wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
        wire.socket.shutdown(Shutdown::Write).unwrap();
        let error = wire
            .request("SETUP", "rtsp://127.0.0.1/1", &[], &[])
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("encrypted request write"));
        assert!(wire.failed && wire.keys.is_none());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn initial_setup_matches_independent_encrypted_sender_fixture() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut peer = listener.accept().unwrap().0;
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let shared: Vec<u8> = (0..32).collect();
        let mut wire = Wire::mock(socket, Duration::from_secs(2), "test");
        wire.keys = Some(super::super::crypto::ControlKeys::new(&shared));
        wire.client_instance = 0x123456789abcdef0;
        wire.active_remote = 123456789;
        let server = thread::spawn(move || {
            let expected = include_bytes!("../../tests/fixtures/initial-setup.frame");
            let mut actual = vec![0; expected.len()];
            peer.read_exact(&mut actual).unwrap();
            assert_eq!(actual, expected);
            let mut keys = super::super::crypto::ControlKeys::new(&shared);
            keys.swap_directions_for_test();
            peer.write_all(
                &keys
                    .encrypt(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
                    .unwrap(),
            )
            .unwrap();
        });
        wire.request(
            "SETUP",
            "rtsp://127.0.0.1/305419896",
            &[("Content-Type", "application/x-apple-binary-plist".into())],
            include_bytes!("../../tests/fixtures/initial-setup.plist"),
        )
        .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn malformed_setup_is_attributed_to_construction_and_sends_nothing() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let mut peer = listener.accept().unwrap().0;
        let mut wire = Wire::mock(socket, Duration::from_secs(1), "test");
        wire.keys = Some(super::super::crypto::ControlKeys::new(&[7; 32]));
        let error = wire.request("SETUP", "bad uri", &[], &[]).err().unwrap();
        assert!(format!("{error:#}").contains("request construction"));
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn event_types_are_acknowledged_without_actions_and_failures_are_detected() {
        for mode in 0..10 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut wire = Wire::mock(socket, Duration::from_secs(1), "test");
            wire.keys = Some(super::super::crypto::ControlKeys::events(&[7; 32]));
            let worker = Worker::events(wire).unwrap();
            let mut keys = super::super::crypto::ControlKeys::events(&[7; 32]);
            keys.swap_directions_for_test();
            if mode == 9 {
                peer.shutdown(Shutdown::Write).unwrap();
            } else {
                let (kind, value) = match mode {
                    0 => ("updateInfo", super::super::profile::dict([])),
                    1..=4 => (
                        "sendMediaRemoteCommand",
                        plist::Value::String(["play", "paus", "nitm", "pitm"][mode - 1].into()),
                    ),
                    5 => ("unknown", plist::Value::String("play".into())),
                    _ => ("updateInfo", plist::Value::String("invalid".into())),
                };
                let mut body = Vec::new();
                super::super::profile::dict([
                    ("type", plist::Value::String(kind.into())),
                    ("value", value),
                ])
                .to_writer_binary(&mut body)
                .unwrap();
                let mut message = format!("POST /command RTSP/1.0\r\nCSeq: 7\r\nContent-Length: {}\r\nContent-Type: application/x-apple-binary-plist\r\n{}\r\n",
                    body.len(), if mode == 8 { "Transfer-Encoding: chunked\r\n" } else { "" }).into_bytes();
                message.extend(body);
                let mut frame = keys.encrypt(&message).unwrap();
                if mode == 7 {
                    frame[3] ^= 1;
                }
                peer.write_all(&frame).unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            if mode <= 5 {
                let mut length = [0; 2];
                peer.read_exact(&mut length).unwrap();
                let mut frame = vec![0; usize::from(u16::from_le_bytes(length)) + 18];
                frame[..2].copy_from_slice(&length);
                peer.read_exact(&mut frame[2..]).unwrap();
                assert!(keys
                    .decrypt(&frame)
                    .unwrap()
                    .starts_with(b"RTSP/1.0 200 OK\r\nCSeq: 7\r\n"));
                while worker.count() == 0 {
                    assert!(Instant::now() < deadline);
                    thread::yield_now();
                }
                worker.check().unwrap();
                assert_eq!(worker.count(), 1);
            } else {
                while worker.check().is_ok() {
                    assert!(Instant::now() < deadline);
                    thread::park_timeout(Duration::from_millis(1));
                }
                assert_eq!(worker.count(), 0);
            }
            drop(worker);
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn partial_encrypted_event_is_interrupted_by_cleanup() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let socket = listener.accept().unwrap().0;
        let mut wire = Wire::mock(socket, Duration::from_secs(3), "test");
        wire.keys = Some(super::super::crypto::ControlKeys::events(&[7; 32]));
        // Only half of the encrypted frame length: read_exact cannot finish.
        peer.write_all(&[1]).unwrap();
        let worker = Worker::events(wire).unwrap();
        thread::park_timeout(Duration::from_millis(50));
        let start = Instant::now();
        drop(worker);
        assert!(start.elapsed() < Duration::from_secs(1));
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn panicked_worker_reports_failure_and_cleanup_does_not_panic() {
        let worker = Worker::spawn(None, |_| panic!("test worker panic"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while !worker.join.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline);
            thread::park_timeout(Duration::from_millis(1));
        }
        assert!(format!("{:#}", worker.check().unwrap_err()).contains("worker panicked"));
        drop(worker);
    }
}

#[cfg(test)]
mod auth_diagnostic_tests {
    use super::*;
    #[test]
    fn rejection_timeout_and_peer_close_are_safe_and_do_not_retry() {
        for scenario in ["403", "timeout", "close"] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let mut peer = listener.accept().unwrap().0;
            let server = thread::spawn(move || {
                peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut b = [0];
                    peer.read_exact(&mut b).unwrap();
                    request.push(b[0]);
                }
                assert!(request.starts_with(b"POST /pair-setup HTTP/1.1\r\n"));
                let mut body = [0; 12];
                peer.read_exact(&mut body).unwrap();
                assert_eq!(&body, b"secret-proof");
                if scenario == "403" {
                    peer.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\nSet-Cookie: secret-cookie\r\nX-Unexpected: secret-token\r\n\r\n").unwrap();
                }
                if scenario != "close" {
                    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
                }
            });
            let output = super::super::diagnostics::tests::capture(tracing::Level::DEBUG, || {
                let mut wire = Wire::mock_method(
                    socket,
                    Duration::from_millis(100),
                    "test",
                    Method::Transient,
                );
                wire.auth_phase("pair-setup M1/M2");
                assert!(wire.post("/pair-setup", b"secret-proof").is_err());
                assert!(wire.failed);
            });
            server.join().unwrap();
            assert!(output.contains("authentication exchange completed"));
            assert!(output.contains("elapsed_ms"));
            assert!(!output.contains("secret"));
            assert!(
                output.contains(match scenario {
                    "403" => "Forbidden",
                    "timeout" => "connection read failed",
                    _ => "peer closed connection",
                }),
                "{output}"
            );
        }
    }
}

#[cfg(test)]
pub(super) fn owntone_test_header(method: &str, path: &str, seq: u32, length: usize) -> String {
    // Literal independent expectation, not the serializer under test.
    let suffix = if method == "POST" {
        format!("Content-Type: application/octet-stream\r\nX-Apple-HKP: 4\r\nContent-Length: {length}\r\n")
    } else {
        String::new()
    };
    format!("{method} {path} RTSP/1.0\r\nCSeq: {seq}\r\nUser-Agent: AirPlay/999.0.0\r\nX-Apple-Client-Name: PiAir\r\nClient-Instance: 123456789ABCDEF0\r\nDACP-ID: 123456789ABCDEF0\r\nActive-Remote: 123456789\r\n{suffix}\r\n")
}
#[cfg(test)]
impl Wire {
    pub(super) fn owntone_test_identity(&mut self) {
        self.client_instance = 0x123456789abcdef0;
        self.active_remote = 123456789;
    }
}

/// Local route lookup only; UDP connect does not transmit a datagram.
fn validate_timing_interface(local: IpAddr, peer: IpAddr) -> Result<()> {
    ensure!(peer != local, "timing peer is the local control address");
    ensure!(
        !matches!(local, IpAddr::V6(v) if v.is_unicast_link_local()),
        "IPv6 link-local timing interface scope unsupported"
    );
    ensure!(
        local.is_ipv4() == peer.is_ipv4(),
        "timing interface address family mismatch"
    );
    // Route lookup only: UDP connect sends no packet. Reject a destination
    // whose default route selects a different local interface/address.
    let wildcard: IpAddr = if local.is_ipv4() { "0.0.0.0" } else { "::" }.parse()?;
    let route = UdpSocket::bind(SocketAddr::new(wildcard, 0))?;
    route
        .connect(SocketAddr::new(peer, 319))
        .context("timing interface route unavailable")?;
    tracing::debug!(target: "piair::ptp", selected_peer = %peer,
            control_local = %local, route_local = %route.local_addr()?.ip(),
            "validating timing interface route");
    ensure!(
        route.local_addr()?.ip() == local,
        "timing peer route differs from authorized control interface"
    );
    Ok(())
}

#[cfg(test)]
mod timing_interface_tests {
    use super::*;
    #[test]
    fn route_must_use_control_local_address_and_supported_family() {
        let local = "127.0.0.1".parse().unwrap();
        assert!(validate_timing_interface(local, "127.0.0.2".parse().unwrap()).is_ok());
        for (local, peer) in [
            ("127.0.0.2", "127.0.0.3"),
            ("127.0.0.1", "127.0.0.1"),
            ("127.0.0.1", "2001:db8::1"),
            ("fe80::1", "2001:db8::1"),
        ] {
            assert!(
                validate_timing_interface(local.parse().unwrap(), peer.parse().unwrap()).is_err()
            );
        }
    }
}
