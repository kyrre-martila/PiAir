//! Fixed-format live input. ALSA's plug layer performs rate/format conversion.
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
const SPF: usize = 352;
const CAPACITY: usize = SPF * 24;
const PREFILL: usize = SPF * 12;
#[derive(Default)]
struct Buffer {
    frames: VecDeque<[i16; 2]>,
    error: Option<String>,
    dropped: u64,
    silence: u64,
}
impl Buffer {
    fn push(&mut self, samples: &[i16]) {
        for pair in samples.as_chunks::<2>().0 {
            if self.frames.len() == CAPACITY {
                self.frames.pop_front();
                self.dropped += 1;
            }
            self.frames.push_back([pair[0], pair[1]]);
        }
    }
    fn block(&mut self) -> Result<[[i16; 2]; SPF]> {
        ensure!(
            self.error.is_none(),
            "ALSA capture failed: {}",
            self.error.as_deref().unwrap_or_default()
        );
        Ok(std::array::from_fn(|_| {
            self.frames.pop_front().unwrap_or_else(|| {
                self.silence += 1;
                [0; 2]
            })
        }))
    }
}
pub struct Capture {
    buffer: Arc<Mutex<Buffer>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Capture {
    pub fn start(device: &str) -> Result<Self> {
        ensure!(
            !device.trim().is_empty()
                && device.len() <= 256
                && !device.chars().any(char::is_control),
            "invalid ALSA device"
        );
        let buffer = Arc::new(Mutex::new(Buffer {
            frames: VecDeque::with_capacity(CAPACITY),
            ..Buffer::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = buffer.clone();
        let stopped = stop.clone();
        let device = device.to_owned();
        let worker = thread::spawn(move || {
            let result = (|| -> Result<()> {
                let pcm =
                    PCM::new(&device, Direction::Capture, true).context("open capture device")?;
                {
                    let hw = HwParams::any(&pcm)?;
                    hw.set_access(Access::RWInterleaved)?;
                    hw.set_channels(2)?;
                    hw.set_format(Format::s16())?;
                    hw.set_rate(44100, ValueOr::Nearest)?;
                    hw.set_period_size_near(SPF as i64, ValueOr::Nearest)?;
                    hw.set_buffer_size_near(4410)?;
                    pcm.hw_params(&hw)?;
                    let actual = pcm.hw_params_current()?;
                    ensure!(
                        actual.get_buffer_size()? <= 44100,
                        "ALSA buffer exceeds one second"
                    );
                    ensure!(actual.get_rate()? == 44100 && actual.get_channels()? == 2 && actual.get_format()? == Format::s16(), "capture requires 44100 Hz S16 stereo; use plughw:CARD=<id>,DEV=0 for ALSA conversion");
                }
                pcm.prepare()?;
                let io = pcm.io_i16()?;
                let mut samples = [0i16; SPF * 2];
                let mut last = Instant::now();
                while !stopped.load(Ordering::Acquire) {
                    match io.readi(&mut samples) {
                        Ok(n) if n > 0 => {
                            last = Instant::now();
                            shared.lock().unwrap().push(&samples[..n * 2]);
                        }
                        Err(e) if e.errno() == libc::EPIPE || e.errno() == libc::ESTRPIPE => {
                            // Re-prepare discards the interrupted ALSA epoch. Avoid
                            // snd_pcm_recover's potentially unbounded suspend retry.
                            // RTP and the sender clock continue unchanged.
                            pcm.prepare()?;
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) if e.errno() != libc::EAGAIN && e.errno() != libc::EINTR => {
                            return Err(e.into())
                        }
                        _ => thread::sleep(Duration::from_millis(2)),
                    }
                    ensure!(
                        last.elapsed() < Duration::from_secs(2),
                        "capture stalled for two seconds"
                    );
                }
                Ok(())
            })();
            if let Err(e) = result {
                shared.lock().unwrap().error = Some(format!("{e:#}"));
            }
        });
        let capture = Self {
            buffer,
            stop,
            worker: Some(worker),
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            {
                let b = capture.buffer.lock().unwrap();
                ensure!(b.error.is_none(), "capture startup failed: {:?}", b.error);
                if b.frames.len() >= PREFILL {
                    break;
                }
            }
            ensure!(Instant::now() < deadline, "capture startup timed out");
            thread::sleep(Duration::from_millis(5));
        }
        println!("Live capture: 44100 Hz S16 stereo; startup=96 ms; queue capacity=192 ms; underrun=silence; overflow=drop oldest");
        Ok(capture)
    }
    pub fn block(&mut self) -> Result<[[i16; 2]; SPF]> {
        self.buffer.lock().unwrap().block()
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let b = self.buffer.lock().unwrap();
        println!(
            "Live capture stopped: dropped_frames={} silence_frames={}; ALSA released",
            b.dropped, b.silence
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_joins_mock_capture_worker() {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let done = finished.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_millis(2));
            }
            done.store(true, Ordering::Release);
        });
        let capture = Capture {
            buffer: Arc::new(Mutex::new(Buffer::default())),
            stop,
            worker: Some(worker),
        };
        drop(capture);
        assert!(finished.load(Ordering::Acquire));
    }
    #[test]
    fn failed_open_joins_worker_and_releases_state() {
        let start = Instant::now();
        assert!(Capture::start("piair_nonexistent_test_pcm").is_err());
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(Capture::start("").is_err());
    }
    #[test]
    fn partial_stereo_reads_silence_and_bounded_overflow() {
        let mut b = Buffer::default();
        b.push(&[i16::MIN, i16::MAX, 123, -456]);
        let block = b.block().unwrap();
        assert_eq!(block[0], [i16::MIN, i16::MAX]);
        assert_eq!(block[1], [123, -456]);
        assert_eq!(b.silence, 350);
        b.push(&vec![7; (CAPACITY + 3) * 2]);
        assert_eq!(b.frames.len(), CAPACITY);
        assert_eq!(b.dropped, 3);
        b.error = Some("disconnected".into());
        assert!(b.block().is_err());
    }
}
