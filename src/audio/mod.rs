//! Capture and activity intent only. This module has no network dependency.
mod calibration;
mod capture;
pub mod detector;

use crate::{cli::AudioCommand, config::Loaded};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub device: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub window_ms: u64,
    pub start_threshold_dbfs: f64,
    pub stop_threshold_dbfs: f64,
    pub start_duration_ms: u64,
    pub stop_duration_ms: u64,
    pub pre_roll_ms: u64,
    pub calibration_duration_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device: "default".into(),
            sample_rate: 48_000,
            channels: 2,
            window_ms: 20,
            start_threshold_dbfs: -45.0,
            stop_threshold_dbfs: -55.0,
            start_duration_ms: 200,
            stop_duration_ms: 5_000,
            pre_roll_ms: 1_000,
            calibration_duration_secs: 30,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if self.device.trim().is_empty()
            || self.device.len() > 256
            || self.device.chars().any(char::is_control)
        {
            bail!("audio.device must be a nonempty ALSA identifier, at most 256 bytes");
        }
        if !(8_000..=192_000).contains(&self.sample_rate) || self.channels != 2 {
            bail!("audio requires sample_rate 8000..192000 and channels = 2 (no downmix)");
        }
        if !(5..=100).contains(&self.window_ms)
            || !(1..=10_000).contains(&self.start_duration_ms)
            || !(1..=120_000).contains(&self.stop_duration_ms)
            || !(1..=10_000).contains(&self.pre_roll_ms)
            || !(5..=300).contains(&self.calibration_duration_secs)
        {
            bail!("audio durations out of bounds: window 5..100 ms, start 1..10000 ms, stop 1..120000 ms, pre-roll 1..10000 ms, calibration 5..300 s");
        }
        if !self.start_threshold_dbfs.is_finite()
            || !self.stop_threshold_dbfs.is_finite()
            || self.stop_threshold_dbfs < -120.0
            || self.stop_threshold_dbfs >= self.start_threshold_dbfs
            || self.start_threshold_dbfs > 0.0
        {
            bail!("audio thresholds must be finite: -120 <= stop < start <= 0 dBFS");
        }
        if self.pre_roll_ms < self.start_duration_ms + self.window_ms {
            bail!("audio.pre_roll_ms must cover start_duration_ms plus one RMS window");
        }
        Ok(())
    }

    pub fn frames(&self, rate: u32, ms: u64) -> u64 {
        (u64::from(rate) * ms).div_ceil(1000)
    }
}

pub fn run(command: AudioCommand, loaded: &Loaded) -> Result<()> {
    if matches!(command, AudioCommand::Devices) {
        return capture::devices();
    }
    let running = Arc::new(AtomicBool::new(true));
    let signal = running.clone();
    ctrlc::set_handler(move || signal.store(false, Ordering::Relaxed))?;
    match command {
        AudioCommand::Devices => unreachable!(),
        AudioCommand::Monitor { duration_secs } => {
            capture::monitor(&loaded.config.audio, duration_secs, &running)
        }
        AudioCommand::Calibrate { duration_secs } => {
            calibration::run(loaded, duration_secs, &running)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_audio_validation() {
        Settings::default().validate().unwrap();
        for setting in [
            "channels = 1",
            "sample_rate = 0",
            "window_ms = 0",
            "start_threshold_dbfs = nan",
            "stop_threshold_dbfs = inf",
            "stop_threshold_dbfs = -45",
            "start_threshold_dbfs = 1",
            "stop_threshold_dbfs = -121",
            "start_duration_ms = 0",
            "stop_duration_ms = 120001",
            "pre_roll_ms = 10001",
            "pre_roll_ms = 100",
            "calibration_duration_secs = 301",
            "device = ''",
            "unknown = 1",
        ] {
            assert!(
                crate::config::Config::parse(&format!("[audio]\n{setting}")).is_err(),
                "{setting}"
            );
        }
    }
}
