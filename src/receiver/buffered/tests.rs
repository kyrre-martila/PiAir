use super::*;
use std::{io::Read, net::TcpListener};
fn connected() -> (Transport, TcpStream, [u8; 32]) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let peer = listener.accept().unwrap().0;
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut prepared = Prepared::new().unwrap();
    let key = *prepared.key;
    let offer = prepared.setup_offer(-7).unwrap();
    let v = profile::parse(&offer).unwrap();
    let s = v.as_dictionary().unwrap()["streams"].as_array().unwrap()[0]
        .as_dictionary()
        .unwrap();
    assert_eq!(profile::integer(s, "type").unwrap(), 103);
    assert_eq!(profile::integer(s, "audioFormat").unwrap(), 1 << 21);
    assert_eq!(s["streamConnectionID"].as_signed_integer(), Some(-7));
    assert!(prepared.setup_offer(1).is_err());
    (
        prepared
            .attach(socket, SEQUENCE_MASK, u32::MAX - 100)
            .unwrap(),
        peer,
        key,
    )
}
#[test]
fn alac24_independent_decoder_preserves_all_bits_and_channels() {
    use symphonia_codec_alac::AlacDecoder;
    use symphonia_core::{
        audio::Signal,
        codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_ALAC},
        formats::Packet,
    };
    let pcm: Vec<_> = (0..codec::FRAMES)
        .map(|i| {
            [
                if i == 0 {
                    -8_388_608
                } else {
                    (i as i32) * 7919
                },
                if i == 0 {
                    8_388_607
                } else {
                    -(i as i32) * 3137
                },
            ]
        })
        .collect();
    let payload = Alac24.encode(&pcm).unwrap();
    let mut params = CodecParameters::new();
    params
        .for_codec(CODEC_TYPE_ALAC)
        .with_extra_data(Box::new(codec::cookie()));
    let mut decoder = AlacDecoder::try_new(&params, &DecoderOptions::default()).unwrap();
    let input = Packet::new_from_slice(0, 0, 0, &payload);
    let decoded = decoder.decode(&input).unwrap();
    assert_eq!(decoded.spec().rate, 48_000);
    let mut samples =
        symphonia_core::audio::AudioBuffer::<i32>::new(decoded.capacity() as u64, *decoded.spec());
    decoded.convert(&mut samples);
    for (i, expected) in pcm.iter().enumerate() {
        assert_eq!(
            [samples.chan(0)[i] >> 8, samples.chan(1)[i] >> 8],
            *expected
        );
    }
    assert!(Alac24.encode(&[]).is_err());
    assert!(Alac24.encode(&[[8_388_608, 0]; codec::FRAMES]).is_err());
}
#[test]
fn loopback_framing_wrap_backpressure_shutdown_and_restart() {
    let (mut t, mut peer, key) = connected();
    let (other, _, other_key) = connected();
    assert_ne!(key, other_key);
    drop(other);
    let pcm = [[-1, 8_388_607]; codec::FRAMES];
    for counter in 0..3u64 {
        assert!(t.enqueue(&pcm, counter == 0).unwrap());
        let before = (t.nonce, t.sequence, t.timestamp);
        assert!(!t.enqueue(&pcm, false).unwrap());
        assert_eq!((t.nonce, t.sequence, t.timestamp), before);
        assert!(t.pending.len() <= MAX_FRAME);
        t.drain(
            Instant::now() + Duration::from_secs(1),
            &AtomicBool::new(false),
        )
        .unwrap();
        let mut prefix = [0; 2];
        peer.read_exact(&mut prefix).unwrap();
        let mut packet = vec![0; u16::from_be_bytes(prefix) as usize];
        packet[..2].copy_from_slice(&prefix);
        peer.read_exact(&mut packet[2..]).unwrap();
        assert_eq!(&packet[packet.len() - 8..], &counter.to_le_bytes());
        assert_eq!(
            u32::from_be_bytes(packet[2..6].try_into().unwrap()) & SEQUENCE_MASK,
            (SEQUENCE_MASK + counter as u32) & SEQUENCE_MASK
        );
        assert_eq!(
            u32::from_be_bytes(packet[6..10].try_into().unwrap()),
            (u32::MAX - 100).wrapping_add(counter as u32 * codec::FRAMES as u32)
        );
        assert_eq!(
            open_frame(&key, &packet).unwrap(),
            Alac24.encode(&pcm).unwrap()
        );
    }
    t.shutdown(
        Instant::now() + Duration::from_secs(1),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
struct ShortWriter {
    output: Vec<u8>,
    calls: usize,
    stall: bool,
}
impl Write for ShortWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.stall || self.calls.is_multiple_of(3) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if self.calls.is_multiple_of(5) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let n = b.len().min(3);
        self.output.extend_from_slice(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn partial_writes_stalls_deadlines_and_cancel_are_bounded() {
    let bytes = frame(&[7; 32], 0, 1, 2, true, b"encoded audio").unwrap();
    let mut w = ShortWriter {
        output: vec![],
        calls: 0,
        stall: false,
    };
    let mut offset = 0;
    drain_to(
        &mut w,
        &bytes,
        &mut offset,
        Instant::now() + Duration::from_secs(1),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(w.output, *bytes);
    w.stall = true;
    offset = 0;
    assert!(drain_to(
        &mut w,
        &bytes,
        &mut offset,
        Instant::now() + Duration::from_millis(10),
        &AtomicBool::new(false)
    )
    .is_err());
    assert_eq!(offset, 0);
    assert!(drain_to(
        &mut w,
        &bytes,
        &mut offset,
        Instant::now() + Duration::from_secs(1),
        &AtomicBool::new(true)
    )
    .is_err());
    let (mut t, mut peer, _) = connected();
    t.enqueue(&[[0; 2]; codec::FRAMES], false).unwrap();
    assert!(t.drain(Instant::now(), &AtomicBool::new(false)).is_err());
    assert!(t.failed && t.key.iter().all(|b| *b == 0) && t.pending.is_empty());
    assert!(t.enqueue(&[[0; 2]; codec::FRAMES], false).is_err());
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
#[test]
fn malformed_frames_and_integrity() {
    let key = [9; 32];
    let f = frame(&key, 42, 17, 123, true, b"ALAC bytes").unwrap();
    for length in 0..f.len() {
        assert!(open_frame(&key, &f[..length]).is_err());
    }
    for i in [0, 1, 2, 6, 10, 14, f.len() - 9, f.len() - 1] {
        let mut bad = f.to_vec();
        bad[i] ^= 1;
        assert!(open_frame(&key, &bad).is_err(), "offset {i}");
    }
    let mut reserved = f.to_vec();
    reserved[3] |= 0x80;
    assert!(open_frame(&key, &reserved).is_err());
    // Sequence is deliberately outside AAD; a low sequence bit is malleable.
    let mut sequence = f.to_vec();
    sequence[5] ^= 1;
    assert!(open_frame(&key, &sequence).is_ok());
    assert!(frame(&key, 0, 0, 0, false, &[]).is_err());
    assert!(frame(&key, 0, 0, 0, false, &vec![0; MAX_FRAME]).is_err());
    assert!(open_frame(&key, &vec![0; MAX_FRAME + 1]).is_err());
    assert!(frame(&key, 0, SEQUENCE_MASK + 1, 0, false, b"a").is_err());
}
#[test]
fn nonce_exhaustion_and_codec_errors_destroy_key() {
    for exhausted in [false, true] {
        let (mut t, mut peer, _) = connected();
        if exhausted {
            t.nonce = u64::MAX;
        }
        let pcm = vec![[0; 2]; if exhausted { codec::FRAMES } else { 1 }];
        assert!(t.enqueue(&pcm, false).is_err());
        assert!(t.key.iter().all(|b| *b == 0));
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }
}
#[test]
fn selection_requires_local_encoder_and_valid_claims() {
    use BufferedFormat::*;
    for claim in [
        Claim::Unknown,
        Claim::Invalid,
        Claim::Conflict,
        Claim::Observed(vec![]),
        Claim::Observed(vec![AacLc48000Stereo, Unknown(99)]),
    ] {
        assert_eq!(select_format(&claim), None);
    }
    for f in [
        vec![Unknown(99), Alac48000Stereo24, AacLc44100Stereo],
        vec![Alac48000Stereo24, Unknown(99)],
    ] {
        assert_eq!(select_format(&Claim::Observed(f)), Some(Alac48000Stereo24));
    }
}

#[test]
fn rfc8439_section_2_8_2_known_answer() {
    fn hex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
            .collect()
    }
    let key: [u8; 32] = (0x80..=0x9f).collect::<Vec<_>>().try_into().unwrap();
    let nonce: [u8; 12] = hex("070000004041424344454647").try_into().unwrap();
    let aad = hex("50515253c0c1c2c3c4c5c6c7");
    let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let expected = hex(concat!(
        "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d6",
        "3dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b36",
        "92ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3",
        "ff4def08e4b7a9de576d26586cec64b6116",
        "1ae10b594f09e26a7e902ecbd0600691"
    ));
    assert_eq!(
        *crypto::seal(&key, &nonce, plaintext, &aad).unwrap(),
        expected
    );
    assert_eq!(
        *crypto::open(&key, &nonce, &expected, &aad).unwrap(),
        plaintext
    );
}

#[test]
fn real_tcp_stalled_receiver_deadline_and_disconnect_cleanup() {
    let (mut t, mut peer, _) = connected();
    // Bound the kernel send queue so a non-reading loopback peer stalls.
    socket2::SockRef::from(&t.socket)
        .set_send_buffer_size(1024)
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(100);
    loop {
        t.enqueue(&[[1, -1]; codec::FRAMES], false).unwrap();
        if t.drain(deadline, &AtomicBool::new(false)).is_err() {
            break;
        }
    }
    assert!(t.failed && t.pending.is_empty() && t.key.iter().all(|b| *b == 0));
    // Drain already delivered bytes before checking the closed connection.
    let mut sink = Vec::new();
    peer.read_to_end(&mut sink).unwrap();
    let (mut t, peer, _) = connected();
    socket2::SockRef::from(&peer)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(peer);
    t.enqueue(&[[0; 2]; codec::FRAMES], false).unwrap();
    assert!(t
        .drain(
            Instant::now() + Duration::from_secs(1),
            &AtomicBool::new(false)
        )
        .is_err());
    assert!(t.failed && t.key.iter().all(|b| *b == 0));
}

#[test]
fn independent_python_type103_known_answer() {
    // Python cryptography/OpenSSL ChaCha20Poly1305 + struct.pack; see docs.
    let expected = [
        0x00, 0x2e, 0x80, 0x7f, 0xff, 0xff, 0x11, 0x22, 0x33, 0x44, 0x15, 0, 0, 0, 0xd0, 0x49,
        0xbf, 0xd0, 0xfd, 0x41, 0x70, 0xa4, 0xb0, 0x0a, 0xb4, 0x0b, 0xb0, 0x3b, 0x18, 0x09, 0xd0,
        0xea, 0xc4, 0xfb, 0x65, 0xca, 0xd2, 0x79, 8, 7, 6, 5, 4, 3, 2, 1,
    ];
    let key: [u8; 32] = (0..32).collect::<Vec<_>>().try_into().unwrap();
    let payload = [0x20, 0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc];
    let actual = frame(
        &key,
        0x0102030405060708,
        SEQUENCE_MASK,
        0x11223344,
        true,
        &payload,
    )
    .unwrap();
    assert_eq!(actual.as_slice(), expected);
    assert_eq!(open_frame(&key, &expected).unwrap().as_slice(), payload);
}

#[test]
fn setup_reply_rejects_changed_format_ports_and_connection() {
    use plist::Value;
    let entry = || {
        profile::dict([
            ("type", profile::uint(103)),
            ("dataPort", profile::uint(5000)),
            ("audioFormatIndex", profile::uint(21)),
            ("streamConnectionID", Value::Integer((-7i64).into())),
        ])
    };
    let wrap = |v| profile::dict([("streams", Value::Array(vec![v]))]);
    assert_eq!(
        NegotiatedStream::parse(&wrap(entry()), -7)
            .unwrap()
            .data_port
            .get(),
        5000
    );
    for (field, value) in [
        ("type", profile::uint(96)),
        ("dataPort", profile::uint(0)),
        ("dataPort", profile::uint(65536)),
        ("audioFormatIndex", profile::uint(23)),
        ("ct", profile::uint(4)),
        ("streamConnectionID", profile::uint(7)),
    ] {
        let mut e = entry();
        e.as_dictionary_mut().unwrap().insert(field.into(), value);
        assert!(NegotiatedStream::parse(&wrap(e), -7).is_err());
    }
    for v in [
        profile::dict([]),
        profile::dict([("streams", Value::Array(vec![]))]),
        Value::String("bad".into()),
    ] {
        assert!(NegotiatedStream::parse(&v, -7).is_err());
    }
}

#[test]
fn drop_pending_and_inflight_cancellation_close_without_reuse() {
    let (mut t, mut peer, _) = connected();
    t.enqueue(&[[0; 2]; codec::FRAMES], false).unwrap();
    drop(t);
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::park_timeout(Duration::from_millis(10));
        signal.store(true, Ordering::Relaxed);
    });
    let mut writer = ShortWriter {
        output: vec![],
        calls: 0,
        stall: true,
    };
    let start = Instant::now();
    let error = drain_to(
        &mut writer,
        b"pending ciphertext",
        &mut 0,
        start + Duration::from_secs(3),
        &cancel,
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(start.elapsed() < Duration::from_secs(1));
    trigger.join().unwrap();
}

#[test]
fn maximum_frame_boundary_is_checked_before_encryption() {
    let payload = vec![42; MAX_FRAME - OVERHEAD];
    let f = frame(&[7; 32], 0, 0, 0, false, &payload).unwrap();
    assert_eq!(f.len(), MAX_FRAME);
    assert_eq!(open_frame(&[7; 32], &f).unwrap().as_slice(), payload);
    assert!(frame(
        &[7; 32],
        0,
        0,
        0,
        false,
        &vec![42; MAX_FRAME - OVERHEAD + 1]
    )
    .is_err());
}
