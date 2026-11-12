//! Fixed-format live input; ALSA plug PCMs handle native device conversion.
use alsa::{
    pcm::{Access, Format, HwParams, PCM},
    Direction, ValueOr,
};
use anyhow::{ensure, Context, Result};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
const RATE: u32 = 44100;
const SPF: usize = 352;
const CAPACITY: usize = SPF * 24;
const PREFILL: usize = SPF * 12;
const WAIT: Duration = Duration::from_millis(20);

#[derive(Default)]
struct Buffer {
    frames: VecDeque<[i16; 2]>,
    automatic: Option<super::activity::Activity>,
    last_samples: Option<Instant>,
    error: Option<String>,
    dropped: u64,
    silence: u64,
    recoveries: u64,
}
impl Buffer {
    fn push(&mut self, samples: &[i16]) {
        if self.automatic.is_some() {
            self.last_samples = Some(Instant::now());
        }
        for pair in samples.as_chunks::<2>().0 {
            if let Some(activity) = &mut self.automatic {
                let generation = activity.generation;
                activity.push(*pair);
                if activity.generation != generation {
                    let keep = activity.retained_start_frames();
                    self.dropped = 0;
                    while self.frames.len() > keep {
                        self.frames.pop_front();
                    }
                }
            }
            let capacity = if self.automatic.is_some() {
                RATE as usize * 10
            } else {
                CAPACITY
            };
            if self.frames.len() == capacity {
                self.frames.pop_front();
                self.dropped += 1;
            }
            self.frames.push_back(*pair);
        }
    }
    fn recovered(&mut self) {
        // The old ALSA epoch is no longer contiguous with the new one. Bound
        // latency by discarding its queue; the sender fills missing slots.
        self.dropped += self.frames.len() as u64;
        self.frames.clear();
        self.recoveries += 1;
        if let Some(activity) = &mut self.automatic {
            activity.reset();
        }
        println!("ALSA capture recovered; interrupted PCM queue discarded");
    }
    fn check(&self) -> Result<()> {
        ensure!(
            self.error.is_none(),
            "ALSA capture failed: {}",
            self.error.as_deref().unwrap_or_default()
        );
        Ok(())
    }
    fn block(&mut self) -> Result<[[i16; 2]; SPF]> {
        self.check()?;
        Ok(std::array::from_fn(|_| {
            self.frames.pop_front().unwrap_or_else(|| {
                self.silence += 1;
                [0; 2]
            })
        }))
    }
}

pub(crate) enum Read {
    Frames(usize),
    Pending,
    Recovered,
}
/// Reads/recovery must be nonblocking; waits have a maximum WAIT duration.
/// Factories run on the capture worker, which exclusively owns the source.
pub(crate) trait Source {
    fn read(&mut self, samples: &mut [i16]) -> Result<Read>;
    fn wait(&self) -> Result<()> {
        thread::park_timeout(WAIT);
        Ok(())
    }
}
struct Alsa(PCM);
impl Alsa {
    fn open(device: &str) -> Result<Self> {
        match Self::open_exact(device) {
            Ok(source) => Ok(source),
            Err(original) => {
                // hw has no conversion layer. Retry the *same* device through
                // ALSA's documented plughw adapter, never another card/source.
                if let Some(hw) = device.strip_prefix("hw:") {
                    return Self::open_exact(&format!("plughw:{hw}")).with_context(|| {
                        format!("direct capture failed ({original:#}); plug conversion also failed")
                    });
                }
                Err(original).context("use plughw:CARD=<id>,DEV=0 or a configured plug PCM for native rate/format/channel conversion")
            }
        }
    }
    fn open_exact(device: &str) -> Result<Self> {
        let pcm = PCM::new(device, Direction::Capture, true).context("open capture device")?;
        // Period/buffer tuning is a preference, not an interoperability gate.
        Self::configure(&pcm, true).or_else(|_| Self::configure(&pcm, false))?;
        let actual = pcm.hw_params_current()?;
        ensure!(
            actual.get_access()? == Access::RWInterleaved,
            "capture requires interleaved access"
        );
        validate_actual(
            actual.get_rate()?,
            actual.get_channels()?,
            actual.get_format()?,
            actual.get_period_size()?,
            actual.get_buffer_size()?,
        )?;
        println!("ALSA live input {device:?}: {} Hz, {} channels, {:?}, interleaved; period={} frames, buffer={} frames", actual.get_rate()?, actual.get_channels()?, actual.get_format()?, actual.get_period_size()?, actual.get_buffer_size()?);
        drop(actual);
        pcm.prepare()?;
        Ok(Self(pcm))
    }
    fn configure(pcm: &PCM, tune: bool) -> Result<()> {
        let hw = HwParams::any(pcm)?;
        hw.set_access(Access::RWInterleaved)?;
        hw.set_channels(2)?;
        hw.set_format(Format::s16())?;
        hw.set_rate(RATE, ValueOr::Nearest)?;
        if tune {
            hw.set_period_size_near(SPF as i64, ValueOr::Nearest)?;
            hw.set_buffer_size_near(i64::from(RATE / 10))?;
        }
        pcm.hw_params(&hw)?;
        Ok(())
    }
}
fn validate_actual(
    rate: u32,
    channels: u32,
    format: Format,
    period: i64,
    buffer: i64,
) -> Result<()> {
    // Exact *client* format is necessary for the encoder; native hardware may
    // have any format/rate the ALSA plug layer can convert. Reads are fixed-size
    // and partial reads are valid, so no duration ceiling or period=SPF rule.
    ensure!(
        rate == RATE && channels == 2 && format == Format::s16(),
        "capture client requires 44100 Hz S16 stereo; use an ALSA plug PCM for conversion"
    );
    ensure!(period > 0 && buffer > 0, "invalid ALSA period/buffer size");
    Ok(())
}
impl Source for Alsa {
    fn read(&mut self, samples: &mut [i16]) -> Result<Read> {
        match self.0.io_i16()?.readi(samples) {
            Ok(0) => Ok(Read::Pending),
            Ok(n) => Ok(Read::Frames(n)),
            Err(e) if e.errno() == libc::EAGAIN || e.errno() == libc::EINTR => Ok(Read::Pending),
            Err(e) if e.errno() == libc::EPIPE || e.errno() == libc::ESTRPIPE => {
                // snd_pcm_recover may loop indefinitely trying to resume a
                // suspended device. Re-prepare this capture epoch instead.
                self.0.prepare()?;
                Ok(Read::Recovered)
            }
            Err(e) => Err(e.into()),
        }
    }
    fn wait(&self) -> Result<()> {
        match self.0.wait(Some(WAIT.as_millis() as u32)) {
            Ok(_) => Ok(()),
            // Let the next read perform recovery; no recursive retry loop.
            Err(e)
                if [libc::EPIPE, libc::ESTRPIPE, libc::EINTR, libc::EAGAIN]
                    .contains(&e.errno()) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }
}

pub struct Capture {
    buffer: Arc<Mutex<Buffer>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Capture {
    /// Starts the worker without waiting for ALSA or prefill. The session keeps
    /// timing/authorization alive while wait_ready observes startup.
    pub fn start(device: &str) -> Result<Self> {
        ensure!(
            !device.trim().is_empty()
                && device.len() <= 256
                && !device.chars().any(char::is_control),
            "invalid ALSA device"
        );
        let device = device.to_owned();
        Self::spawn(move || Alsa::open(&device))
    }
    pub(crate) fn spawn<S: Source>(
        open: impl FnOnce() -> Result<S> + Send + 'static,
    ) -> Result<Self> {
        let buffer = Arc::new(Mutex::new(Buffer {
            frames: VecDeque::with_capacity(CAPACITY),
            ..Buffer::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = buffer.clone();
        let stopped = stop.clone();
        let worker = thread::Builder::new()
            .name("piair-capture".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                        let mut source = open()?;
                        let mut samples = [0i16; SPF * 2];
                        let mut burst = 0;
                        while !stopped.load(Ordering::Acquire) {
                            match source.read(&mut samples)? {
                                Read::Frames(n) => {
                                    ensure!((1..=SPF).contains(&n), "invalid capture frame count");
                                    shared
                                        .lock()
                                        .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?
                                        .push(&samples[..n * 2]);
                                    burst += 1;
                                    // Even a synthetic/unclocked plug PCM cannot spin
                                    // forever. Real hardware normally reaches EAGAIN.
                                    if burst == 16 {
                                        thread::park_timeout(Duration::from_millis(1));
                                        burst = 0;
                                    }
                                }
                                Read::Recovered => {
                                    shared
                                        .lock()
                                        .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?
                                        .recovered();
                                    thread::park_timeout(WAIT);
                                    burst = 0;
                                }
                                Read::Pending => {
                                    let before = Instant::now();
                                    source.wait()?;
                                    // A misbehaving plug PCM or repeated EINTR
                                    // must not make this loop spin.
                                    if before.elapsed() < Duration::from_millis(1) {
                                        thread::park_timeout(Duration::from_millis(1));
                                    }
                                    burst = 0;
                                }
                            }
                        }
                        Ok(())
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("capture worker panicked")));
                if let Err(e) = result {
                    if let Ok(mut buffer) = shared.lock() {
                        buffer.error = Some(format!("{e:#}"));
                    }
                }
            })
            .context("start capture worker")?;
        Ok(Self {
            buffer,
            stop,
            worker: Some(worker),
        })
    }
    pub fn wait_ready(&self, cancel: &AtomicBool, check: impl FnMut() -> Result<()>) -> Result<()> {
        self.wait_ready_until(cancel, Instant::now() + Duration::from_secs(10), check)
    }
    fn wait_ready_until(
        &self,
        cancel: &AtomicBool,
        deadline: Instant,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<()> {
        loop {
            ensure!(!cancel.load(Ordering::Relaxed), "capture startup cancelled");
            check()?;
            {
                let b = self
                    .buffer
                    .lock()
                    .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?;
                b.check()?;
                if b.frames.len() >= PREFILL {
                    break;
                }
            }
            ensure!(
                Instant::now() < deadline,
                "capture startup timed out without sufficient samples"
            );
            thread::park_timeout(Duration::from_millis(5));
        }
        if self
            .buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?
            .automatic
            .is_some()
        {
            println!("Automatic capture: 44100 Hz S16 stereo; pre-roll/queue capacity=10 seconds; underrun=silence; overflow=drop oldest");
        } else {
            println!("Live capture: 44100 Hz S16 stereo; startup=96 ms; queue capacity=192 ms; underrun=silence; overflow=drop oldest");
        }
        Ok(())
    }
    /// Automatic owner outlives per-session readers. Only the owner joins ALSA.
    pub fn automatic(device: &str, settings: super::activity::Settings) -> Result<Self> {
        let settings = settings.validate()?;
        let capture = Self::start(device)?;
        capture.enable_automatic(settings)?;
        Ok(capture)
    }
    pub(crate) fn enable_automatic(&self, settings: super::activity::Settings) -> Result<()> {
        self.buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?
            .automatic = Some(super::activity::Activity::new(settings.validate()?));
        Ok(())
    }
    pub fn activity(&self) -> Result<(bool, u64)> {
        let b = self
            .buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?;
        b.check()?;
        let a = b.automatic.as_ref().context("automatic detection absent")?;
        Ok((
            a.meaningful()
                && b.last_samples
                    .is_some_and(|at| at.elapsed() < Duration::from_millis(100)),
            a.generation,
        ))
    }
    pub fn reader(&self) -> Self {
        if let Ok(b) = self.buffer.lock() {
            println!("AirPlay pre-roll: {} retained frames; {} frames lost to bounded-buffer overflow since detection (startup preservation is limited to ten seconds)", b.frames.len(), b.dropped);
        }
        Self {
            buffer: self.buffer.clone(),
            stop: self.stop.clone(),
            worker: None,
        }
    }
    pub fn ended(&self) -> Result<bool> {
        let b = self
            .buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?;
        b.check()?;
        Ok(b.automatic.as_ref().is_some_and(|a| !a.active))
    }
    pub fn block(&mut self) -> Result<[[i16; 2]; SPF]> {
        self.buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("capture buffer poisoned"))?
            .block()
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
        if let Ok(b) = self.buffer.lock() {
            println!("Live capture stopped: dropped_frames={} silence_frames={} recoveries={}; ALSA released", b.dropped, b.silence, b.recoveries);
        }
    }
}
#[cfg(test)]
pub(crate) mod tests;
