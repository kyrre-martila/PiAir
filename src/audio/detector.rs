//! Pure sample-clock engine: no ALSA, clocks, threads, sockets, or callbacks.
use super::Settings;
use anyhow::{bail, Result};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Starting,
    Streaming,
    Stopping,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Idle => "IDLE",
            Self::Starting => "STARTING",
            Self::Streaming => "STREAMING",
            Self::Stopping => "STOPPING",
        })
    }
}

/// Controller-facing intent. Reset is a fault, never a qualified silent stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Started,
    Stopped,
    Reset,
}

#[derive(Clone, Copy, Debug)]
pub struct Meter {
    pub end_frame: u64,
    pub dbfs: [f64; 2],
    pub level: f64,
    pub clipped_samples: u64,
    pub state: State,
    pub qualified_frames: u64,
    pub required_frames: u64,
    pub event: Option<Event>,
}

/// Timestamp is an absolute frame index at the negotiated rate, within an epoch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub index: u64,
    pub pcm: [f32; 2],
}

pub struct Detector {
    settings: Settings,
    window_frames: u64,
    start_frames: u64,
    stop_frames: u64,
    ring_limit: usize,
    ring: VecDeque<Frame>,
    pub overwritten_frames: u64,
    next: Option<u64>,
    state: State,
    qualified: u64,
    count: u64,
    sum: [f64; 2],
    clips: u64,
}

impl Detector {
    pub fn new(settings: &Settings, rate: u32) -> Result<Self> {
        settings.validate()?;
        if !(8_000..=192_000).contains(&rate) {
            bail!("unsupported negotiated sample rate");
        }
        let ring_limit = settings.frames(rate, settings.pre_roll_ms) as usize;
        // Frame occupies 16 bytes: maximum 1,920,000 frames = 30.72 MB.
        if ring_limit * std::mem::size_of::<Frame>() > 32 * 1024 * 1024 {
            bail!("pre-roll exceeds 32 MiB");
        }
        Ok(Self {
            settings: settings.clone(),
            window_frames: settings.frames(rate, settings.window_ms),
            start_frames: settings.frames(rate, settings.start_duration_ms),
            stop_frames: settings.frames(rate, settings.stop_duration_ms),
            ring_limit,
            ring: VecDeque::with_capacity(ring_limit),
            overwritten_frames: 0,
            next: None,
            state: State::Idle,
            qualified: 0,
            count: 0,
            sum: [0.0; 2],
            clips: 0,
        })
    }

    /// Ordered borrowed pre-roll; consumer must track last frame to avoid replay.
    pub fn pre_roll(&self) -> impl Iterator<Item = &Frame> {
        self.ring.iter()
    }
    pub fn capacity(&self) -> usize {
        self.ring_limit
    }

    pub fn reset(&mut self) -> Event {
        self.next = None;
        self.state = State::Idle;
        self.qualified = 0;
        self.count = 0;
        self.sum = [0.0; 2];
        self.clips = 0;
        self.ring.clear();
        self.overwritten_frames = 0;
        Event::Reset
    }

    /// One stereo frame, no allocation. Caller must propagate errors as Reset,
    /// and must not replace bad data with zeros. Partial windows are discarded.
    pub fn push(&mut self, index: u64, pcm: [f32; 2]) -> Result<Option<Meter>> {
        if self.next.is_some_and(|next| next != index)
            || index == u64::MAX
            || pcm.iter().any(|v| !v.is_finite() || v.abs() > 1.0)
        {
            self.reset();
            bail!("invalid PCM or discontinuous sample timestamp; detector reset");
        }
        self.next = Some(index + 1);
        if self.ring.len() == self.ring_limit {
            self.ring.pop_front();
            self.overwritten_frames = self.overwritten_frames.saturating_add(1);
        }
        self.ring.push_back(Frame { index, pcm });
        for (channel, value) in pcm.into_iter().enumerate() {
            self.sum[channel] += f64::from(value).powi(2);
            // Includes both asymmetric integer full-scale endpoints.
            if value.abs() >= 1.0 - 1.0 / 32768.0 {
                self.clips += 1;
            }
        }
        self.count += 1;
        if self.count != self.window_frames {
            return Ok(None);
        }
        let dbfs = self.sum.map(|sum| {
            if sum == 0.0 {
                f64::NEG_INFINITY
            } else {
                10.0 * (sum / self.count as f64).log10()
            }
        });
        let level = dbfs[0].max(dbfs[1]);
        let event = self.advance(level);
        let required_frames = match self.state {
            State::Starting => self.start_frames,
            State::Stopping => self.stop_frames,
            _ => 0,
        };
        let meter = Meter {
            end_frame: index + 1,
            dbfs,
            level,
            clipped_samples: self.clips,
            state: self.state,
            qualified_frames: self.qualified,
            required_frames,
            event,
        };
        self.count = 0;
        self.sum = [0.0; 2];
        self.clips = 0;
        Ok(Some(meter))
    }

    fn advance(&mut self, level: f64) -> Option<Event> {
        let start = level >= self.settings.start_threshold_dbfs;
        let stop = level <= self.settings.stop_threshold_dbfs;
        match self.state {
            State::Idle | State::Starting if start => {
                self.state = State::Starting;
                self.qualified += self.window_frames;
                if self.qualified >= self.start_frames {
                    self.state = State::Streaming;
                    self.qualified = 0;
                    return Some(Event::Started);
                }
            }
            State::Idle | State::Starting => {
                self.state = State::Idle;
                self.qualified = 0;
            }
            State::Streaming | State::Stopping if stop => {
                self.state = State::Stopping;
                self.qualified += self.window_frames;
                if self.qualified >= self.stop_frames {
                    self.state = State::Idle;
                    self.qualified = 0;
                    return Some(Event::Stopped);
                }
            }
            State::Streaming | State::Stopping => {
                self.state = State::Streaming;
                self.qualified = 0;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> Detector {
        Detector::new(
            &Settings {
                start_duration_ms: 40,
                stop_duration_ms: 60,
                pre_roll_ms: 60,
                ..Settings::default()
            },
            8000,
        )
        .unwrap()
    }
    fn window(d: &mut Detector, pcm: [f32; 2]) -> Meter {
        let mut result = None;
        for _ in 0..d.window_frames {
            result = d.push(d.next.unwrap_or(0), pcm).unwrap();
        }
        result.unwrap()
    }

    #[test]
    fn silence_known_rms_stereo_and_sine() {
        let mut d = detector();
        assert_eq!(window(&mut d, [0.0; 2]).dbfs, [f64::NEG_INFINITY; 2]);
        let m = window(&mut d, [0.5, -0.5]);
        assert!((m.level + 6.0206).abs() < 0.0001);
        assert_eq!(m.dbfs[0], m.dbfs[1]);
        d.reset();
        let mut m = None;
        for i in 0..160 {
            let x = (2.0 * std::f32::consts::PI * i as f32 / 16.0).sin() * 0.5;
            m = d.push(i, [0.0, x]).unwrap();
        }
        let m = m.unwrap();
        assert!((m.level + 9.0309).abs() < 0.001);
        assert_eq!(m.dbfs[0], f64::NEG_INFINITY);
        assert_eq!(window(&mut d, [0.0, 0.5]).event, Some(Event::Started));
    }

    #[test]
    fn spikes_pauses_noise_and_timing() {
        let mut d = detector();
        assert_eq!(window(&mut d, [0.1, 0.0]).state, State::Starting);
        assert_eq!(window(&mut d, [0.0; 2]).state, State::Idle);
        window(&mut d, [0.0, 0.1]);
        assert_eq!(window(&mut d, [0.0, 0.1]).event, Some(Event::Started));
        assert_eq!(window(&mut d, [0.0; 2]).state, State::Stopping);
        // Hysteresis band cancels stop, but cannot initiate a start.
        assert_eq!(window(&mut d, [0.003; 2]).state, State::Streaming);
        for _ in 0..2 {
            assert_eq!(window(&mut d, [0.0001; 2]).state, State::Stopping);
        }
        assert_eq!(window(&mut d, [0.0001; 2]).event, Some(Event::Stopped));
        for _ in 0..100 {
            assert_eq!(window(&mut d, [0.0001; 2]).state, State::Idle);
        }
        assert_eq!(window(&mut d, [0.003; 2]).state, State::Idle);
    }

    #[test]
    fn exact_thresholds_and_hysteresis() {
        let mut d = detector();
        // Exact comparisons are tested directly, avoiding f32 normalization rounding.
        assert_eq!(d.advance(-45.0), None);
        assert_eq!(d.state, State::Starting);
        d.advance(-45.0001);
        assert_eq!(d.state, State::Idle);
        d.advance(-45.0);
        assert_eq!(d.advance(-45.0), Some(Event::Started));
        d.advance(-55.0);
        assert_eq!(d.state, State::Stopping);
        d.advance(-54.9999);
        assert_eq!(d.state, State::Streaming);
        d.advance(-55.0);
        d.advance(-55.0);
        assert_eq!(d.advance(-55.0), Some(Event::Stopped));
    }

    #[test]
    fn ring_order_overflow_and_fault_recovery() {
        let mut d = detector();
        for i in 0..560 {
            d.push(i, [0.0; 2]).unwrap();
        }
        assert_eq!(d.pre_roll().count(), 480);
        assert_eq!(d.capacity(), 480);
        assert_eq!(d.pre_roll().next().unwrap().index, 80);
        assert_eq!(d.pre_roll().last().unwrap().index, 559);
        assert_eq!(d.overwritten_frames, 80);
        assert!(d.push(561, [0.0; 2]).is_err());
        assert_eq!(d.pre_roll().count(), 0);
        for bad in [f32::NAN, f32::INFINITY, 1.1] {
            assert!(d.push(0, [bad, 0.0]).is_err());
        }
        assert_eq!(window(&mut d, [1.0, -1.0]).clipped_samples, 320);
        assert_eq!(d.state, State::Starting);
        assert_eq!(d.reset(), Event::Reset);
        assert_eq!(window(&mut d, [0.1; 2]).state, State::Starting);
        window(&mut d, [0.1; 2]);
        assert_eq!(d.state, State::Streaming);
        d.reset();
        assert_eq!(window(&mut d, [0.0; 2]).state, State::Idle);
        window(&mut d, [0.1; 2]);
        window(&mut d, [0.1; 2]);
        window(&mut d, [0.0; 2]);
        assert_eq!(d.state, State::Stopping);
        d.reset();
        assert_eq!(window(&mut d, [0.1; 2]).state, State::Starting);
    }

    #[test]
    fn fractional_duration_rounds_up_to_complete_windows() {
        let mut d = Detector::new(
            &Settings {
                start_duration_ms: 21,
                ..Settings::default()
            },
            44100,
        )
        .unwrap();
        assert_eq!(window(&mut d, [0.1; 2]).state, State::Starting);
        assert_eq!(window(&mut d, [0.1; 2]).state, State::Streaming);
    }
}
