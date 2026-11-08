# Interactive CLI verification (PR #30)

These are operator commands for a Raspberry Pi **after merge**, not evidence of
hardware testing. Do not stop or modify OwnTone/autostream. Run contact tests
only when the intended receiver is available and you explicitly want the stated
operation. Audible playback remains unverified.

From the PiAir checkout, update and validate:

```sh
git fetch origin main
git switch main
git pull --ff-only origin main
git rev-parse HEAD
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
git diff --check
```

Use your existing config (replace the path below if necessary); keep the same
path throughout so receiver-state and approval remain consistent:

```sh
export PIAIR_CONFIG="$HOME/.config/piair/config.toml"
./target/release/piair config check
./target/release/piair receivers
./target/release/piair pairing
```

Verify names, stable IDs and endpoints independently. If unapproved, edit the
existing `receiver_allowlist` manually and rerun `config check` and `pairing`.
Choose the documented pairing method for that receiver; do not guess a setup
code or substitute transient authentication for persistent pairing. Confirm only
when you want to contact that receiver. An already-paired result makes no
connection and does not prove current authentication.

Set the exact ID printed by `receivers` (the shell prompt here is only for
constructing the advanced commands):

```sh
printf 'Receiver stable ID: '
read -r PIAIR_RECEIVER_ID
./target/release/piair authenticate "$PIAIR_RECEIVER_ID"
./target/release/piair stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile a --volume-db -15
./target/release/piair stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile b --volume-db -15
./target/release/piair stream-test "$PIAIR_RECEIVER_ID" --setup-check --setup-profile c --volume-db -15
```

Confirm each setup check separately. Expect zero audio packets, unchanged
receiver volume and acknowledged teardown. Authentication success does not
establish playback readiness. A failed setup check is not playback success.

Playback checks below set receiver volume, which **may remain changed**:

```sh
./target/release/piair stream-test
./target/release/piair stream-test --seconds 3 --volume-db -15
./target/release/piair stream-test "$PIAIR_RECEIVER_ID" --seconds 3 --volume-db -15
```

Each command requires its own explicit y/yes confirmation. Check receiver, ID,
duration and volume before authorizing. Observe negotiation, transmission and
teardown separately; record audible results independently. The signal lasts
1–3 seconds plus two seconds of latency drain. Do not assume audio was audible
because packets were sent.

Cancellation checks: enter `q`, EOF (Ctrl+D) or Ctrl+C at a chooser; enter an
empty line, `n`, `no` or another invalid answer at confirmation. Expect a failure
exit status and no receiver contact. Receiver and method choices are provisional:
cancellation must leave selection, pinned endpoints, credentials and the allowlist
unchanged. Final y/yes confirmation authorizes saving the selection immediately
before contact, after local validation. If the subsequent connection, authentication
or playback fails, that explicitly authorized selection remains saved; no stale
selection is restored. A concurrent selection change causes an explicit failure
instead of overwriting it. An existing pairing must remain intact. No `--yes` bypass exists.

Illustrative sessions (not hardware results):

```text
$ piair pairing
Available AirPlay receivers:

  1. Living Room  [Not paired]
Select receiver (1-1, q to cancel): 1
Choose an explicit method: 1. PIN-display; 2. Existing HAP setup code; no fallback. Selection is provisional until final confirmation.
Select method (1-2, q to cancel): 1
Receiver: Living Room
Device ID: device:aabbccddeeff
Operation: Pair and save verified credentials (no audio).
Confirming saves device:aabbccddeeff with method PinDisplay and its pinned endpoint as the active receiver selection. It remains selected if the subsequent operation fails.
Continue? [y/N]: n
[receiver operation cancelled; no pairing performed]

$ piair stream-test --seconds 3 --volume-db -15
[choose an approved receiver; existing selection and credentials checked]
The test includes a 2-second latency drain. Volume may persist after playback. Audible playback is unverified.
Receiver: Living Room
Device ID: device:aabbccddeeff
Operation: Playback test
Duration: 3 seconds
Volume: -15 dB
Start playback test? [y/N]: n
[receiver operation cancelled; no playback performed]
```

Use `piair receivers` for full technical details: full friendly name and stable
ID, IPv4/IPv6 addresses, advertised service types and ports, approval, selection
and local pairing. Compact rows use `[Not approved]` before any other status;
approved receivers use `[Paired, selected]`, `[Paired]`, `[Selected]` or
`[Not paired]`. Stored credentials never imply current authentication or audible
playback. Duplicate names include stable IDs, and long names are shortened only
in the compact rows. Verify identity independently before approval or contact.
