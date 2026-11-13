# Bounded WAV playback diagnostic

For HomePod Stua (`device:0a05559f132f`), after deploying a release build,
with the receiver already approved and explicitly selected for transient authentication:

```sh
./target/release/piair -v stream-test device:0a05559f132f --experimental-owntone-transient --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav --seconds 30 --volume-db -20
```

For Stua Apple TV (`device:02d899c1833f`, AppleTV6,2), approve and explicitly
select that receiver using its persistent pairing method, and retain its existing
local HAP pairing. After deploying a release build:

```sh
./target/release/piair -v stream-test device:02d899c1833f \
  --audio-file tests/assets/audio/Pachelbel_Canon_audio_test.wav \
  --seconds 30 \
  --volume-db -20
```

Without `--experimental-owntone-transient`, WAV requires a persistent selection
and validated local HAP credentials. The stored keys establish an operation-only
StoredHap method, then pair-verify cryptographically verifies receiver identity.
The existing credential fingerprint and approval checks remain active throughout
the operation. No pairing, credential update, receiver switch or authentication
fallback is attempted. Missing credentials require explicit pairing first;
a changed endpoint or policy requires investigation and explicit reselection.
A transient selection requires the explicit experimental flag and policy approval;
that flag cannot be combined with a persistent selection. Apple TV's receiver
policy disallows transient authentication.

Hardware evidence before this change: HomePod Stua played 30 seconds of this WAV
audibly and continuously at -20 dB using OwnTone transient; subsequent TEARDOWN
failed. Apple TV's stored-HAP tone diagnostic completed pair-verify, encrypted
SETUP/RECORD, stream SETUP, volume, 376 RTP packets, TEARDOWN HTTP 200 and cleanup,
but the one-second -40 dB tone was not audible. **Apple TV WAV audibility still
requires deployment and physical listening on hardware.** Record that result
separately from transport success. Existing HAP tone commands are unchanged.

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

`--seconds` defaults to 1 and permits 1–49 for WAV. Playback stops at the
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

## HomePod duration-boundary investigation (follow-up)

The `bounded transmission outcome` log's `success` field is computed from
`Session::transmit_input` before `Session::stop` attempts TEARDOWN. A subsequent
TEARDOWN failure therefore cannot explain `success=false` in that playback log.
The duration limit itself exits the packet loop normally, sends the existing
silent drain and checks worker health before returning success. Failures can
still arise from worker health/NTP freshness, scheduler guards, send errors,
revoked authorization, cancellation or prohibited retransmission requests;
the exact hardware error chain is needed to identify which occurred. No causal
claim beyond the ordering is established, and this change does not alter timing,
feedback, retransmission handling or TEARDOWN. Investigate with the playback error
chain, packet count and timestamps in a separate PR.

PR #53 records two pre-existing timing-sensitive tests:
`both_stream_modes_report_failed_remote_teardown_and_release_locally` and
`transient_session_negotiates_encrypted_audio_and_acknowledges_teardown`.
Scheduler stalls under CI load must be distinguished from new failures; do not
relax production timing guards to make loopback tests pass.

The optional `piair setup` test uses the same sender and existing recording for
49 seconds (or earlier EOF), preserving receiver volume. See [SETUP.md](SETUP.md)
for resource locations and the recording’s separate CC BY-SA 3.0 attribution.
