# Authenticated buffered format masks

## Source comparison before implementation

Inspected PiAir base 43553dbdd89c496bbbdc2a48c3f20ff1619be29c and
lo-tech-systems/owntone-mini commit e9b058448b7d921ed048790b45614685fd13802b,
[src/outputs/airplay.c](https://github.com/lo-tech-systems/owntone-mini/blob/e9b058448b7d921ed048790b45614685fd13802b/src/outputs/airplay.c),
particularly buffered_modes_parse(), format constants and buffered SETUP.

OwnTone Mini treats format N as bit N in the legacy bufferStream integer;
SETUP audioFormat uses the same 1 << N convention. ALAC 48 kHz/24-bit stereo
is ID 21; AAC IDs include 22, 23 and 39. The extended array contains IDs,
not masks, and can represent IDs >=64. Its parser independently adds modes
from either source (a permissive union); it does not reject opposing claims.
No upstream implementation code is copied into this Apache-2.0 project.

PiAir planning.rs currently reads the legacy integer with as_unsigned_integer,
rejecting valid negative signed masks, and compares the complete legacy set
with the <64 projection of the extended list. Thus unrelated format differences
reject ALAC 21. lifecycle.rs validate_info consumes that evidence and requires
21 before initial SETUP. profile.rs capabilities validates realtime audioStream
and is a separate path; its unsigned numeric validation must remain unchanged.

## Intended reconciliation

Decode only plist Integer values: unsigned u64 directly, or signed i64 by its
exact two's-complement bytes. This preserves bit 63 and every lower bit.
Reconcile membership of required ID 21, not equality of codec catalogues.
When both sources are valid, disagreement about 21 is contradictory; agreement
allows a conservative intersection for other legacy-representable IDs, plus
extended IDs >=64 that have no legacy representation. Do not union opposing
claims for any representable ID.

Missing evidence can defer to a valid source. An absent legacy bufferStream
(including an absent parent or an empty dictionary) makes no negative claim,
so a valid extended list is sufficient. No present malformed legacy value is
ignored, even with a valid extended list. Malformed parent types, non-integer
masks, invalid/duplicate extended IDs or lists over the existing 64-entry bound
remain malformed. Two missing sources remain missing. Valid sources excluding
21 remain unsupported. These rules apply only to buffered formats; authenticated
identity, feature, status, access, selection and authorization gates remain.

## Hardware limitation

The Stua report (AudioAccessory6,1, firmware 27.2) establishes a rejected plist
integer and a valid 34-ID extended array containing 21 and IDs above 63. It does
not establish the integer's value or the complete ID list. Synthetic tests use
explicitly invented ID lists and a family of signed masks, never a purported
capture. They cover both possible legacy bit-21 claims. The actual normalized
mask and its agreement with the extended list require a separately authorized
hardware run. No receiver contact or hardware tests are authorized here.

## Bounded diagnostics

The existing fixed-field diagnostic now reports `representation=signed-negative`
or `unsigned-compatible`, the normalized 16-digit hexadecimal mask, bit63,
bounded numeric IDs, each source's typed `legacy21`/`extended21` claim and the
final decision. `Unknown` means missing and `Invalid` means malformed. The plist
crate normalizes positive integers, so `unsigned-compatible` does not claim to
recover a positive integer's original wire signedness. The lifecycle emits this
summary at debug level on success or failure and retains it in format rejection
errors. No arbitrary plist strings, payload dumps, keys or credentials are logged.

## Validation

On Linux x86_64 with Rust 1.99.1, all required commands passed:

- `cargo fmt --check`
- `cargo clippy --all-targets --locked -- -D warnings`
- `cargo test --locked`: 288 unit tests, 6 allowlist integration tests and
  23 CLI integration tests; zero failures or ignored tests.
- `cargo build --release --locked`
- `git diff --check`

Rust was installed locally in the workspace. The first compile found missing
ALSA development metadata; the official Debian 13 libasound2-dev package was
extracted locally and its pkg-config prefix pointed at that extraction, using
the installed system ALSA runtime library. No repository dependency or lockfile
changes were needed. An initial sandboxed test run failed at prohibited loopback
socket creation; the complete suite passed outside that restriction. All
protocol traffic in tests used local mocks. No receiver commands, hardware,
audio-device capture, playback or volume operations were run. Release compilation
was native x86_64; Raspberry Pi/ARM and Stua behavior remain unverified.
