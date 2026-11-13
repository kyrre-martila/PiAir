use super::{capture, detector::Meter};
use crate::config::{Config, Loaded};
use anyhow::{bail, Result};
use std::{
    io::{self, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use toml_edit::{DocumentMut, Item, Table, Value};

#[derive(Debug)]
pub struct Proposal {
    pub median: f64,
    pub p95: f64,
    pub start: f64,
    pub stop: f64,
}

/// Nearest-rank statistics of the louder channel, no averaging across channels.
pub fn propose(levels: &[f64], clipped: u64) -> Result<Proposal> {
    if levels.len() < 50
        || clipped != 0
        || levels
            .iter()
            .any(|v| !v.is_finite() || *v > 0.0 || *v < -120.0)
    {
        bail!(
            "invalid/clipped or digital-silence calibration; check analog input, gain and source"
        );
    }
    let mut values = levels.to_vec();
    values.sort_by(f64::total_cmp);
    let percentile = |p: usize| values[(values.len() * p).div_ceil(100).saturating_sub(1)];
    let median = percentile(50);
    let p95 = percentile(95);
    // Reject spread, transients and apparently active input. These are conservative
    // heuristics; constant quiet music cannot be distinguished from stationary noise.
    if p95 - percentile(5) > 6.0 || values[values.len() - 1] - median > 12.0 || p95 > -25.0 {
        bail!("unstable or apparently active input; stop music, check gain/cabling, and repeat");
    }
    let stop = p95 + 6.0;
    let start = p95 + 12.0;
    Ok(Proposal {
        median,
        p95,
        start,
        stop,
    })
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} Type yes to confirm: ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim() == "yes")
}

/// Modify only two TOML values, retaining their decorations and all other items.
fn render(original: &str, proposal: &Proposal) -> Result<String> {
    Config::parse(original)?;
    let mut doc = original.parse::<DocumentMut>()?;
    if doc.get("audio").is_none() {
        doc["audio"] = Item::Table(Table::new());
    }
    for (key, number) in [
        ("start_threshold_dbfs", proposal.start),
        ("stop_threshold_dbfs", proposal.stop),
    ] {
        let decor = doc["audio"]
            .get(key)
            .and_then(Item::as_value)
            .map(|v| v.decor().clone());
        let mut value = Value::from(number);
        if let Some(decor) = decor {
            *value.decor_mut() = decor;
        }
        doc["audio"][key] = Item::Value(value);
    }
    let text = doc.to_string();
    Config::parse(&text)?;
    Ok(text)
}

#[cfg(test)]
fn save(path: &std::path::Path, expected: Option<&str>, rendered: &str) -> Result<()> {
    let snapshot = crate::allowlist::snapshot(path)?;
    if snapshot.as_ref().map(|s| s.text.as_str()) != expected {
        bail!("configuration changed during calibration; refusing to overwrite");
    }
    crate::allowlist::save(path, snapshot.as_ref(), rendered)
}

pub fn run(loaded: &Loaded, duration: Option<u64>, running: &AtomicBool) -> Result<()> {
    println!("Calibration updates [audio] monitor thresholds only; automatic playback uses [playback] and is unchanged.");
    let settings = &loaded.config.audio;
    let original = crate::allowlist::snapshot(&loaded.path)?;
    if !confirm("Stop the music source and keep hardware gain at its normal setting. Is the source stopped?")? {
        println!("Calibration cancelled; nothing saved.");
        return Ok(());
    }
    if !running.load(Ordering::Relaxed) {
        return Ok(());
    }
    let duration = duration.unwrap_or(settings.calibration_duration_secs);
    let limit = (duration * 1000 / settings.window_ms + 2) as usize;
    let mut levels = Vec::with_capacity(limit);
    let mut clips = 0;
    let mut first_frame = None;
    let mut last_frame = 0;
    println!("Measuring idle noise for {duration} seconds; capture faults reject the measurement.");
    capture::collect(
        settings,
        Some(Instant::now() + Duration::from_secs(duration)),
        running,
        |m: Meter, _| {
            if levels.len() == limit {
                bail!("calibration window bound exceeded");
            }
            first_frame.get_or_insert(m.end_frame);
            last_frame = m.end_frame;
            levels.push(m.level);
            clips += m.clipped_samples;
            Ok(())
        },
    )?;
    if !running.load(Ordering::Relaxed) {
        println!("Calibration interrupted; nothing saved.");
        return Ok(());
    }
    // At least 90% of the requested capture duration; reject slow or empty devices.
    // Window count avoids assuming the preferred rate equals the negotiated rate.
    if levels.len() as u64 * settings.window_ms < duration * 900
        || first_frame.is_none()
        || last_frame == 0
    {
        bail!("too little valid audio for calibration; nothing saved");
    }
    let proposal = propose(&levels, clips)?;
    println!(
        "Noise median={:.1} dBFS, p95={:.1} dBFS ({} windows).",
        proposal.median,
        proposal.p95,
        levels.len()
    );
    println!(
        "Proposed start={:.1} dBFS (p95 + 12 dB), stop={:.1} dBFS (p95 + 6 dB).",
        proposal.start, proposal.stop
    );
    println!(
        "Existing start={:.1}, stop={:.1} dBFS. Destination: {}",
        settings.start_threshold_dbfs,
        settings.stop_threshold_dbfs,
        loaded.path.display()
    );
    println!("Before accepting calibration, test your quietest intended music: it must exceed start for the start duration, and quiet passages must not satisfy stop for the stop duration. Values remain provisional until tested. Stationary music/noise cannot always be distinguished.");
    let rendered = render(original.as_ref().map_or("", |s| s.text.as_str()), &proposal)?;
    if confirm("Save these two thresholds to the displayed configuration file?")?
        && running.load(Ordering::Relaxed)
    {
        crate::allowlist::save(&loaded.path, original.as_ref(), &rendered)?;
        println!("Saved. Run audio monitor with quiet music and a long stopped-source recording.");
    } else {
        println!("Calibration not saved.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn statistics_margins_and_rejections() {
        let mut levels = vec![-70.0; 90];
        levels.extend([-68.0; 10]);
        let p = propose(&levels, 0).unwrap();
        assert_eq!(p.median, -70.0);
        assert_eq!(p.p95, -68.0);
        assert_eq!(p.start, -56.0);
        assert_eq!(p.stop, -62.0);
        assert!(propose(&levels, 1).is_err());
        for bad in [f64::NEG_INFINITY, f64::NAN, 1.0, -121.0, -20.0] {
            assert!(propose(&[bad; 100], 0).is_err());
        }
        assert!(propose(&[-70.0; 49], 0).is_err());
        levels[0] = -40.0;
        assert!(propose(&levels, 0).is_err());
        let unstable: Vec<_> = (0..100)
            .map(|i| if i % 2 == 0 { -70.0 } else { -60.0 })
            .collect();
        assert!(propose(&unstable, 0).is_err());
    }

    #[test]
    fn save_preserves_comments_policy_and_other_values() {
        let original = "# keep me\nreceiver_allowlist = ['device:aabbccddeeff'] # approval\n[audio]\ndevice = 'hw:CARD=USB,DEV=0' # card\nstart_threshold_dbfs = -45.0 # start comment\n";
        let p = propose(&[-70.0; 100], 0).unwrap();
        let text = render(original, &p).unwrap();
        for retained in [
            "# keep me",
            "# approval",
            "# card",
            "# start comment",
            "device = 'hw:CARD=USB,DEV=0'",
        ] {
            assert!(text.contains(retained), "{text}");
        }
        let config = Config::parse(&text).unwrap();
        assert!(config.allows("device:aabbccddeeff"));
        assert_eq!(config.audio.start_threshold_dbfs, -58.0);
        Config::parse(&render("", &p).unwrap()).unwrap();
        Config::parse(&render("audio = { device = 'default' }", &p).unwrap()).unwrap();
    }

    #[test]
    fn save_refuses_changed_file_and_creates_only_explicit_values() {
        let dir = std::env::temp_dir().join(format!("piair-calibration-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("config.toml");
        let p = propose(&[-70.0; 100], 0).unwrap();
        let text = render("", &p).unwrap();
        save(&path, None, &text).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(save(&path, None, &text).is_err());
        fs::write(&path, "# concurrent edit\n").unwrap();
        assert!(save(&path, Some(&text), &text).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "# concurrent edit\n");
        let updated = render("# concurrent edit\n", &p).unwrap();
        save(&path, Some("# concurrent edit\n"), &updated).unwrap();
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("# concurrent edit"));
        fs::remove_dir_all(dir).unwrap();
    }
}
