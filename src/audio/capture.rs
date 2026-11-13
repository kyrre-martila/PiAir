use super::{
    detector::{Detector, Meter},
    Settings,
};
use alsa::{
    device_name::HintIter,
    pcm::{Access, Format, HwParams, PCM},
    Direction, ValueOr,
};
use anyhow::{bail, Context, Result};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

fn normalize_s16(value: i16) -> f32 {
    f32::from(value) / 32768.0
}

fn normalize_s32(value: i32) -> f32 {
    (f64::from(value) / 2147483648.0) as f32
}

/// ALSA's own capture hints; these are candidates, not availability promises.
pub(crate) fn discover_devices() -> Result<Vec<(String, String)>> {
    let mut devices = Vec::new();
    for hint in HintIter::new(None, c"pcm")? {
        if hint.direction != Some(Direction::Playback) {
            if let Some(name) = hint.name {
                devices.push((name, hint.desc.unwrap_or_default().replace('\n', " ")));
            }
        }
    }
    devices.sort();
    devices.dedup();
    Ok(devices)
}
pub fn devices() -> Result<()> {
    println!("ALSA capture PCM hints (availability is checked only on capture):");
    for (name, description) in discover_devices()? {
        println!("{name:?}  {description:?}");
    }
    println!(
        "Use a stable hw:CARD=<USB card ID>,DEV=0 identifier; compare arecord -l and arecord -L."
    );
    Ok(())
}

struct Capture {
    pcm: PCM,
    format: Format,
    rate: u32,
    period: i64,
    buffer: i64,
}

impl Capture {
    fn open(settings: &Settings) -> Result<Self> {
        let pcm = PCM::new(&settings.device, Direction::Capture, true).context(
            "cannot open ALSA capture; check device, permissions and exclusive ownership",
        )?;
        let (format, rate, period, buffer) = {
            let hw = HwParams::any(&pcm)?;
            hw.set_access(Access::RWInterleaved)?;
            hw.set_channels(settings.channels)?;
            let format = [Format::s32(), Format::s16()]
                .into_iter()
                .find(|format| hw.test_format(*format).is_ok())
                .context("input must support native signed 32-bit or 16-bit PCM")?;
            hw.set_format(format)?;
            hw.set_rate_near(settings.sample_rate, ValueOr::Nearest)?;
            hw.set_period_size_near(1024, ValueOr::Nearest)?;
            hw.set_buffer_size_near(i64::from(settings.sample_rate / 5))?;
            pcm.hw_params(&hw)?;
            let actual = pcm.hw_params_current()?;
            if actual.get_channels()? != 2 || actual.get_access()? != Access::RWInterleaved {
                bail!("ALSA did not negotiate interleaved stereo");
            }
            let rate = actual.get_rate()?;
            if !(8_000..=192_000).contains(&rate) {
                bail!("negotiated rate {rate} is outside supported 8000..192000 Hz");
            }
            let buffer = actual.get_buffer_size()?;
            if buffer <= 0 || buffer > i64::from(rate) {
                bail!("negotiated ALSA buffer exceeds one second");
            }
            (
                actual.get_format()?,
                rate,
                actual.get_period_size()?,
                buffer,
            )
        };
        pcm.prepare()?;
        Ok(Self {
            pcm,
            format,
            rate,
            period,
            buffer,
        })
    }
}

/// One capture epoch. Any discontinuity returns an error; callers explicitly
/// reset intent and choose whether to retry. Never manufacture silent samples.
pub fn collect(
    settings: &Settings,
    deadline: Option<Instant>,
    running: &AtomicBool,
    mut meter: impl FnMut(Meter, &Detector) -> Result<()>,
) -> Result<()> {
    let capture = Capture::open(settings)?;
    let mut detector = Detector::new(settings, capture.rate)?;
    println!(
        "Input {:?}: {} Hz, 2 channels, {:?}, interleaved; period={} frames, ALSA buffer={} frames",
        settings.device, capture.rate, capture.format, capture.period, capture.buffer
    );
    println!("RMS window={} frames ({:.3} ms); thresholds start={:.1}, stop={:.1} dBFS; pre-roll={} frames / {} bytes (32 MiB ceiling)",
        settings.frames(capture.rate, settings.window_ms),
        settings.frames(capture.rate, settings.window_ms) as f64 * 1000.0 / f64::from(capture.rate),
        settings.start_threshold_dbfs, settings.stop_threshold_dbfs,
        detector.capacity(), detector.capacity() * std::mem::size_of::<super::detector::Frame>());
    // Allocate once. Short ALSA reads are processed using the returned frame count.
    let mut s16 = [0_i16; 2048];
    let mut s32 = [0_i32; 2048];
    let mut index = 0_u64;
    let mut last_data = Instant::now();
    let mut first_data = true;
    while running.load(Ordering::Relaxed) && deadline.is_none_or(|end| Instant::now() < end) {
        let read = if capture.format == Format::s16() {
            capture.pcm.io_i16()?.readi(&mut s16)
        } else if capture.format == Format::s32() {
            capture.pcm.io_i32()?.readi(&mut s32)
        } else {
            bail!("unsupported negotiated sample format {:?}", capture.format);
        };
        match read {
            Ok(frames) if frames > 0 => {
                // Long scheduler/device gaps cannot bridge qualification timers.
                if !first_data && last_data.elapsed() > Duration::from_secs(1) {
                    bail!("capture stalled or scheduling gap exceeded one second");
                }
                first_data = false;
                last_data = Instant::now();
                for frame in 0..frames {
                    let pcm = if capture.format == Format::s16() {
                        [
                            normalize_s16(s16[frame * 2]),
                            normalize_s16(s16[frame * 2 + 1]),
                        ]
                    } else {
                        [
                            normalize_s32(s32[frame * 2]),
                            normalize_s32(s32[frame * 2 + 1]),
                        ]
                    };
                    if let Some(value) = detector.push(index, pcm)? {
                        meter(value, &detector)?;
                    }
                    index += 1;
                }
            }
            Err(error) if error.errno() != libc::EAGAIN => {
                let event = detector.reset();
                return Err(error).with_context(|| format!("capture fault ({event:?}); overrun/disconnection invalidates activity and pre-roll"));
            }
            _ => {
                if last_data.elapsed() > Duration::from_secs(2) {
                    detector.reset();
                    bail!("capture timed out without samples; input is unavailable, not silent");
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    if first_data && running.load(Ordering::Relaxed) {
        bail!("monitoring duration ended without captured samples; input is unavailable");
    }
    Ok(())
}

pub fn monitor(settings: &Settings, duration: Option<u64>, running: &AtomicBool) -> Result<()> {
    let deadline = duration.map(|s| Instant::now() + Duration::from_secs(s));
    let mut errors = 0_u64;
    let mut clips = 0_u64;
    let mut last_display = Instant::now();
    let mut displayed = false;
    let mut last_state = None;
    let mut noise = std::collections::VecDeque::with_capacity(250);
    let mut levels = Vec::with_capacity(250);
    println!("STREAMING = confirmed activity intent; AirPlay session = absent. Ctrl+C exits.");
    println!("If the stopped source stays at/above stop, check noise/gain and recalibrate; thresholds never adapt automatically.");
    loop {
        let result = collect(settings, deadline, running, |m, d| {
            clips = clips.saturating_add(m.clipped_samples);
            if noise.len() == 250 {
                noise.pop_front();
            }
            noise.push_back(m.level);
            if !displayed
                || last_display.elapsed() >= Duration::from_millis(200)
                || m.event.is_some()
                || last_state != Some(m.state)
            {
                levels.clear();
                levels.extend(noise.iter().copied());
                levels.sort_unstable_by(f64::total_cmp);
                println!("frame={} L={:.1} R={:.1} combined={:.1} dBFS start={:.1} stop={:.1} state={} qualification={}/{} frames clips={} capture_errors={} pre-roll={}/{} overwritten={} recent_median={:.1} p95={:.1} event={:?}",
                    m.end_frame, m.dbfs[0], m.dbfs[1], m.level,
                    settings.start_threshold_dbfs, settings.stop_threshold_dbfs,
                    m.state, m.qualified_frames, m.required_frames, clips, errors,
                    d.pre_roll().count(), d.capacity(), d.overwritten_frames,
                    levels[levels.len().div_ceil(2)-1], levels[(levels.len()*95).div_ceil(100)-1], m.event);
                if m.level >= settings.stop_threshold_dbfs
                    && m.level < settings.start_threshold_dbfs
                {
                    println!("Level is in hysteresis band; if the source is idle, check gain/noise and recalibrate.");
                }
                displayed = true;
                last_state = Some(m.state);
                last_display = Instant::now();
            }
            Ok(())
        });
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                errors += 1;
                noise.clear();
                last_state = None;
                eprintln!(
                    "capture_errors={errors} event=Reset state=IDLE input=UNAVAILABLE: {error:#}"
                );
                if !running.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if errors >= 4 {
                    return Err(error).context(
                        "capture recovery exhausted (three retries); check input and rerun monitor",
                    );
                }
                // Bounded backoff, interruptible within 10 ms.
                let until = Instant::now() + Duration::from_millis(250 * errors);
                while Instant::now() < until {
                    if !running.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    if deadline.is_some_and(|end| Instant::now() >= end) {
                        return Err(error)
                            .context("monitor duration expired during capture recovery");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_pcm_full_scale_and_half_scale() {
        for (value, expected) in [
            (-32768_i16, -1.0),
            (-16384, -0.5),
            (0, 0.0),
            (16384, 0.5),
            (32767, 0.9999695),
        ] {
            assert!((normalize_s16(value) - expected).abs() < 0.0000001);
        }
        assert_eq!(normalize_s32(i32::MIN), -1.0);
        assert_eq!(normalize_s32(i32::MAX), 1.0);
        assert_eq!(normalize_s32(0), 0.0);
        assert_eq!(normalize_s32(1 << 30), 0.5);
        assert_eq!(normalize_s32(-(1 << 30)), -0.5);
    }
}
