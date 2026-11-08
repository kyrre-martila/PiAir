# Realtime RECORD latency and sender synchronization

This supersedes the diagnostic-only decision in PR #27. Base: fresh
`origin/main` c4581b03c3c34b315e48df833a6f0e278ab2b148 (merged #27).
Reported hardware evidence: Apple TV Stua, AppleTV6,2, tvOS 26.6.1 completes
verified authentication, encrypted initial SETUP, early NTP, RECORD 200 with
`Audio-Latency: 0`, and encrypted cleanup TEARDOWN. The old RECORD range check
stopped negotiation before stream SETUP; no audio had been transmitted.
These are user-supplied observations, not playback verified by this PR.

## Pinned OwnTone data flow

Reference is OwnTone 29.3 commit
[`d6fb3edf5831de38134ebd92fcf09a730ddd37aa`](https://github.com/owntone/owntone-server/tree/d6fb3edf5831de38134ebd92fcf09a730ddd37aa).
The following are calculations in executed code, not inferred from packet
example comments. No OwnTone implementation code was copied.

| Source | Calculation / behavior |
| --- | --- |
| [`airplay.c:3031`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L3031) `response_handler_record` | Sets RECORD state and continues; no Audio-Latency parsing, validation or assignment. |
| [`airplay.c:3652`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L3652) startup sequence | RECORD response precedes stream SETUP. |
| [`airplay.c:2628`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2628) stream offer | latencyMax=88200, latencyMin=11025; separate from RECORD. |
| [`conffile.c:79`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/conffile.c#L79), [`outputs.c:1332`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs.c#L1332) | Configured start_buffer_ms defaults to **2250**; read into outputs_buffer_duration_ms. |
| [`airplay.c:1196`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L1196) master session | Require buffer duration >250 ms; output_buffer_samples=(buffer_duration_ms−250)×sample_rate/1000. At default 2250 ms and 44100 Hz: **88200** samples. |
| [`airplay.c:2129`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2129) `timestamp_set` | Save player pts as cur_stamp.ts; cur_stamp.pos=next RTP pos+unsent input_buffer_samples−output_buffer_samples. |
| [`airplay.c:4235`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L4235) `airplay_write` | Set timestamp and send sync **before** adding incoming PCM. Then append input, consume complete 352-frame blocks, send and commit packets. Input remainder is included in the next timestamp calculation. |
| [`airplay.c:2158`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L2158) `packets_sync_send` | Apply optional user offset; initial 0x90 and later 0x80 use the same calculated stamp. |
| [`rtp_common.c:311`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/rtp_common.c#L311), [`rtp_common.c:347`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/rtp_common.c#L347) | NTP d4 sync: bytes 4..8=cur_stamp.pos, 8..16=NTP(cur_stamp.ts), 16..20=next RTP pos; flags 00 07. No extra latency subtraction in packet construction. Comments about receiver latency are explicitly tentative. |
| [`rtp_common.c:186`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/rtp_common.c#L186) | Packet commit advances RTP pos by packet sample count using u32 modular arithmetic. |
| [`rtp_common.c:90`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/rtp_common.c#L90), [`rtp_common.c:229`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/rtp_common.c#L229) | Zero sync interval selects sample_rate; packet commits increment the sync counter. Sync becomes due when the counter exceeds that rate, and is checked on player writes. PiAir checks on packet deadlines, about once per second. |
| [`airplay.c:544`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/outputs/airplay.c#L544), [`player.c:873`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/player.c#L873), [`player.c:1352`](https://github.com/owntone/owntone-server/blob/d6fb3edf5831de38134ebd92fcf09a730ddd37aa/src/player.c#L1352) | Timing replies use CLOCK_MONOTONIC; player pts starts on that clock and advances by calculated sample/tick durations. Both NTP conversions add the 1900 epoch delta and scale fractions by 2^32. |

## PiAir policy and arithmetic

PiAir retains its existing **2000 ms total playback lead**, rather than adopting
OwnTone's configurable default of 2250 ms. This is explicit sender policy, not a
fallback for any header and not a consequence of latencyMax. At 44100 Hz:

- Total lead / silence tail: `PLAYBACK_LEAD_SAMPLES = 2 × RATE = 88200`.
- Stream latency offer component: `STREAM_LATENCY_SAMPLES = RATE / 4 = 11025`.
- Sender sync offset: `SYNC_OFFSET_SAMPLES = 88200 − 11025 = 77175` (1750 ms).

This is OwnTone's buffer calculation with a 2000 ms configuration. It preserves
PiAir's previous timing for absent/11025 RECORD headers, and applies the same
model for every header. It does **not** claim to reproduce OwnTone's default
2250 ms behavior or establish actual receiver render latency.

PiAir generates one complete ALAC block on demand, so at each sync there is no
unsent PCM remainder. Given starting RTP timestamp R, S frames already sent,
monotonic start T, and one shared session clock C:

```
next RTP       = R + S                         (mod 2^32)
sync position  = next RTP - 77175              (mod 2^32)
sync NTP       = C(T + sample_time(S))
audio deadline = T + sample_time(S)
```

The sync position corresponds to the scheduled sender time. The receiver-side
250 ms stream component is accounted for once when deriving the sender offset;
no second subtraction is applied to the NTP stamp or packet timestamp. Initial
and periodic sync packets call the same function with the same fixed offset.
Audio timestamps advance by exactly 352 frames, including across u32 wrap.

PiAir's clock samples a wall-clock NTP epoch once, then advances it by monotonic
elapsed time. Unlike OwnTone's monotonic-origin NTP epoch, this is wall anchored,
but both PiAir timing d3 replies and d4 sync stamps use **the same instance**.
Later wall-clock adjustments cannot change their relationship. NTP seconds and
fractions intentionally wrap modulo 2^64, including era rollover. Sample time
uses total integer frames /44100, not accumulated rounded packet durations;
nanosecond truncation contributes less than 6 NTP fractional ticks to comparisons.

The 1..3 second signal is followed by two seconds of silence, covering its
modeled playback lead. Complete packets can exceed this tail by fewer than 352
frames (<8 ms); the final wait targets signal+lead, rather than adding another
lead after the silence. Final silence need not itself finish rendering before
FLUSH. Generation and the 256-packet resend cache stay bounded. Existing 100 ms
scheduler-stall checks, retransmission budgets, feedback, cancellation, worker
checks, and joined cleanup remain in place. PiAir does not implement OwnTone's
multi-output player buffering, PTP, user timing offsets or adaptive capture clocks.

## RECORD handling and validation boundary

After an authenticated encrypted RECORD 200, log only debug-level presence,
optional parsed u32 (including zero and out-of-offer numbers), fixed parse status,
and fixed sender policy. Absent, malformed, signed, fractional and overflowing
values have no protocol effect. Malformed raw text is never logged. There is no
clamp, replacement receiver value, retry, receiver identifier branch or header
based effective latency. RTSP framing, duplicate-header rejection, status/CSeq,
Session pinning and encryption checks still apply.

Stream SETUP remains strict: one type96 stream, valid peer-bound nonzero ports,
exact optional echoed codec/rate/spf/encryption/latency fields and valid streamID.
Initial timing profile validation, receiver identity/authentication and encryption
are unchanged. Early NTP ownership, probe-source replies, event synchronization,
setup-only A/B/C ordering and CLI options are unchanged.

Regression coverage includes informational header boundaries/malformed values,
actual encrypted negotiation for each header category, full encrypted media with
zero and 11025, exact sync offsets, sample/NTP fractional arithmetic and rollover,
packet rounding/drain bounds, strict stream SETUP, invalid-port cleanup after
RECORD zero, cancellation and existing failure/lifecycle tests. Loopback tests
exercise the sender; they do not establish receiver acceptance or audible output.

## Remaining uncertainty and Raspberry Pi verification

OwnTone's source does not establish a required action for RECORD Audio-Latency,
nor fully specify how each receiver applies latencyMin or sync flags 00 07.
Physical delay, acceptance of PiAir's full stream offer and first media packets,
long-term drift, and audible quality remain unverified. No Codex hardware
playback was attempted; no production OwnTone/autostream service was touched.

For an operator's controlled Raspberry Pi check:

1. Leave production OwnTone and autostream services unchanged. Use a separate
   test Pi or an already isolated test environment and an otherwise idle receiver.
2. Build the reviewed commit and use an existing securely stored pairing and
   selected receiver. Run the existing setup-only checks first if needed; they
   transmit no audio and preserve profiles A/B/C.
3. When physically present and ready for sound, run the existing bounded command
   `piair -v --config config.toml stream-test <selected-receiver-id> --seconds 1`.
   It sets receiver volume to −30 dB, which may remain there afterward. Keep the
   existing selection/confirmation flow; do not run a production pipeline.
4. Confirm safe diagnostics show RECORD 200 / informational Some(0), then stream
   SETUP validation, NTP/event readiness, packet counts and FLUSH/TEARDOWN. If it
   fails, preserve the first failing stage and confirm resource cleanup; do not
   loosen validation or retry with receiver-specific settings. Ctrl-C should
   perform bounded cleanup.
5. Record separately whether sound was physically heard, its approximate onset,
   continuity and completion, plus model/software version and safe stage logs.
   Do not export keys, credentials, arbitrary headers or encrypted payloads.
   Transmission counters alone are not audible success.
