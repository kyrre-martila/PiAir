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
4. Select your AirPlay speaker by its advertised name. The simplified picker
   offers friendly names, rescan and cancellation. Technical details are available
   separately with `piair receivers` or `piair devices`, not from the picker. A new speaker needs one explicit approval prompt;
   its advertised name and device ID do not verify its identity. The connection
   choice is saved separately after confirmation. See
   [receiver address binding](RECEIVER_ADDRESS_BINDING.md) if discovery changes.
5. Choose an advertised compatible authentication path. Existing validated HAP
   credentials can be reused without enrollment. PIN-display enrollment requires
   actual supported PIN delivery. HAP setup-code enrollment requires a genuine
   independently obtained code; an AirPlay password is not a HAP setup code.
   Codes are entered without echo. OwnTone-reference transient authentication
   requires explicit confirmation and does not establish verified persistent
   receiver identity. There is no automatic authentication fallback.
6. Normal setup keeps automatic detection and the current/default volume.
   Advanced settings are optional. They include input measurement, activity
   thresholds and explicit volume changes; see [runtime settings](RUNTIME.md).
   Defaults are -45 dBFS, 1000 ms sustained audio to start and 30000 ms silence
   to stop. Verify them with your source; they are not universal recommendations.
7. Review the input, speaker, connection, detection and volume summary. Save only
   when satisfied. The atomic writer retains TOML comments, unrelated settings,
   approval, credentials, ownership and file permissions. Saving does not prove
   audible playback.
8. Optionally enable and start the installed stock service for your current
   account. No is the default. Setup displays the account and saved path and asks
   for confirmation. Normal systemctl/polkit authorization applies.
9. Independently play or skip the optional recording (Skip is the default).
   Declining boot startup does not prevent testing. The test uses the just-saved
   configuration and selected authentication method, with no fallback or receiver
   substitution. It always preserves the receiver's current volume, including
   when an explicit automatic-playback volume is saved. It plays at most the
   existing 49 seconds through the existing PTP/ALAC/RTP sender. It tests speaker
   output, **not ALSA input**. After completed transmission, answer whether you
   heard it; packets alone never establish audibility. A negative answer or test
   failure retains the saved setup and provides troubleshooting.
10. Setup displays the saved speaker, audio input and connection method, plus
    the observed automatic startup state. Only systemd’s persistent `enabled`
    state is displayed as Enabled. Declining startup leaves existing enablement
    unchanged; an unavailable query displays Unavailable rather than guessing.
    The audio test is Passed only after you confirm hearing the recording,
    Failed after a transmission failure or a negative listening answer, and
    Skipped when skipped, unavailable or left unconfirmed. Failure guidance is
    shown with the test result. Setup completion alone makes no audibility claim.

Example completion (the values come from your saved settings and systemd):

```text
Setup complete.

Speaker: Living room
Audio input: plughw:CARD=USB,DEV=0
Connection: Without pairing
Automatic startup: Enabled
Audio test: Skipped

Next: Play audio through your connected input.
Use piair status to check PiAir, or piair start when ready.
```

### Confirmations deliberately retained

All Yes/No prompts default to No. Numbered confirmations use **1. Yes** and
**2. No [default]**; q, EOF and Ctrl+C cancel without approval.

- **Use “NAME” as your speaker?** saves approval of a previously unapproved
  canonical speaker identity. Discovery is untrusted; the friendly name does not
  establish identity. The picker never approves anything by itself.
- The PIN/setup-code prerequisite confirms access to the actual speaker’s PIN
  or independently obtained HAP setup code. They are not AirPlay passwords.
- **Connect without pairing for this session?** consents to transient
  authentication, which does not establish verified persistent identity.
- **Save this speaker and connection method?** persists the selection, even if
  later setup is cancelled. It is separate from speaker approval.
- **Pair with this speaker and save the pairing?** authorizes verified credential
  persistence. Credentials are still saved only after cryptographic verification.
- **Save settings** at review authorizes replacing existing playback settings.
  Explicit volume changes retain their own confirmation because speaker volume
  may remain changed after playback.
- Automatic startup remains two choices: whether to configure startup, then
  whether to **enable automatic startup and start PiAir now**. The latter has
  the additional immediate-playback consequence and uses normal system authorization.
- Interrupting running playback and starting it again require separate consent.
  Playing the recording and confirming that it was heard are different decisions.

Technical identifiers remain in `piair receivers` and `piair devices`. Use
`piair -v setup` for original error context and `piair -v status` for service
and configuration diagnostics.

## Service-backed test and recovery

The installed service runs automatic capture, not the recording. Testing creates
one randomly named `piair-recording-USER-UUID.service` using `systemd-run`, with
normal system authorization. It runs only the installed executable's fixed
`service-recording --saved FINGERPRINT` operation as the current non-root user.
The temporary unit copies security restrictions and CAP_NET_BIND_SERVICE from
the compiled stock template, has no restart policy or boot enablement, and has a
100-second total runtime backstop plus the existing 30-second stop timeout.
No ExecStart overrides, file capabilities, sudoers or sysctl changes are used.

The wizard verifies the installed executable, unmodified root-owned template,
account and default config path through existing service integrity checks. The
worker holds the existing per-user runtime lock and configuration-writer lock,
and rejects a changed config fingerprint before playback. Another user's active
PiAir service is a conflict; it is never stopped. Foreground runtimes must be
stopped by their owners. Timing-port contention fails safely without killing
other processes or deleting locks.

A stopped service stays stopped and its boot enablement stays unchanged. If your
service is running, testing asks before interrupting it, then separately offers
to restore automatic playback. Declining interruption skips testing. Ctrl+C, q
or EOF cancels the test with bounded teardown and never silently restarts playback.
Missing recording/attribution, missing/customized service, wrong executable/config
path or authorization denial makes the optional test unavailable; setup can finish.
On uncertain teardown, PiAir reports the exact temporary unit and suppresses
recovery startup. Inspect it with `systemctl status UNIT` and ask an administrator
if authorization prevents stopping it. The manager lifetime bound also covers a
killed/disconnected wizard; SIGKILL and a failed system manager cannot guarantee
immediate cleanup. Temporary units are collected after completion.

The ordinary installed executable has no file capabilities and setup must not run
as root. Separate foreground developer diagnostics still require explicit timing
port permission; see [PTP permissions](PTP_PORT_PERMISSIONS.md).

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
asks before stopping its verified current-account service and never stops another process or another user's service. The lock stays held until
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
for source, modifications and licensing. The packaged LICENSE.md retains this attribution alongside the recording;
the simplified wizard does not display it before playback. No additional tone, recording or sender is created for setup.

The installed service-backed test requires both `Pachelbel_Canon_audio_test.wav`
and `LICENSE.md` in `/usr/local/share/piair/audio`. Reinstall PiAir if either is
missing. Service execution deliberately ignores foreground environment overrides
and development-checkout data. Standalone developer diagnostics retain their explicit audio-file options.

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
