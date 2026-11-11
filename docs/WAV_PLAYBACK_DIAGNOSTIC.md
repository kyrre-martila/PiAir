# Bounded WAV playback diagnostic

For HomePod Stua (`device:0a05559f132f`), after deploying a release build,
with the receiver already approved and explicitly selected for transient authentication:

```sh
./target/release/piair -v stream-test device:0a05559f132f --experimental-owntone-transient --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav --seconds 30 --volume-db -20
```

Run in an interactive terminal and enter `y` or `yes` only after reviewing the
receiver and volume warning. Receiver volume may persist after success or failure;
it is not restored. Start at -20 dB. The permitted receiver range is -60..=-10 dB.
There is no automatic authentication fallback or selection change.

The primary QA asset is 50 seconds of RIFF PCM, 48 kHz, stereo, signed 16-bit
little endian (see the asset's license). The diagnostic supports integer PCM
format 1, 8/16/24/32 bits, mono or stereo, 8–192 kHz. Mono duplicates into left
and right; stereo preserves left/right order. Other encodings (including float,
compressed and extensible WAV) and channel layouts fail before connection.
Input reads are bounded to the requested duration plus one interpolation frame.
Conversion uses linear interpolation to 44.1 kHz and saturating 16-bit output;
it is a diagnostic resampler, not a mastering-quality antialias filter.

`--seconds` defaults to 1 and permits 1–30 for WAV. Playback stops at the
requested duration or EOF, whichever comes first, and reports which limit applies.
The remainder of a longer file is explicitly not played. Short files are never
looped. Both boundaries have up to 50 ms fades. Final partial packets are padded
with silence, followed by the existing two-second latency drain and TEARDOWN.
WAV uses the existing ALAC/encrypted realtime RTP transport, sequence/timestamp
handling and failure cleanup. Other diagnostic modes cannot be combined with WAV;
`--playback-check` retains its fixed one-second tone and -40 dB volume.

Successful RTP transmission, authenticated negotiation and acknowledged TEARDOWN
alone do **not** prove audible playback. At Stua, physically verify music, channel
balance, clean start/end and absence of repeated audio. Check the reported packet
count (for 30 seconds plus drain: ceil(32 * 44100 / 352) = 4010 packets), teardown
and cleanup. Record the actual listening result separately. CI uses synthetic WAV
fixtures and loopback encrypted receivers; it cannot verify HomePod audibility.
