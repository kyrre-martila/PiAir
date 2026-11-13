//! PCM-only automatic mode hysteresis, independent of sessions and RTP clocks.
use anyhow::{ensure, Result};

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub threshold_dbfs: f64,
    pub start_ms: u64,
    pub stop_ms: u64,
}
impl Settings {
    pub fn validate(self) -> Result<Self> {
        ensure!(
            self.threshold_dbfs.is_finite() && (-120.0..=0.0).contains(&self.threshold_dbfs),
            "threshold must be -120..=0 dBFS"
        );
        ensure!(
            (20..=9000).contains(&self.start_ms),
            "start delay must be 20..=9000 ms"
        );
        ensure!(
            (20..=120000).contains(&self.stop_ms),
            "silence timeout must be 20..=120000 ms"
        );
        Ok(self)
    }
}

pub(super) struct Activity {
    settings: Settings,
    pub level_dbfs: f64,
    threshold_power: f64,
    sum: [f64; 2],
    count: u64,
    loud: u64,
    quiet: u64,
    pub active: bool,
    pub generation: u64,
}
impl Activity {
    pub fn new(settings: Settings) -> Self {
        Self {
            threshold_power: 10f64.powf(settings.threshold_dbfs / 10.0),
            settings,
            level_dbfs: f64::NEG_INFINITY,
            sum: [0.0; 2],
            count: 0,
            loud: 0,
            quiet: 0,
            active: false,
            generation: 0,
        }
    }
    pub fn meaningful(&self) -> bool {
        self.active && self.loud >= self.settings.start_ms
    }
    pub fn retained_start_frames(&self) -> usize {
        ((self.settings.start_ms + 120) * 44100 / 1000) as usize
    }
    pub fn reset(&mut self) {
        self.sum = [0.0; 2];
        self.level_dbfs = f64::NEG_INFINITY;
        self.count = 0;
        self.loud = 0;
        self.quiet = 0;
        self.active = false;
    }
    pub fn push(&mut self, pcm: [i16; 2]) {
        for (sum, sample) in self.sum.iter_mut().zip(pcm) {
            *sum += (f64::from(sample) / 32768.0).powi(2);
        }
        self.count += 1;
        // 20 ms RMS, strongest channel. No per-frame log or allocation.
        if self.count != 882 {
            return;
        }
        let power = self.sum.into_iter().fold(0.0, f64::max) / 882.0;
        self.level_dbfs = 10.0 * power.log10();
        let meaningful = power >= self.threshold_power;
        self.count = 0;
        self.sum = [0.0; 2];
        if meaningful {
            self.quiet = 0;
            self.loud += 20;
            if !self.active && self.loud >= self.settings.start_ms {
                self.active = true;
                self.generation += 1;
            }
        } else {
            self.loud = 0;
            self.quiet += 20;
            if self.active && self.quiet >= self.settings.stop_ms {
                self.active = false;
            }
        }
        self.loud = self.loud.min(self.settings.start_ms);
        self.quiet = self.quiet.min(self.settings.stop_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn meter() -> Activity {
        Activity::new(Settings {
            threshold_dbfs: -45.0,
            start_ms: 1000,
            stop_ms: 30000,
        })
    }
    fn feed(a: &mut Activity, ms: u64, sample: i16) {
        for _ in 0..ms * 44100 / 1000 {
            a.push([sample, -sample]);
        }
    }
    #[test]
    fn silence_noise_clicks_and_sustained_signal() {
        let mut a = meter();
        feed(&mut a, 32000, 0);
        feed(&mut a, 32000, 30);
        feed(&mut a, 20, 20000);
        feed(&mut a, 1000, 0);
        assert!(!a.active);
        feed(&mut a, 980, 3000);
        assert!(!a.active);
        feed(&mut a, 20, 3000);
        assert!(a.active);
    }
    #[test]
    fn invalid_settings_are_rejected_and_one_channel_can_start() {
        for threshold_dbfs in [f64::NAN, f64::INFINITY, -121.0, 0.1] {
            assert!(Settings {
                threshold_dbfs,
                start_ms: 1000,
                stop_ms: 30000
            }
            .validate()
            .is_err());
        }
        let mut a = meter();
        for _ in 0..44100 {
            a.push([0, 3000]);
        }
        assert!(a.active);
        assert!(a.meaningful());
        feed(&mut a, 20, 0);
        feed(&mut a, 20, 20000);
        assert!(a.active); // The playing session survives the short gap.
        assert!(!a.meaningful()); // A new connection still needs sustained audio.
        feed(&mut a, 980, 3000);
        assert!(a.meaningful());
        a.reset();
        feed(&mut a, 20, 20000); // A click cannot reuse a failed session qualification.
        feed(&mut a, 1000, 0);
        assert!(!a.active);
    }
    #[test]
    fn gaps_reset_stop_and_restart() {
        let mut a = meter();
        feed(&mut a, 1000, 3000);
        feed(&mut a, 10000, 0);
        assert!(a.active);
        feed(&mut a, 20, 3000);
        feed(&mut a, 29980, 0);
        assert!(a.active);
        feed(&mut a, 20, 0);
        assert!(!a.active);
        feed(&mut a, 1000, 3000);
        assert!(a.active);
        assert_eq!(a.generation, 2);
    }

    #[test]
    fn setup_suggestions_reject_quiet_pcm_noise_and_qualify_sustained_music() {
        use crate::receiver::setup::suggested_threshold;

        // Collect 25 real detector windows, just as setup collects RMS levels.
        let levels = |digital_silence: bool, music: bool| {
            let mut detector = meter();
            let mut levels = Vec::new();
            let mut random = 1u32;
            for frame in 0..25 * 882 {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let sample = if music {
                    // 100 Hz at approximately -30 dBFS RMS.
                    (1465.0 * (std::f64::consts::TAU * frame as f64 / 441.0).sin()) as i16
                } else if digital_silence {
                    0
                } else {
                    // Quiet broadband input noise, approximately -50 dBFS RMS.
                    ((random >> 16) % 361) as i16 - 180
                };
                detector.push([sample, -sample]);
                if (frame + 1) % 882 == 0 {
                    levels.push(detector.level_dbfs);
                }
            }
            levels
        };
        let music = levels(false, true);
        for digital_silence in [true, false] {
            let threshold = suggested_threshold(&levels(digital_silence, false), &music).unwrap();
            let mut detector = Activity::new(Settings {
                threshold_dbfs: threshold,
                ..meter().settings
            });
            let mut random = 1u32;
            // Realistic quiet background must not start playback even when setup
            // measured digital zeros, and even after the full silence timeout.
            for _ in 0..32 * 44100 {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = ((random >> 16) % 361) as i16 - 180;
                detector.push([noise, -noise]);
            }
            assert!(!detector.active);
            assert_eq!(detector.generation, 0);
            for frame in 0..44100 {
                let sample = (1465.0 * (std::f64::consts::TAU * frame as f64 / 441.0).sin()) as i16;
                detector.push([sample, -sample]);
                if frame + 1 == 980 * 44100 / 1000 {
                    assert!(!detector.active);
                }
            }
            assert!(detector.active);
            assert!(detector.meaningful());
            assert_eq!(detector.generation, 1);
        }
    }
}
