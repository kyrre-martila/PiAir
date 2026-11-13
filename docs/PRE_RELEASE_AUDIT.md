# 1.0.2 final pre-release audit

Scope: CLI dispatch/help; setup/menu/service/log presentation; config/allowlist
writers and protected receiver store; passive discovery and connection binding;
HAP/transient authentication boundaries; runtime, activity/capture and session
cleanup; installer transaction/recovery/uninstall and service state; packaging/CI;
current user docs and historical diagnostic examples. Fixes deliberately leave
protocol framing, authentication and installer security policy unchanged.

## Confirmed issues and regression coverage

| File / function | Trigger and impact | Severity | Fix and regression |
| --- | --- | --- | --- |
| `src/logging.rs::init`, `src/main.rs::main` | Ordinary setup/menu initialized JSON INFO logging; internal cleanup events clutter prompts. | Medium UX | Readable WARN+ for ordinary commands, DEBUG/TRACE with verbosity; JSON INFO+ for `run`. Actual emitted-output tests cover all six combinations, warning/error retention and INFO cleanup filtering; CLI process tests verify formatting. |
| `src/receiver/setup/mod.rs::run` | Completion recommended foreground automatic playback, but the installed binary has no capability for timing ports. | Medium operational | Recommend status and explicit installed service start; explain separate foreground permission and that saved settings do not prove sound. Permission feedback tests and documentation/CLI review cover the guidance. |
| `src/receiver/wire.rs::ptp_bind_error`, `setup::playback_feedback_details` | UDP 319/320 EACCES was presented as a speaker/network playback failure. | Medium operational | Typed local permission context overrides generic playback guidance. OS error/port remain in diagnostic chain. Tests inject both ports, preserve error classification, prevent audible-success questions; existing lifecycle test verifies TEARDOWN and resource release. Other bind errors do not recommend privileges. |
| `src/receiver/setup/daily.rs::setup_child` | `piair -v` / `-vv` launches setup from menu without forwarding verbosity. | Medium diagnostics | Propagate diagnostic level to child; regression checks default/debug/trace arguments and existing config path. |
| `src/receiver/setup/mod.rs` validation/authentication retry prompts | Normal retry messages exposed complete internal error chains. | Low UX | Actionable connection/validation guidance with DEBUG chain; verbose retains details and cleanup failures remain visible. Presentation regression checks both modes and cleanup errors. |
| `docs/SETUP.md`, README, runtime docs, `config.example.toml` | Instructions described superseded ID verification, approval prompts, advanced-setting screens, attribution display and unconditional restart/foreground steps; config comment incorrectly scoped discovery timeout to `devices`. | Medium documentation | Align with actual wizard and CLI; distinguish installed service, ordinary foreground executable and developer capability grant. Retain recording attribution, historical evidence and published-release installation warning. |
| Public network/device examples | Unnecessary usernames, private network addresses, room names and canonical device IDs exposed a real environment. | Low privacy | Use USERNAME, example names/IDs and TEST-NET addresses. Public repository attribution/links, historical results, command aliases and licenses remain intact. |

## Receiver binding decision

`Selection::matches_observation` intentionally keeps exact full address-set
approval. Legitimate IPv6 additions/removals/rotation can reject a still-reachable
IPv4 receiver. This availability limitation is accepted for 1.0.2; no trust is
broadened based on an unauthenticated advertisement. See
[the security analysis and explicit reselection workflow](RECEIVER_ADDRESS_BINDING.md).
Tests add IPv4-only, IPv6-only, dual-stack and rotation cases, alongside existing
changed-endpoint, duplicate-identity, conflict, retry and credential tests.

## Independent review findings

No further confirmed installer, audio or protocol bug was established that
justifies a code change in this pass. Existing coverage was retained for interrupted
install/update/uninstall, rollback, disabled/active service states, symlink/path
refusal, private ownership/modes, concurrent writes, missing input/receiver,
invalid credentials, cancellation, silence qualification, failed playback and
cleanup. Inspection and synthetic tests do not prove all real-world behavior.

Security invariants retained: no automatic root playback, capability grant,
sudoers/sysctl changes, receiver/authentication fallback or background activation;
normal system authorization; installed file hash/capability checks; transactional
maintenance; explicit approvals and endpoint selection; signed HAP identity
verification and revocation; transient identity limitations; private credentials,
no-clobber enrollment and checked atomic configuration writes.

## Validation and release gates

Run format, Clippy with warnings denied, locked Rust tests/release build, and the
complete Python suite. Exact final results and CI links belong in the PR report.
Live systemd installation tests need CI's running system manager; native ARM64
Bookworm package/ELF/interpreter/glibc <=2.36 and clean runtime checks must pass in
`release.yml`. These cannot be replaced with an x86-64 source build.

Follow [the executable Raspberry Pi plan](PRE_RELEASE_QA.md) for the isolated,
unpublished candidate on an existing 1.0.1 Trixie/UCA202/HomePod installation.
**Release blockers until manually recorded:** audible full recording, sustained
USB automatic playback, silence stop/restart, cancellation/resource cleanup and
configuration/credential preservation on the final candidate. Installed-candidate
stock service detection must also be rechecked after supported official installation;
an isolated executable must refuse to control the installed service unless its
actual executing file identity matches the trusted installed executable.
Do not claim production readiness from synthetic protocol success. Version stays
1.0.2; this work does not tag, publish or merge.

## Final review blockers corrected on PR #79

- **High operational safety:** `setup/service.rs` previously checked the stock
  unit and installed binary without checking the executable invoking control.
  An isolated candidate could therefore control the older installed service.
  Mutation now compares device/inode from an opened `/proc/self/exe` with an
  `O_NOFOLLOW` opened trusted `/usr/local/bin/piair`, retaining its descriptor
  through invocation. Symlink invocations of that same inode work; copies,
  replaced/deleted executables, missing/untrusted installations and unverifiable
  process identity fail closed. Existing unit, owner, environment, config and
  authorization checks remain. Gates cover direct start/stop/restart, menu stops
  and recovery starts, setup enable/start, and maintenance handoff. Read-only
  status/logs remain available and label installed service versus invoker.
  Tests cover same inode, symlink invocation, separate copy, replacement before
  and after inspection, missing executable, direct controls, menu actions and
  stop prompts, setup activation, and read-only candidate status. Foreground
  candidate capture/recording paths do not require installed-executable identity.
- **Medium QA correctness:** the original combined state checksum could be empty
  or confuse client identity with HAP credentials. The documented shell now uses
  independent credential/client/selection baselines, accepts a genuinely empty
  HAP baseline, and fails for missing baselines/state or deleted/modified saved
  credentials. Existing client identity is verified; initially absent identity
  may initialize; reviewed selection changes require private comparison. Private
  backups are made while stopped, and installed binary/unit/integrity-manifest
  hashes are checked separately. `scripts/test_manual_qa.py` executes the actual
  shell blocks from `/` on transient, saved-HAP, multi-receiver, identity,
  selection-change and missing/modified-file fixtures without exposing secrets.
- **Low documentation correctness:** the simplified receiver picker has friendly
  names, rescan and cancellation, with no technical-details menu. SETUP, candidate
  QA and address-reselection guidance now point to separate `receivers`/`devices`
  diagnostics; README, DAILY_USE and ADVANCED_USAGE state the same distinction.

No AirPlay authentication, packet encoding or transport timing changes are made.
File identity checks cannot authorize concurrent privileged replacement of the
installation; maintenance and administrative changes must remain serialized as
already documented. Final-commit CI results are recorded in the PR description.
Real Trixie/Pi/UCA202/HomePod listening, automatic playback, silence cycling and
cleanup/preservation checks remain manual release blockers. The isolated build
cannot verify successful installed-candidate systemd activation/capability grants;
recheck those only after supported official installation.
