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
selection and the explicit authentication argument below. It provides no verified
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
when PiAir exits. Omitting volume preserves it. Use `--authentication stored-hap`
for an already paired receiver selected with a persistent method. There is no
inferred authentication profile. The threshold and duration values above are
also the run command's defaults; capture device and receiver remain explicit.

`--allow-unattended` is required on each `run` invocation, applies only to its
receiver/settings, and bypasses no other command's terminal confirmation. It is
not saved as a global permission. Successful validation emits a JSON
`unattended_authorized` event with receiver, profile, device, detection and volume
settings. Neither a TTY nor root is required. Existing Linux PTP port permissions
still apply; see [PTP_PORT_PERMISSIONS.md](PTP_PORT_PERMISSIONS.md). This command
does not grant capabilities or change system permissions.

The existing TOML configuration and private receiver-state files remain compatible.
No new persistent configuration format or environment-based permission is added.
Persistent configuration unification is deferred to PR #70.

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
- **1**: configuration, authorization, capture or runtime failure, signal-handler
  registration failure, or unsuccessful bounded teardown during cancellation.
- **2**: CLI syntax/argument error, including missing unattended opt-in or profile.

Recoverable session failures are logged and retried while the runtime remains
active. Lifecycle events (`monitoring`, `authentication`, `negotiation`, `playback`,
`session_stopped`, `retry`, `shutdown`) use existing JSON stderr logging. Existing
audio/session diagnostics may also appear on stdout. Logs do not establish audible
playback or receiver clock synchronization.

The process stays in the foreground. This prepares a future systemd caller;
**no systemd service, installer, daemonization, PID file or system modification is
provided**. Synthetic tests cover orchestration, authorization, cancellation and
cleanup without audio hardware. They do not replace validation on the Pi and
approved receiver.
