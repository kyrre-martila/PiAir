# Receiver discovery, selection and diagnostics

`piair receivers` passively discovers mDNS advertisements and displays friendly
names, canonical IDs, services, capabilities and local approval/pairing state.
Names are labels and may repeat. Canonical IDs (`device:` plus twelve lowercase
hex digits) are authoritative; independently verify identity before allowlisting.
Discovery alone grants no permission and does not authenticate a receiver.

```sh
piair receivers
piair receiver-plan
piair allowlist add 'device:<canonical-id>'
piair select 'device:<canonical-id>' --method pin-display
piair hap-stream-setup-check 'device:<canonical-id>'
```

Replace placeholders with independently verified values. Allowlisting requires
its existing terminal confirmation. Pairing is separate: use `piair pairing`
with verified PIN delivery or a genuine HAP setup code. Diagnostics do not pair,
replace credentials or guess authentication methods.

Omit the diagnostic ID to use the same interactive picker as `stream-test`:

```sh
piair hap-stream-setup-check
piair stream-test --record-check --experimental-owntone-transient
piair stream-test --stream-setup-check --experimental-owntone-transient
```

The picker shows names and canonical IDs, asks for an explicit method for a new
selection (PIN-display or setup-code for a paired HAP receiver), and validates approval
and capabilities. Enter `q`, EOF or an invalid choice to cancel without contact
or selection writes. The final y/yes operation confirmation persists a provisional
selection and its endpoint pin, rechecking approval, credentials and observation.
Declining leaves state unchanged. A confirmed selection remains if the operation
later fails. Changing a pinned receiver requires explicit review and reselection.

With an explicit ID, `stream-test` requires that ID's persisted selection and
method. HAP commands retain their existing operation-scoped endpoint and
credential pin: the explicit ID is validated from discovery without changing
the default selection. Picker-driven HAP commands pin the confirmed receiver observation and derive
operation-only Stored HAP authentication from its existing validated credentials;
the persisted enrollment method is retained.
Both paths bind authentication, RTSP, event/timing/media endpoints and cleanup
to the same canonical ID. Approval and credential revocation remain enforced.
Unsupported capabilities, changed pairing identity, ambiguous shared addresses,
endpoint drift and credential mismatch fail closed with an error; no fallback.

`hap-stream-setup-check` requires stored persistent HAP, audio/ALAC and realtime
NTP support. It performs pair-verify, initial SETUP, event connection, RECORD,
stream SETUP and TEARDOWN, with zero audio, sync or volume changes. Legacy
`stua-stream-setup-check` remains an alias. Transient diagnostics require an
explicit transient method and `--experimental-owntone-transient`; session proofs
do not establish persistent identity. RECORD-only stops before stream SETUP.

Playback is separately confirmed and may leave receiver volume changed:

```sh
piair hap-playback-check 'device:<canonical-id>' --volume-db -20
piair stream-test --playback-check --experimental-owntone-transient
```

HAP volume is an integer from -40 to -10 dB (default -40). The transient diagnostic
uses fixed -40 dB. Both send a one-second faded tone and two-second silent drain;
packet transmission does not prove sound. `stua-playback-check` remains an alias.
No permissions, timing requirements or media readiness gates are expanded.

`buffered-test 'device:<canonical-id>' --setup-check` and its staged
`--control-check record|stream|anchor` workflow retain the existing explicit ID,
persisted selection, pairing and per-stage confirmations. Buffered live playback
is still blocked pending clock-adoption evidence. PR #51's tokenless authenticated
sessions, RTSP stage order, AEAD/CSeq checks, PTP validation and bounded TEARDOWN
remain unchanged. Invalid or revoked transport can prevent remote TEARDOWN;
local resources are released, but remote cleanup is not claimed as acknowledged.

Validation uses synthetic discovery and loopback receivers only. No physical
receiver contact, Raspberry Pi service changes or live playback are part of this
change. Hardware compatibility and audible output remain unverified.
