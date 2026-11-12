use super::*;

pub(crate) struct Mock {
    pub closed: Arc<AtomicBool>,
    pub reads: usize,
    pub fail_after: Option<usize>,
    pub pending: bool,
    pub failure: Option<Arc<AtomicBool>>,
    pub gate: Option<Arc<AtomicBool>>,
}
impl Source for Mock {
    fn read(&mut self, samples: &mut [i16]) -> Result<Read> {
        ensure!(
            !self
                .failure
                .as_ref()
                .is_some_and(|f| f.load(Ordering::Acquire)),
            "mock device disconnected"
        );
        ensure!(
            self.fail_after != Some(self.reads),
            "mock device disconnected"
        );
        self.reads += 1;
        if self.pending
            || self
                .gate
                .as_ref()
                .is_some_and(|g| !g.load(Ordering::Acquire))
        {
            return Ok(Read::Pending);
        }
        samples.fill(123);
        Ok(Read::Frames(SPF))
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}
pub(crate) fn mock(closed: Arc<AtomicBool>, pending: bool, fail_after: Option<usize>) -> Capture {
    Capture::spawn(move || {
        Ok(Mock {
            closed,
            reads: 0,
            fail_after,
            pending,
            failure: None,
            gate: None,
        })
    })
    .unwrap()
}
#[test]
fn startup_failure_cancel_network_and_timeout_join_workers() {
    for mode in ["cancel", "network", "timeout", "device"] {
        let closed = Arc::new(AtomicBool::new(false));
        let capture = mock(
            closed.clone(),
            mode != "device",
            (mode == "device").then_some(0),
        );
        let cancel = AtomicBool::new(mode == "cancel");
        let start = Instant::now();
        let deadline = start
            + if mode == "timeout" {
                Duration::ZERO
            } else {
                Duration::from_secs(2)
            };
        let result = capture.wait_ready_until(&cancel, deadline, || {
            ensure!(mode != "network", "mock network failure");
            Ok(())
        });
        assert!(result.is_err(), "{mode}");
        drop(capture);
        assert!(closed.load(Ordering::Acquire), "{mode}");
        assert!(start.elapsed() < Duration::from_secs(1), "{mode}");
    }
    let capture = Capture::spawn(|| -> Result<Mock> { anyhow::bail!("mock open failed") }).unwrap();
    assert!(capture
        .wait_ready(&AtomicBool::new(false), || Ok(()))
        .is_err());
    drop(capture);
}
#[test]
fn device_failure_after_prefill_and_cancel_release_source() {
    let closed = Arc::new(AtomicBool::new(false));
    let mut capture = mock(closed.clone(), false, None);
    capture
        .wait_ready(&AtomicBool::new(false), || Ok(()))
        .unwrap();
    assert_eq!(capture.block().unwrap()[0], [123; 2]);
    drop(capture);
    assert!(closed.load(Ordering::Acquire));
    let capture = Capture::start("piair_nonexistent_test_pcm").unwrap();
    assert!(capture
        .wait_ready(&AtomicBool::new(false), || Ok(()))
        .is_err());
    assert!(Capture::start("").is_err());
}
#[test]
fn negotiated_period_and_buffer_are_not_packet_or_duration_gates() {
    for (period, buffer) in [(1, 2), (1024, 4096), (8192, 262144)] {
        validate_actual(RATE, 2, Format::s16(), period, buffer).unwrap();
    }
    for (rate, channels, format) in [
        (48000, 2, Format::s16()),
        (RATE, 1, Format::s16()),
        (RATE, 2, Format::s32()),
    ] {
        assert!(validate_actual(rate, channels, format, 1024, 4096).is_err());
    }
}
#[test]
fn partial_stereo_reads_silence_overflow_and_recovery() {
    let mut b = Buffer::default();
    b.push(&[i16::MIN, i16::MAX, 123, -456]);
    let block = b.block().unwrap();
    assert_eq!(block[0], [i16::MIN, i16::MAX]);
    assert_eq!(block[1], [123, -456]);
    assert_eq!(b.silence, 350);
    b.push(&vec![7; (CAPACITY + 3) * 2]);
    assert_eq!(b.frames.len(), CAPACITY);
    assert_eq!(b.dropped, 3);
    b.recovered();
    assert_eq!(b.recoveries, 1);
    assert!(b.block().unwrap().iter().all(|f| *f == [0; 2]));
    b.push(&[100, -200]);
    assert_eq!(b.block().unwrap()[0], [100, -200]);
    b.error = Some("disconnected".into());
    assert!(b.block().is_err());
}
fn push_ids(buffer: &mut Buffer, generated: &mut u32, count: usize) {
    let mut samples = [0i16; (SPF + 1) * 2];
    for frame in samples[..count * 2].as_chunks_mut::<2>().0 {
        *generated += 1;
        *frame = [*generated as i16, (*generated >> 16) as i16];
    }
    buffer.push(&samples[..count * 2]);
}
#[test]
fn twenty_minutes_of_fast_and_slow_capture_has_bounded_latency() {
    for ppm in [-1000, -100, 100, 1000] {
        let mut b = Buffer::default();
        let mut generated = 0;
        for _ in 0..12 {
            push_ids(&mut b, &mut generated, SPF);
        }
        let mut phase = 0u64;
        let mut previous = 0;
        for _ in 0..(20 * 60 * u64::from(RATE) / SPF as u64) {
            phase += SPF as u64 * (1_000_000 + ppm) as u64;
            let n = (phase / 1_000_000) as usize;
            phase %= 1_000_000;
            push_ids(&mut b, &mut generated, n);
            let block = b.block().unwrap();
            for frame in block {
                let id = u32::from(frame[0] as u16) | (u32::from(frame[1] as u16) << 16);
                if id != 0 {
                    assert!(id > previous);
                    assert!(generated - id < CAPACITY as u32);
                    previous = id;
                }
            }
            assert!(b.frames.len() <= CAPACITY);
        }
        if ppm > 0 {
            assert!(b.dropped > 0);
            assert_eq!(b.silence, 0);
        } else {
            assert!(b.silence > 0);
            assert_eq!(b.dropped, 0);
        }
    }
}

// Run ALSA's configuration in a child test process: ALSA_CONFIG_PATH is global
// process state and must not race the other capture/lifecycle tests.
#[test]
fn alsa_plug_converts_48000_s32_stereo_and_mono_without_hardware() {
    for channels in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("input.raw");
        let mut data = Vec::new();
        for _ in 0..48000 {
            data.extend_from_slice(&0x4000_0000i32.to_le_bytes());
            if channels == 2 {
                data.extend_from_slice(&(-0x2000_0000i32).to_le_bytes());
            }
        }
        std::fs::write(&raw, data).unwrap();
        let config = dir.path().join("alsa.conf");
        std::fs::write(&config, format!(
            "pcm.fixture {{ type plug slave {{ pcm {{ type file slave.pcm {{ type null }} infile \"{}\" file \"/dev/null\" format raw }} rate 48000 format S32_LE channels {channels} }} }}\n", raw.display()
        )).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "audio::live::tests::alsa_plug_fixture_child",
                "--nocapture",
            ])
            .env("ALSA_CONFIG_PATH", config)
            .env("PIAIR_LIVE_ALSA_FIXTURE", channels.to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn alsa_plug_fixture_child() {
    let Ok(channels) = std::env::var("PIAIR_LIVE_ALSA_FIXTURE") else {
        return;
    };
    let mut source = Alsa::open("fixture").unwrap();
    let mut samples = [0i16; SPF * 2];
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut reads = 0;
    loop {
        assert!(Instant::now() < deadline);
        match source.read(&mut samples).unwrap() {
            Read::Frames(n) => {
                reads += 1;
                if reads < 3 {
                    continue;
                }
                for frame in samples[..n * 2].as_chunks::<2>().0 {
                    assert!((i32::from(frame[0]) - 16384).abs() <= 2, "{frame:?}");
                    let expected = if channels == "1" { 16384 } else { -8192 };
                    assert!((i32::from(frame[1]) - expected).abs() <= 2, "{frame:?}");
                }
                break;
            }
            Read::Pending => source.wait().unwrap(),
            Read::Recovered => panic!("unexpected fixture XRUN"),
        }
    }
}

#[test]
fn mock_xrun_discards_old_epoch_inserts_silence_and_recovers() {
    struct Recovering {
        available: Arc<AtomicBool>,
        recover: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
        value: i16,
    }
    impl Source for Recovering {
        fn read(&mut self, samples: &mut [i16]) -> Result<Read> {
            if self.recover.swap(false, Ordering::AcqRel) {
                self.value = 42;
                return Ok(Read::Recovered);
            }
            if !self.available.load(Ordering::Acquire) {
                return Ok(Read::Pending);
            }
            samples.fill(self.value);
            Ok(Read::Frames(SPF))
        }
    }
    impl Drop for Recovering {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Release);
        }
    }
    let available = Arc::new(AtomicBool::new(true));
    let recover = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));
    let (input, recovery, released) = (available.clone(), recover.clone(), closed.clone());
    let mut capture = Capture::spawn(move || {
        Ok(Recovering {
            available: input,
            recover: recovery,
            closed: released,
            value: 11,
        })
    })
    .unwrap();
    let cancel = AtomicBool::new(false);
    capture.wait_ready(&cancel, || Ok(())).unwrap();
    assert_eq!(capture.block().unwrap()[0], [11; 2]);
    available.store(false, Ordering::Release);
    recover.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if capture.buffer.lock().unwrap().recoveries == 1 {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::park_timeout(Duration::from_millis(1));
    }
    assert!(capture.block().unwrap().iter().all(|f| *f == [0; 2]));
    available.store(true, Ordering::Release);
    capture.wait_ready(&cancel, || Ok(())).unwrap();
    assert!(capture.block().unwrap().iter().all(|f| *f == [42; 2]));
    drop(capture);
    assert!(closed.load(Ordering::Acquire));
}
