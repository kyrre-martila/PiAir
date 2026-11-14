# Development

Build with a current stable Rust toolchain and ALSA development headers. See
[advanced usage](ADVANCED_USAGE.md) for source builds and manual audio tools.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 -m unittest discover -s scripts -p 'test_*.py'
```

## Retained diagnostics

`receiver-plan` inspects passive advertisements without contacting receivers.
`authenticate` checks authentication and optionally encrypted control. `stream-test`
exercises bounded NTP tone/WAV playback and individual negotiation stages.
`ptp-negotiation-check` observes timing without audio; `ptp-wav-playback` checks a
bounded recording. `experimental-ptp-capture` provides direct live ALSA playback;
`automatic-ptp-capture` adds activity detection. These share production protocol,
authorization, timing and cleanup code. Their individual `--help` describes flags.
`audio monitor` and `audio calibrate` remain useful input troubleshooting tools.

`buffered-test` remains a non-playing control diagnostic with separately confirmed
stages. Its lifecycle is shared with production realtime PTP; regression tests
preserve failure, clock-adoption and cleanup gates. Buffered live audio is unsupported.
The redundant `hap-stream-setup-check` / `hap-playback-check` commands and their
`stua-*` aliases have been removed. Use `stream-test` with stored HAP credentials
for NTP diagnostics; normal setup and daily recording tests remain available.

Hidden `run --allow-unattended` is the supported systemd entry point, and
`service-recording` is the temporary recording service worker. Neither is disposable
diagnostic code. Setup, approval, credential and transport regression tests remain.

## Validation status

Version 1.0.2 was physically tested on Debian 13 ARM64 with UCA202 and HomePod
Stua, including automatic playback and installed systemd startup. Automated tests
cover synthetic receivers, PTYs, codecs, authorization, cleanup, installation,
updates and rollback. They cannot establish audibility or hardware interoperability.
After changes, verify USB input, sustained HomePod playback, silence start/stop,
unplug recovery, reconnect, cancellation and boot startup on actual target hardware.
