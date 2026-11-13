# Automatic playback runtime and unattended operation

`receiver::runtime::Runtime` owns automatic orchestration independently of terminal
interaction. `Settings` carries the exact receiver ID, ALSA capture device,
activity settings, explicit authentication profile and optional receiver volume.
`prepare` validates and pins existing local state; `run` accepts a shared
cancellation flag. Callers own signal registration and interactive authorization.
The runtime never reads stdin, chooses a receiver, pairs, or writes configuration
or selection. The existing receiver client identity is reused (and initialized
by the existing wire implementation on first connection if absent).

`automatic-ptp-capture` retains terminal confirmation before calling this runtime.
All experimental command names remain available. Manual capture and WAV playback
continue to use the same session implementation and timing policies.

## Explicit unattended invocation

First explicitly approve and persistently select the intended receiver through
PiAir's existing allowlist and selection workflows. Review its stable ID, endpoint,
capabilities and access policy. Persistent HAP requires existing valid enrolled
credentials. OwnTone transient requires an explicit saved `--method transient`
selection and an explicit authentication profile from CLI or `[playback]`. It provides no verified
persistent receiver identity and is never a fallback for failed HAP authentication.

For the verified UCA202 configuration, substitute the exact approved receiver ID:

```sh
piair --config /path/to/config.toml run device:0123456789ab \
  --allow-unattended \
  --capture-device 'plughw:CARD=CODEC,DEV=0' \
  --authentication own-tone-transient \
  --threshold-dbfs -45 \
  --start-duration-ms 1000 \
  --stop-duration-ms 30000
```

Optionally add `--volume-db -20`; this changes the receiver's volume and may persist
when PiAir exits. Omitting volume preserves it when no volume is stored; `--preserve-volume` overrides a stored volume. Use `--authentication stored-hap`
for an already paired receiver selected with a persistent method. There is no
inferred authentication profile. The threshold and duration values above are
also the retained runtime numeric defaults, not universal recommendations; device, receiver and authentication must be supplied explicitly in CLI or TOML.

`--allow-unattended` is required on each `run` invocation, applies only to its
receiver/settings, and bypasses no other command's terminal confirmation. It is
not saved as a global permission. Successful validation emits a JSON
`unattended_authorized` event with receiver, profile, device, detection and volume
settings. Neither a TTY nor root is required. Existing Linux PTP port permissions
still apply; see [PTP_PORT_PERMISSIONS.md](PTP_PORT_PERMISSIONS.md). This command
does not grant capabilities or change system permissions.

## Persistent playback configuration

`[playback]` is additive; existing `[audio]`, approval and private receiver-state
files remain compatible without migration or re-enrollment. No secrets, endpoint
pins or credentials belong in this table. The ID references the approved,
persistently selected receiver; it never establishes trust.

| Key | Default / requirement | Validation |
| --- | --- | --- |
| `capture_device` | Required explicitly | Nonblank ALSA identifier, <=256 bytes, no controls |
| `receiver_id` | Required explicitly | Canonical `device:<12 lowercase hex digits>`, no zero/broadcast |
| `authentication` | Required explicitly | `stored-hap` or `own-tone-transient`, matching selected profile |
| `threshold_dbfs` | -45.0 | Finite -120..=0 dBFS |
| `start_duration_ms` | 1000 | 20..=9000 ms |
| `stop_duration_ms` | 30000 | 20..=120000 ms |
| `volume_db` | Absent: preserve receiver volume | Integer -60..=-10 dB |

See `config.example.toml` for a Behringer UCA202 / approved HomePod example.
These are proven values for one setup, not general hardware recommendations.
Automatic RMS windows and qualification semantics remain unchanged (20 ms windows;
start and stop are observed at window boundaries).

Save using the existing config command hierarchy:

```sh
piair config set-playback device:0123456789ab \
  --capture-device 'plughw:CARD=CODEC,DEV=0' \
  --authentication own-tone-transient
piair config set-playback --threshold-dbfs -48 --volume-db -20
piair config set-playback --preserve-volume
piair config check
piair config show
piair run --allow-unattended
```

Replace the placeholder ID; approve and select it first using existing workflows.
The scriptable setter patches omitted values from the current file and validates
all effective settings and local authorization before saving, with no receiver
contact, discovery, audio, pairing, approval or selection writes. Unknown/invalid
settings cause refusal with the original file intact. TOML comments, unrelated
values and allowlist entries are retained. `config show` displays effective values
without writing; absent required fields remain absent.

Explicit `run` CLI values override stored `[playback]` values, which override the
numeric defaults above. `--preserve-volume` overrides stored volume for one run.
An explicitly provided positional receiver ID overrides the stored ID but must
still match current approval and persistent selection. Missing required settings
fail before capture or discovery. Authentication never falls back.

All configuration writers (setter, allowlist and calibration) share a nonblocking
lock on the parent directory inode, held through snapshot comparison and atomic
publication. A same-directory private temporary file is flushed and renamed, then
the directory is synced. Existing owner/group and permissions are preserved;
incompatible ownership, read-only files, hard links, symlink files/ancestors and
unsafe writable parent directories are rejected. Missing directories are created
privately. Writers detect changed content and inode/metadata and ask the operator
to rerun. External editors must honor this directory lock to serialize the final
check/rename boundary; arbitrary noncooperating same-user edits cannot be made
transactional. A directory sync error can mean the file was saved but durability
was not confirmed. Receiver-state files are never rewritten by configuration saves.

`[audio]` remains authoritative only for legacy `audio monitor` and `audio calibrate`.
It has separate start/stop thresholds and different hysteresis semantics;
calibration does not configure automatic playback. No implicit conversion is made.
Choose and test automatic thresholds explicitly on your hardware.

Operational settings are fixed at invocation startup. Restart explicitly to apply
changes to device, threshold, durations, volume or playback profile/ID. This does
not cache authorization: removal of approval, selection changes and stored HAP
credential revocation still terminate via existing runtime and wire/media checks.
Invalid configuration during execution also fails closed. Saving cannot authorize
unattended playback; `--allow-unattended` is mandatory every time.

## Concurrent automatic instances

Both `run` and `automatic-ptp-capture` acquire a nonblocking, kernel-managed
process lock before opening ALSA or connecting to a receiver. Interactive automatic
capture reserves it before discovery and confirmation. Only one automatic instance
per effective user ID in a Linux network namespace can run, even with different
configuration paths, capture devices or receiver selections. A second invocation
exits with code 1 and asks the operator to stop the existing process before retrying;
it cannot cancel, signal, unlock or otherwise interrupt the owner.

The lock is an exclusively bound Linux abstract Unix socket named
`piair.automatic.<effective-uid>`, held until capture and session cleanup finish.
It accepts no commands or connections in application code, creates no filesystem
objects, follows no paths, and changes no receiver-state permissions or authorization.
Its descriptor is close-on-exec. Closing the process's descriptor releases the
binding, including after errors, SIGINT/SIGTERM and uncatchable termination such
as SIGKILL. There are no PID files or stale-lock removal operations. Kernel release
after SIGKILL does not imply that remote TEARDOWN was sent; remote expiry remains
unconfirmed. This is per-user automatic-mode exclusion, not coordination between
different Unix accounts/network namespaces or with manual playback commands.

## Authorization and failure behavior

The invocation pins the saved selection. Approval, selection and (for stored HAP)
credential validity/binding are rechecked while monitoring, before attempts, and
through the existing wire and PTP/media authorization leases. Stored-HAP runtime
leases additionally retain the persisted-selection pin, while existing diagnostic
operation scopes keep their behavior. Discovery must match the pinned identity,
endpoint and capabilities. Authenticated identity and receiver policy checks,
PTP negotiation and media gates remain mandatory. Revoked approval, changed
selection or invalid/removed credentials terminate the runtime. No receiver or
method is substituted and no approval, pairing or selection is created.

Sustained input qualifies playback; silence stops the session, drains the existing
media timeline, and returns to monitoring with ALSA still open. New sustained
audio starts a new session. Session failures retain the existing 2, 4, 8, 16, 32,
60 second capped exponential backoff and require fresh sustained audio. Silence
never redials. Unrecoverable capture errors terminate and release resources.
See [LIVE_CAPTURE.md](LIVE_CAPTURE.md) for RMS, pre-roll, overflow, starvation,
XRUN and silence/drain details. No codec, RTP or PTP engine is duplicated here.

## Shutdown and exit codes

SIGINT and SIGTERM set the same shared cancellation flag in the foreground
process, including without a TTY. Idle/retry monitoring checks at 20 ms intervals;
discovery checks at 10 ms intervals and retains its bounded one-second mDNS
shutdown acknowledgement. Connecting and in-flight control calls retain existing
bounded deadlines (TCP connect up to three seconds; control socket waits up to
three seconds, or shorter polling intervals). Cancellation stops media rather
than promising a complete graceful drain. Existing bounded TEARDOWN uses a
250 ms request budget; remote acknowledgement/expiry is reported separately from
local resource release. A failed teardown remains an error even during cancellation.
The capture owner stops and joins its nonblocking ALSA worker; session ownership
reclaims feedback, timing and event workers and releases sockets and keys. ALSA
source factories must return and reads/recovery must remain nonblocking; external
ALSA plugins that block inside open/configuration cannot be given a hard wall-clock
shutdown guarantee by an in-process thread join.

- **0**: orderly runtime cancellation, including SIGINT/SIGTERM. Interrupted
  authentication/control errors are logged; cleanup failures remain nonzero.
- **1**: another automatic instance already running, lock acquisition failure,
  configuration, authorization, capture or runtime failure, signal-handler
  registration failure, or unsuccessful bounded teardown during cancellation.
- **2**: CLI syntax/argument error, including missing unattended opt-in or invalid profile syntax. A missing profile in both CLI and config is a configuration error (1).

Recoverable session failures are logged and retried while the runtime remains
active. Lifecycle events (`monitoring`, `authentication`, `negotiation`, `playback`,
`session_stopped`, `retry`, `shutdown`) use existing JSON stderr logging. Existing
audio/session diagnostics may also appear on stdout. Logs do not establish audible
playback or receiver clock synchronization.

The process stays in the foreground. The optional [systemd template](SYSTEMD.md)
supervises this same `run --allow-unattended` command and existing persistent
configuration. Interactive approval, selection and pairing remain separate
preparation steps. Systemd provides boot startup, journal logging, bounded restart
policy and SIGTERM shutdown; installation never starts or enables it automatically.
No installer, daemonization or PID file is provided. Synthetic tests cover
orchestration, authorization, cancellation and cleanup without audio hardware. Process tests use synthetic ALSA `null` capture;
active SIGINT/SIGTERM tests use mock PCM and loopback PTP/RTP/control sockets on
unprivileged ports, verify packet transmission before cancellation, joined workers,
rebindable sockets and visible bounded-teardown failures. They do not replace
validation on the Pi and approved receiver.
