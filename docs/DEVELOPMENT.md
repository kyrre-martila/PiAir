# Development and diagnostic commands

Normal operation starts with `piair setup`, then `piair` for the unchanged daily
menu. See [INSTALL.md](INSTALL.md), [SETUP.md](SETUP.md) and
[DAILY_USE.md](DAILY_USE.md). Detailed source-build instructions, manual audio
configuration and retained protocol procedures are in [ADVANCED_USAGE.md](ADVANCED_USAGE.md).

## CLI review and classification

| Class | Commands | Presentation and purpose |
| --- | --- | --- |
| A: normal use | No command, `setup`, `status`, `start`, `stop`, `restart`, `logs`, `devices`, `audio devices`, `config`, `update`, `uninstall` | Daily menu and prominent top-level help; installation and maintenance retain PR #74 behavior. |
| B: advanced troubleshooting/configuration | `audio monitor`, `audio calibrate`, `config check`, `config show`, `config set-playback`, `allowlist`, `receivers`, `pairing`, `select`, `pair`, `unpair` | Available in help for manual configuration and audio/receiver troubleshooting. |
| B: service/foreground runtime | `run --allow-unattended` | Hidden from top-level help; fully supported systemd entry point with existing explicit authorization. |
| C: protocol development/hardware diagnostics | `receiver-plan`, `authenticate`, `stream-test`, `ptp-negotiation-check`, `ptp-wav-playback`, `experimental-ptp-capture`, `automatic-ptp-capture`, `buffered-test`, `hap-stream-setup-check`, `hap-playback-check` | Hidden using Clap `hide = true`; callable with unchanged arguments and individual `--help`. |
| D: demonstrably unused/redundant code | None identified for removal in this review | Every command has a dispatch path. Tests, protocol fixtures, compatibility checks and distinct senders are retained. |

The `stua-stream-setup-check` and `stua-playback-check` aliases remain supported.
Hidden commands are not disabled or deprecated. In particular, experimental PTP
capture implements useful transport behavior shared by daily playback; its name
is not evidence that the implementation is disposable. NTP WAV, PTP WAV, live
capture and buffered diagnostics retain their distinct implementations and gates.

Historical PR #77 review scope: `src/cli.rs`, `src/main.rs`, `src/receiver/setup/` (daily menu and
setup), receiver authentication/runtime/selection, integration tests, and PR #74's
installer, maintenance helper, service contract and release workflows. Only help
metadata changed executable behavior in that historical review. No configuration, authentication,
authorization, cancellation, service arguments or transport changes are intended.

## Validation status

- **Implementation behavior:** explicit receiver approval/selection and
  authentication, bounded playback, cleanup, configuration retention and service
  contracts have automated regression coverage. Authentication methods never
  silently fall back. Legacy `[audio]` and `[playback]` detector semantics remain
  distinct; no migration or re-pairing is introduced.
- **Automated tests:** synthetic protocol peers, codecs, audio fixtures, PTY
  interaction and Python/systemd tests exercise local behavior. Native ARM64
  Bookworm packaging and clean-runtime CI verify build/ELF/dependency/startup
  behavior. Green CI does not prove physical audio or universal interoperability.
- **Historical physical observations:** Apple TV Example accepted authenticated PTP
  type-96 UDP, with audibility still unconfirmed. An earlier HomePod transient
  NTP stream was audible but stopped after about four seconds with an NTP timing
  stall. These are limited observations, not release-readiness evidence. See
  [the recorded PTP work and HomePod test](PTP_IMPLEMENTATION_PLAN.md#validation-and-remaining-hardware-work)
  and [Apple TV investigation](APPLE_TV_SILENT_PLAYBACK.md).
- **Pending physical verification:** sustained Raspberry Pi/USB capture, receiver
  audibility, timing stability, unplug recovery and cancellation on actual target
  hardware remain final QA work. See [live capture validation](LIVE_CAPTURE.md#automated-validation-and-hardware-limits).
- **Explicitly blocked:** buffered live audio remains gated on receiver
  clock-adoption evidence and media lifecycle integration. `buffered-test
  --confirm-audio` still fails locally; hiding commands enables no transport.
  [Buffered lifecycle](BUFFERED_LIFECYCLE.md) and
  [compatibility evidence](AIRPLAY2_COMPATIBILITY.md) retain the historical record.

## Regression checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 -m unittest discover -s scripts -p 'test_*.py'
```

`tests/cli.rs` checks concise ordinary help and hidden command help/arguments,
including legacy aliases, invalid combinations, authorization failures, WAV
bounds and unattended opt-in. `tests/setup.rs` uses a real PTY to verify that no
command opens the existing daily menu, including all six choices and cancellation.
Existing unit/integration tests retain authentication, configuration, timing,
security and cleanup coverage. `scripts/test_systemd.py` pins the service's
`piair run --allow-unattended` arguments. Release/installation Python tests cover
credential preservation, controlled updates, rollback and safe uninstall.

The existing `.github/workflows/release.yml` runs native ARM64 Bookworm packaging
and clean runtime checks on PRs without publishing. No release or version tag is
needed for this cleanup. See [CLI hardware verification](CLI_VERIFICATION.md) and
individual diagnostic guides before deliberately contacting a receiver.

## Release QA

Ordinary commands emit readable warnings/errors on stderr, without INFO cleanup
chatter. `-v` enables DEBUG and `-vv` TRACE in readable form. `run` retains JSON
INFO lifecycle events for unattended operation and systemd; verbosity adds full
safe diagnostics. Warnings and cleanup failures remain visible in every mode.
Verbose diagnostics can interrupt prompts, so use them when investigating a
problem. Version 1.0.2 is published and physically tested on Debian 13 ARM64
with UCA202 and HomePod Stua. See the [historical hardware checklist](PRE_RELEASE_QA.md).
Historical diagnostic documents retain earlier observations; example network
addresses and receiver names/IDs have been anonymized and must be replaced.
