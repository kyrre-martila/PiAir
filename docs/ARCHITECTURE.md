# Architecture

PiAir is one Rust CLI and an optional systemd service. Setup and the daily menu
save a selected input, receiver and authentication method. `run --allow-unattended`
loads those settings and supervises activity-driven live playback.

- `audio/live.rs` owns bounded ALSA capture and recovery; `audio/activity.rs`
  detects activity. Legacy monitor/calibration use the separate detector semantics
  described in [AUDIO_ACTIVITY.md](AUDIO_ACTIVITY.md).
- `receiver/automatic.rs` and `runtime.rs` manage automatic playback, silence,
  retries and shutdown. `playback.rs` drives direct live or WAV realtime PTP media.
- `receiver/session.rs`, `lifecycle.rs`, `ptp/` and `realtime.rs` own authenticated
  session state, timing, retransmission, FLUSH and TEARDOWN. Buffered live playback
  remains disabled; shared lifecycle code also supports bounded control diagnostics.
- `receiver/wire.rs` confines socket creation to the selected authorized peer and
  validated negotiated ports. Approval and endpoint bindings are rechecked.
- `pairing.rs`, `srp.rs`, `crypto.rs` and `tlv.rs` implement authentication;
  `store.rs` keeps private selection and credentials. See [PAIRING.md](PAIRING.md).
- `receiver/setup/` implements setup, daily controls and service recording tests.
  `maintenance.rs` and the installer preserve configuration and credentials through
  updates, rollback and uninstall. See [INSTALL.md](INSTALL.md) and [SYSTEMD.md](SYSTEMD.md).

Discovery is passive mDNS: advertisements are unverified and never authorize
connections. Explicit approval of an exact canonical receiver ID and explicit
selection precede authentication. Friendly names and shared IPs are not identities.
Stored HAP verifies signed receiver identity; transient authentication has different
trust limits and requires explicit selection, without fallback. See
[transient authentication](TRANSIENT_AUTHENTICATION.md).

Logs must not contain PCM, credentials, pairing secrets, raw TXT or key material.
Configuration errors omit source snippets; advertised strings are escaped.
Application code forbids unsafe Rust. Dependencies can contain unsafe code; see
[dependency and licensing notes](DEPENDENCIES.md).

The working live path uses realtime PTP/type-96, ALAC16 stereo at 44100 Hz and
bounded capture queues. Capture faults discard stale input; network reconnection
requires current approval and fresh activity. See [runtime](RUNTIME.md) and
[live capture](LIVE_CAPTURE.md) for buffering and hardware limitations.
