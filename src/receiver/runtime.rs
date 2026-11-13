//! Foreground automatic playback. No terminal access or persistent configuration writes.
//! Session remains the sole owner of authentication, PTP, media and teardown.
use super::{automatic::Retry, store, wire, Selection};
use crate::{
    audio::{activity, live::Capture},
    config::{self, Loaded},
    discovery,
};
use anyhow::{ensure, Context, Result};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

mod instance;
pub(super) use instance::InstanceLock;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Authentication {
    StoredHap,
    OwnToneTransient,
}

pub struct Settings {
    pub capture_device: String,
    pub detection: activity::Settings,
    pub receiver_id: String,
    pub authentication: Authentication,
    pub volume_db: Option<i32>,
}
impl Settings {
    pub(crate) fn validate(&self) -> Result<()> {
        super::exact_id(&self.receiver_id)?;
        self.detection.validate()?;
        config::validate_capture_device(&self.capture_device)?;
        ensure!(
            self.volume_db.is_none_or(|v| (-60..=-10).contains(&v)),
            "volume must be -60..=-10 dB"
        );
        Ok(())
    }
}

pub fn signal_cancellation() -> Result<Arc<AtomicBool>> {
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))?;
    Ok(cancel)
}

/// Opt-in applies only to this invocation, receiver and typed settings. It never
/// grants approval, chooses a receiver, pairs, or changes authentication methods.
pub fn unattended(loaded: &Loaded, settings: Settings, allow_unattended: bool) -> Result<()> {
    ensure!(allow_unattended, "run requires explicit --allow-unattended");
    let cancel = signal_cancellation()?;
    let runtime = Runtime::prepare(loaded, settings)?;
    tracing::info!(target: "piair::runtime", event = "unattended_authorized",
        receiver = %runtime.settings.receiver_id, authentication = ?runtime.settings.authentication,
        capture_device = %runtime.settings.capture_device, threshold_dbfs = runtime.settings.detection.threshold_dbfs,
        start_ms = runtime.settings.detection.start_ms, stop_ms = runtime.settings.detection.stop_ms,
        volume_db = ?runtime.settings.volume_db, "explicit unattended invocation authorized");
    runtime.run(cancel)
}

/// Immutable invocation pin. Preparation is read-only and never contacts a receiver.
pub struct Runtime {
    path: PathBuf,
    discovery_timeout: u64,
    settings: Settings,
    selected: Selection,
    operation: Selection,
    credential: Option<store::Credential>,
}
impl Runtime {
    pub fn prepare(loaded: &Loaded, settings: Settings) -> Result<Self> {
        settings.validate()?;
        ensure!(
            loaded.config.allows(&settings.receiver_id),
            "receiver is not approved"
        );
        ensure!(
            config::approval(&loaded.path, &settings.receiver_id)?,
            "receiver is not approved"
        );
        let store = store::Store::new(&loaded.path)?;
        let selected = store
            .selection()?
            .context("select this approved receiver first")?;
        ensure!(
            selected.id == settings.receiver_id,
            "receiver is not selected"
        );
        let (operation, credential) = super::wav_authentication(
            &store,
            &selected,
            matches!(settings.authentication, Authentication::OwnToneTransient),
        )?;
        let required = (1u64 << 9) | (1u64 << 19) | (1u64 << 41);
        ensure!(
            selected.capabilities.features & required == required,
            "selected receiver must advertise audio, ALAC and PTP; no fallback"
        );
        let runtime = Self {
            path: loaded.path.clone(),
            discovery_timeout: loaded.config.discovery_timeout_secs,
            settings,
            selected,
            operation,
            credential,
        };
        runtime.check()?;
        Ok(runtime)
    }
    fn check(&self) -> Result<()> {
        ensure!(
            config::approval(&self.path, &self.selected.id)?,
            "receiver approval revoked"
        );
        let store = store::Store::new(&self.path)?;
        ensure!(
            store.selection()?.as_ref() == Some(&self.selected),
            "receiver selection changed"
        );
        if let Some(expected) = &self.credential {
            let current = store
                .credential(&self.selected.id)?
                .context("operation credentials removed")?;
            use subtle::ConstantTimeEq;
            ensure!(
                bool::from(
                    current
                        .fingerprint()?
                        .as_ref()
                        .ct_eq(expected.fingerprint()?.as_ref())
                ),
                "operation credentials changed or revoked"
            );
        }
        Ok(())
    }
    pub fn run(self, cancel: Arc<AtomicBool>) -> Result<()> {
        let instance = InstanceLock::acquire()?;
        self.run_locked(cancel, instance)
    }
    pub(super) fn run_locked(self, cancel: Arc<AtomicBool>, _instance: InstanceLock) -> Result<()> {
        self.check()?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let capture = Capture::automatic(&self.settings.capture_device, self.settings.detection)?;
        let mut engine = Engine {
            runtime: self,
            capture,
            cancel: cancel.clone(),
        };
        let result = supervise(&mut engine, &cancel);
        // Dropping the sole capture owner stops and joins its nonblocking worker.
        drop(engine);
        tracing::info!(target: "piair::runtime", event = "shutdown", success = result.is_ok(), "runtime resources released");
        result
    }
    fn attempt(&self, capture: &Capture, cancel: Arc<AtomicBool>) -> Result<()> {
        self.attempt_with_discovery(&cancel, discovery::discover_cancellable, |snapshot| {
            self.play_discovered(capture, cancel.clone(), snapshot)
        })
    }
    // Keep the discovery/authentication boundary injectable without opening ALSA
    // or sockets in tests. Both setup and automatic playback validate through
    // playback_discovery; only the runtime additionally pins persisted state.
    fn attempt_with_discovery(
        &self,
        cancel: &AtomicBool,
        mut discover: impl FnMut(u64, &AtomicBool) -> Result<Vec<discovery::Receiver>>,
        start: impl FnOnce(&[discovery::Receiver]) -> Result<()>,
    ) -> Result<()> {
        self.check()?;
        let mut loaded = config::load(Some(self.path.clone()))?;
        // Preserve the invocation's configured discovery window.
        loaded.config.discovery_timeout_secs = self.discovery_timeout;
        let snapshot =
            super::playback_discovery(&loaded, &self.operation, cancel, |timeout, cancel| {
                self.check()?;
                discover(timeout, cancel)
            });
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let snapshot = snapshot?;
        self.check()?;
        start(&snapshot)
    }
    fn play_discovered(
        &self,
        capture: &Capture,
        cancel: Arc<AtomicBool>,
        snapshot: &[discovery::Receiver],
    ) -> Result<()> {
        let mut authenticated = super::session::Session::new();
        authenticated.cancel = cancel.clone();
        tracing::info!(target: "piair::runtime", event = "authentication", "selected receiver session starting");
        let result = authenticated.authenticate_ptp_selected(
            || match &self.credential {
                Some(credential) => wire::Wire::connect_hap_scoped(
                    &self.path,
                    &self.operation,
                    snapshot,
                    credential,
                    Some(&self.selected),
                ),
                None => {
                    wire::Wire::connect(&self.path, &self.selected.id, &self.operation, snapshot)
                }
            },
            self.credential.as_ref(),
        );
        // Before SETUP there is no remote session to tear down. Authentication
        // cancellation closes the wire through its existing owning Session.
        if cancel.load(Ordering::Relaxed) {
            if let Err(error) = result {
                tracing::warn!(error = %format!("{error:#}"), "authentication interrupted");
            }
            return Ok(());
        }
        result.context("selected receiver PTP authentication failed; no fallback")?;
        let mut session = match authenticated.into_ptp_foundation() {
            Ok(session) => session,
            Err(error) if cancel.load(Ordering::Relaxed) => {
                tracing::warn!(error = %format!("{error:#}"), "authenticated handoff interrupted");
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let result = (|| {
            tracing::info!(target: "piair::runtime", event = "negotiation", "PTP realtime negotiation starting");
            session.setup_realtime_authenticated()?;
            session.negotiate_realtime()?;
            tracing::info!(target: "piair::runtime", event = "playback", "automatic playback starting");
            session.play_automatic(capture.reader(), self.settings.volume_db)?;
            session.stop()
        })();
        tracing::info!(target: "piair::runtime", event = "session_stopped", diagnostic = %session.diagnostic(), "session resources released");
        finish_attempt(result, cancel.load(Ordering::Relaxed))
    }
}

// Cancellation may interrupt a request, but must never hide failed teardown.
pub(super) fn finish_attempt(result: Result<()>, cancelled: bool) -> Result<()> {
    if cancelled {
        if let Err(error) = result {
            if error.is::<super::lifecycle::CleanupFailure>() {
                return Err(error);
            }
            tracing::warn!(error = %format!("{error:#}"), "session interrupted");
        }
        return Ok(());
    }
    result
}

// This is the existing automatic orchestration loop, with a synthetic seam for
// time, capture and session attempts. It does not implement session states.
trait Driver {
    fn check(&mut self) -> Result<()>;
    fn meaningful(&self) -> Result<bool>;
    fn attempt(&mut self) -> Result<()>;
    fn rearm(&mut self) -> Result<()>;
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn wait(&mut self) {
        std::thread::park_timeout(Duration::from_millis(20));
    }
}
fn supervise(driver: &mut impl Driver, cancel: &AtomicBool) -> Result<()> {
    let mut retry = Retry::new(driver.now());
    tracing::info!(target: "piair::runtime", event = "monitoring", "waiting for sustained audio");
    loop {
        driver.check()?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !retry.ready(driver.now(), driver.meaningful()?) {
            driver.wait();
            continue;
        }
        let result = driver.attempt();
        // Capture errors and revoked authorization terminate, including during
        // cancellation. Neither becomes a successful shutdown or endless retry.
        driver.check()?;
        if cancel.load(Ordering::Relaxed) {
            return result;
        }
        match result {
            Ok(()) => {
                retry = Retry::new(driver.now());
                tracing::info!(target: "piair::runtime", event = "monitoring", "playback stopped; waiting for sustained audio");
            }
            Err(error) => {
                driver.rearm()?;
                let delay = retry.failed(driver.now());
                tracing::warn!(target: "piair::runtime", event = "retry", delay_secs = delay,
                    error = %format!("{error:#}"), "session failed; fresh sustained audio required");
            }
        }
    }
}
struct Engine {
    runtime: Runtime,
    capture: Capture,
    cancel: Arc<AtomicBool>,
}
impl Driver for Engine {
    fn check(&mut self) -> Result<()> {
        self.capture.activity()?;
        self.runtime.check()
    }
    fn meaningful(&self) -> Result<bool> {
        Ok(self.capture.activity()?.0)
    }
    fn attempt(&mut self) -> Result<()> {
        self.runtime.attempt(&self.capture, self.cancel.clone())
    }
    fn rearm(&mut self) -> Result<()> {
        self.capture.rearm()
    }
}

#[cfg(test)]
mod tests;
