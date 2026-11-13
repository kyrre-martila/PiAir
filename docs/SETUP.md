# Interactive setup

Run `piair setup` as the Linux account that will run PiAir. Use a local terminal
or an SSH session with a TTY (`ssh -t ...`). Both stdin and stdout must be
terminals. Root is not required. Setup uses numbered, line-oriented prompts;
it needs no fullscreen terminal features. Enter `q`, send EOF (Ctrl+D at an empty
prompt), or press Ctrl+C to cancel. EOF never means Yes. Back choices are available
before leaving audio selection and at the final settings review. Failed input
validation can be retried; receiver discovery can be rescanned.

The wizard checks Linux, ALSA device presence, local network interfaces, existing
configuration and optional installed systemd support. Network checks are only
prerequisites: they do not establish receiver reachability. It never installs
packages, changes sound permissions, edits system settings or installs a service.
A source checkout is not an installed service.

## First-time Raspberry Pi and USB input workflow

1. Connect a USB audio interface and its audio source to a Raspberry Pi running
   a supported Linux distribution with ALSA. Put the Pi and receiver on the same
   local network. Install PiAir using the [release installation procedure](INSTALL.md).
2. Run `piair setup` from your ordinary account. Select an actual discovered input
   by number, or enter its ALSA identifier. Friendly names and identifiers come
   from ALSA hints. Even one input requires an explicit selection. If necessary,
   use `arecord -l` and `arecord -L` to find your card's own identifier, such as
   `plughw:CARD=<your-card-id>,DEV=0`; replace the placeholder.
3. Setup opens exactly that input with the automatic runtime's 44100 Hz signed
   16-bit stereo client requirements, then closes it. If a raw `hw:` device
   cannot provide that format, explicitly select its `plughw:` conversion PCM.
   Setup does not silently replace the identifier or choose another card.
4. Select your AirPlay receiver after discovery. Inspect its canonical device ID,
   endpoint, capabilities and local approval/selection/pairing state. Verify its
   identity independently using a physical label or trusted receiver settings.
   mDNS advertisements are untrusted. New approval requires two explicit
   confirmations; choosing a menu row alone grants nothing.
5. Choose an advertised compatible authentication path. Existing validated HAP
   credentials can be reused without enrollment. PIN-display enrollment requires
   actual supported PIN delivery. HAP setup-code enrollment requires a genuine
   independently obtained code; an AirPlay password is not a HAP setup code.
   Codes are entered without echo. OwnTone-reference transient authentication
   requires explicit confirmation and does not establish verified persistent
   receiver identity. There is no automatic authentication fallback.
6. Keep the current/default activity settings or adjust them. The numeric defaults
   are -45 dBFS, 1000 ms sustained audio to start and 30000 ms silence to stop;
   they are starting points, not universal hardware recommendations. The optional
   measurement step shows actual input levels using the automatic detector. Stop
   the source for five seconds, then play representative music for five seconds.
   Only sufficiently separated measurements yield a provisional suggestion
   using the background's 95th percentile and quieter music's 25th percentile.
   The suggestion is at least six dB above background, preferably twelve dB below
   quiet music, and never below -60 dBFS. It requires at least six dB of headroom
   below quiet music; overlapping or very weak measurements get no suggestion.
   Digital silence does not justify an extremely sensitive threshold. These are
   conservative suggestion limits only; manual configuration retains its full
   supported range. Confirm before applying a suggestion to the draft and again
   before saving the reviewed configuration.
   Repeat under realistic quiet/noisy conditions and verify your chosen settings.
7. Keep or explicitly change the saved runtime volume. An absent volume preserves
   the receiver volume. An explicit change may persist on the receiver even after
   playback or failure. The optional recording test always preserves receiver
   volume. Choose Play explicitly to hear up to 49 seconds, bounded by actual
   file content, through the existing PTP/ALAC/RTP sender. Ctrl+C interrupts it
   through existing cancellation and bounded cleanup. Answer whether you heard
   audio after a completed test: successful protocol responses or packets alone
   never verify audibility. A failed test reports the error and troubleshooting
   guidance, then continues setup without an audible-success confirmation prompt.
   This test checks output; it does not test ALSA input.
8. Review the final summary; changed values are marked and previous values shown.
   Save only when satisfied. Setup retains TOML comments, unrelated configuration,
   allowlist entries, protected credentials, file ownership and permissions using
   the existing atomic writer and concurrent-edit checks. Restart PiAir explicitly
   to apply operational changes. For a foreground trial, run
   `piair run --allow-unattended`, retaining any custom `--config` argument.
9. Optionally enable/start the already installed stock service for your current
   non-root account. No is the default. Setup displays the exact unit and saved
   path, verifies configuration resolution and requires another confirmation.
   Normal system authorization (systemctl/polkit) applies. Declining or lacking
   service support does not prevent successful setup. See [SYSTEMD.md](SYSTEMD.md)
   and [release installation](INSTALL.md).

## Reconfiguration and cancellation

Re-run `piair setup` later, using the same account and config path. Existing numeric
settings are used as the draft; choose the current input explicitly, review the
receiver, choose authentication explicitly, and reuse stored credentials when
appropriate. Opening setup alone does not overwrite working settings.

Approval, receiver selection and verified enrollment are separate explicit writes;
the wizard is **not one atomic transaction**. If cancelled after one of these
operations, setup reports the completed writes. They remain available when setup
is resumed. Unconfirmed playback settings are discarded. A failed filesystem
sync or enrollment can have an uncertain persistence outcome; inspect the reported
state before retrying. Credentials are never silently removed or replaced.

Setup reserves the existing per-user automatic-runtime lock before changing state
or testing audio. Stop your own `piair run` or `automatic-ptp-capture` first. Setup
will not stop a process or another user's service. The lock stays held until
capture/playback cleanup completes. It does not coordinate different Linux users
or network namespaces. Operational settings take effect on explicit restart;
security revocation checks remain active during running sessions.

## Test recording and shared data

The existing recording is **Pachelbel's Canon in D**, arranged/performed by
**Lee Galloway**, derived from
[the Wikimedia recording](https://commons.wikimedia.org/wiki/File:Pachelbel%27s_Canon.ogg).
It is separately licensed under
[CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/), not PiAir's
Apache-2.0 license. See [the full attribution](../tests/assets/audio/LICENSE.md)
for source, modifications and licensing. Setup also displays attribution before
playback. No additional tone, recording or sender is created for setup.

Resource resolution requires **both** `Pachelbel_Canon_audio_test.wav` and
`LICENSE.md` in the same directory, in this order:

- If `PIAIR_AUDIO_DATA_DIR` is set, only that directory is checked. It is a directory
  override, not a URL, and a missing override is not silently replaced.
- `/usr/local/share/piair/audio`.
- `/usr/share/piair/audio`.
- The build checkout's `tests/assets/audio` directory, for development.

The executable does not embed the roughly 9.6 MB recording. Installed binaries
work independently of the current working directory. The release installer packages the existing WAV and its unchanged attribution together in one of these
shared-data locations. A missing recording or license produces guidance and lets
setup finish without the test. Setup never downloads the recording or remote code.
WAV parsing/sender/CLI bounds allow 1–49 seconds; the diagnostic CLI retains its
30-second default. EOF, resampling, fades, packet scheduling, silent drain and
bounded teardown retain their existing behavior. No content is looped or extended;
only the existing partial-packet padding and silent drain follow actual content.

## Troubleshooting

- **No ALSA inputs:** reconnect the interface; check `piair audio devices`,
  `arecord -l`, `arecord -L` and `/dev/snd`. Manual identifier entry and rescan
  remain available even when no hints exist.
- **Unavailable/permission denied/busy:** read the underlying ALSA error. Verify
  the identifier, ordinary user's device permissions, and competing applications.
  Stop a competing capture yourself. Do not run setup as root to bypass access.
- **Unsupported capture format:** explicitly use the same card's `plughw:` PCM;
  the automatic client needs 44100 Hz S16 interleaved stereo.
- **No/ambiguous receivers:** check power, Wi-Fi/Ethernet network membership and
  multicast/firewall policy, then rescan. Never approve a guessed device ID.
- **Pairing prerequisite absent:** cancel or explicitly choose another supported
  method after review. Do not invent PINs/codes. Failed HAP authentication never
  triggers transient fallback. Existing credential errors require investigation.
- **No audible test audio:** check receiver volume/mute and speakers, local
  networking and PTP port permissions. See [PTP_PORT_PERMISSIONS.md](PTP_PORT_PERMISSIONS.md)
  and [WAV_PLAYBACK_DIAGNOSTIC.md](WAV_PLAYBACK_DIAGNOSTIC.md). Do not change receiver
  selection or authentication merely because packet transmission succeeded.
- **Configuration changed during setup:** rerun and review the newer settings;
  the wizard refuses a stale save. Keep private config/state permissions intact.
- **Service unavailable/customized/path mismatch:** finish without activation.
  Review [SYSTEMD.md](SYSTEMD.md). Setup activates only the installed stock unit
  without drop-ins/overridden environment, resolving the account home to the same
  saved config. Custom paths and service overrides need deliberate manual review.
  If activation fails, inspect both unit enablement and status: enablement can
  have succeeded before a subsequent start/authorization error.

## Hardware-independent checks

The suite uses PTYs for terminal, invalid-input, EOF and signal tests; synthetic
advertisements for approval, selection, authentication and persistence; mock PCM
sources for levels and cleanup; and the existing encrypted loopback/virtual-clock
playback tests for packet integrity, EOF/fades, interruption and teardown. Tests
need no physical receiver, capture hardware, root or running systemd. CI retains
native ARM64 release compilation. These checks do not establish audible output,
real device compatibility or real systemd authorization on a Raspberry Pi.
