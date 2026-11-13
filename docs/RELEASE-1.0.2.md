# PiAir 1.0.2: first-time setup

## Hardware evidence and diagnosis

A clean Raspberry Pi running Debian 13 Trixie successfully installed official
v1.0.1. The stock `piair@kyrre.service` was loaded, inactive and disabled, with
`FragmentPath=/etc/systemd/system/piair@.service`, `User=kyrre`,
`WorkingDirectory=~` and the expected stock command. `systemctl show` emitted
`Environment=` and `DropInPaths=` but omitted empty `EnvironmentFiles` entirely.
The Rust wizard incorrectly required that array property to be printed. Like the
v1.0.1 Python maintenance helper, it now treats an omitted EnvironmentFiles array
as empty. Scalar properties still must be present. Nonempty environment files,
environment variables and drop-ins remain rejected. Exact root-owned stock
unit content, executable ownership, account and config path checks remain.

HomePod Stua (`device:0a05559f132f`, `192.168.1.119`) was discovered, explicitly
approved and selected for session-only authentication. The bundled playback test
reported “selected receiver PTP authentication failed; no fallback: receiver not
discovered”, with zero RTP transmission. A later `piair devices` found it again.
The test takes a separate discovery snapshot; with the default five-second
window, that snapshot lacked the selected ID. Wire::connect_authorized rejected
it before dialing/authentication. This establishes missing fresh discovery, not
bad pairing or a rejected SRP proof. The evidence does not establish why mDNS
was temporarily incomplete on that network.

Playback now takes up to three fresh discovery snapshots (at most three configured
discovery windows, normally 15 seconds). Only absence of the selected receiver
is retried. Discovery errors, identity conflicts, duplicate IDs, altered endpoint
or capability bindings fail immediately. A successful fresh snapshot is validated
before authentication and checked again by Wire immediately before connecting.
Cancellation is checked before and after every discovery attempt, so discovery's
partial result on cancellation cannot authorize a connection. Discovery failures
say authentication has not started. No old endpoint snapshot is used as a
substitute and no authentication or transport fallback is added.

## First-time experience

The wizard presents audio input, speaker, connection, audio test, final review
and optional automatic startup. Detection and volume defaults are retained;
manual detection, measurement and volume controls are optional advanced settings.
When both `hw:` and the equivalent `plughw:` ALSA hint exist, the normal list
prefers conversion. Exact manual ALSA entry and `piair audio devices` remain
available. Speaker selection shows friendly names; `piair devices` and verbose
diagnostics retain technical information. Approval is one explicit confirmation
of the friendly name, never automatic and never a request to verify a canonical
ID physically. Canonical validation and exact allowlisting remain internal.

Connection labels use plain language. Session-only authentication still does not
verify a persistent speaker identity or save pairing credentials. Persistent
pairing still requires the actual supported PIN/setup code and cryptographic
credential verification. Installed recording attribution and license files are
unchanged. Test narration omits protocol and licensing details; diagnostic
commands and verbose logging retain details. Obsolete installer plans and the
unconditional instruction to restart a nonexistent service are removed.

Safe checked config writes, separate confirmed approval/selection persistence,
runtime exclusion, revocation checks, cleanup failure handling and explicit
service enable/start authorization remain. No unrelated transport changes,
release tags or published release modifications are included.

## Validation and hardware follow-up

Regression coverage includes real systemctl empty-array output, overrides and
missing scalars; transient missing discovery then success; persistent absence;
cancellation before/during discovery; conflicting and duplicate identities;
changed endpoint/capabilities; friendly-only selection; single approval; normal
defaults and advanced adjustments; manual ALSA entry; verified stored credentials;
no fallback; and explicit service activation.

Run formatting, Clippy with warnings denied, Rust unit/integration tests and the
Python/systemd suite. Existing PR workflows build/package on native ARM64
Bookworm, verify ELF dependencies and glibc <= 2.36, and start the package in a
clean Bookworm runtime. These checks do not establish audible hardware playback.
Re-test on the Trixie Pi with HomePod Stua: complete both normal/advanced setup,
stock inactive-service acceptance, transient playback with audible confirmation,
code pairing where supported, cancellation/cleanup, and explicitly authorized
boot startup. Confirm actual microphone/line-in behavior and silence detection.

## Remaining UX review follow-up (PR #78)

The standalone `allowlist add` command now asks once to approve the exact
canonical ID, without requiring a physical device label or local-settings lookup.
The command still validates IDs before prompting, never discovers or approves
receivers automatically, defaults to denial on any unconfirmed/incomplete input,
and uses the existing snapshot/concurrent-write protection. Approval grants
connection eligibility, not cryptographic receiver identity verification.

Playback errors carry typed discovery, connection and playback presentation
stages. Normal setup explains what to check without dumping protocol names,
network port numbers or the internal error chain. Verbose (`-v`/`-vv`) setup and
logging retain the original nested diagnostics. A discovery validation rejection
is never described as failed authentication. Successful local transmission still
requires a separate answer about audible sound. Serious cleanup errors retain
their original error type and details and stop setup, including on cancellation.

The primary ALSA list groups only recognized, explicitly parameterized routes
using exact CARD/DEV identifiers. It chooses an advertised `plughw` PCM (or the
advertised `hw` PCM when no converting PCM is advertised), then validates that
exact choice live. This is a suitable input candidate, not a claim that ALSA
aliases are equivalent. Failure never switches to another alias. Separate card
and device numbers stay separate; Bluetooth, virtual and nonstandard routes stay
available. Similar friendly names never establish grouping. All original PCM
identifiers, manual entry and saved-input reuse remain in advanced options.
Unknown parameters such as SUBDEV are never folded into a different route.

Focused regressions cover single-confirmation approval, help and cancellation,
stale writes, normal/verbose/trace stage-specific playback output, cleanup errors,
multiple sound cards with identical names, all six UCA202 aliases, multiple
hardware devices, loopback, Bluetooth, nonstandard parameters, advanced/manual
selection and validation failure without substitution. The earlier systemd,
fresh-discovery binding, credential and wizard regressions remain in place.
