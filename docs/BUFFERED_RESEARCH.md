# Buffered-profile research update (Prompt 4b)

Research decision recorded before implementing the additional components,
2026-11-07. The existing branch was fetched at `8830ce0`.

**No complete modern Apple TV buffered profile is established by the evidence
below. The session gate remains closed.** The concrete packet layout below is a
receiver-observed component, not a negotiated Apple TV transport profile. No
SETUP, RECORD, anchor or media connection will be enabled from those observations
alone. ALAC validity is independently testable without asserting receiver support.

## New evidence

- [openairplay/airplay2-receiver](https://github.com/openairplay/airplay2-receiver),
  receiver revision `6c343d3679ddb561c61566985acaaf587d0a3bd3`:
  `ap2/connections/audio.py` interprets buffered bytes 1..3 as a **24-bit sequence**,
  not a conventional RTP payload-type octet plus 16-bit sequence. It reads a
  big-endian TCP length that includes its own two bytes, a 12-byte header,
  timestamp/format-word AAD at bytes 4..11, ciphertext, tag, then an eight-byte
  transmitted nonce. `stream.py` accepts a 32-byte `shk` separately from HAP keys.
  These are receiver parser observations, not copied sender code or a captured
  Apple TV session. The receiver README explicitly excludes accurate PTP/NTP sync.
- [openairplay/goplay2](https://github.com/openairplay/goplay2) documents buffered
  AAC at 44.1 kHz and PTP, with ALAC listed as future work. This does not establish
  48 kHz ALAC acceptance by an Apple TV. Its README, rather than sender source,
  was consulted.
- [Shairport Sync issue 2319](https://github.com/mikebrady/shairport-sync/issues/2319)
  contains packet observations distinguishing buffered framing from realtime and
  reports differences between advertised capability and working playback. It is
  a receiver-side issue report, not a conformance specification.
- [Apple ALAC format documentation](https://github.com/macosforge/alac/blob/master/ALACMagicCookieDescription.txt)
  defines the 24-byte big-endian decoder configuration and stereo ordering.
  Apple's codec header specification describes partial-frame counts and escape
  frames; the [reference codec](https://github.com/macosforge/alac/blob/master/codec/ALACEncoder.cpp)
  is a codec, not an AirPlay sender. Only format semantics inform PiAir's original
  bit writer. Escape frames are genuine ALAC (without entropy compression), not
  raw PCM labelled with an ALAC identifier.

## Component choices and unresolved assumptions

Implement bounded stereo S16/48 kHz ALAC escape frames, explicit sample conversion,
a monotonic sample counter and a bounded queue without networking. Separately
implement the observed buffered header, TCP length prefix and ChaCha20-Poly1305
packet protection. Use a fresh random media key per packetizer and an independent
64-bit nonce counter, never a wrapping sequence as the nonce. The packetizer has
no key export, key exchange, socket or negotiation API.

The observed 24-bit field is preserved as such; reserved/marker-bit semantics,
the header format word, negotiated ALAC support and frames per packet remain
unvalidated for Apple TV. Its progression test proves the chosen receiver-parser
model, not Apple's wrap convention. A caller must not attach this component to a
receiver before resolving those fields. A matching independently generated AEAD
fixture checks composition, not key-exchange acceptance or hardware compatibility.

Still missing: a consistent Apple TV authenticated two-stage SETUP/event/RECORD
transcript for type 103; supported format table and codec mapping; media-key
acceptance and any FairPlay requirement; PTP master selection, domain and
unicast signaling/TLV policy; the clock identity/time units and acceptance rules
for rate anchors; validated latency/feedback/flow-control and partial-setup
cleanup. Standard IEEE 1588 alone does not establish Apple's profile. A local
mock that simply accepts guessed fields would not resolve these requirements.

Hardware validation must correlate authenticated negotiation, the selected media
format, media nonce/header bytes, actual PTP exchanges and accepted rate anchors
on the same Apple TV firmware. Sanitized independent captures can resolve these
points; an RTSP 200 or a packet count alone cannot prove audible playback. No
hardware operation is performed by this PR.
