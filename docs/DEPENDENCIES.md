# Dependency license screening

Generated from local Linux `cargo metadata --format-version 1 --locked`,
Rust 1.99.1, 2026-11-07, with mdns-sd default features disabled and the receiver authentication dependencies. This inventory
covers the committed Cargo.lock, including target-specific packages not built
on Linux. It records declared licenses, not a source audit. ALSA also links to
the system libasound library (LGPL-2.1-or-later); preserve applicable library
license and relinking obligations when distributing it. PiAir adds no manual
FFI bindings.

The declared choices permit this Apache-2.0 original application to depend on
these crates; no AirPlay sender implementation is included. Choose MIT or
Apache-2.0 where offered. flume uses the legacy `Apache-2.0/MIT` expression for
its dual licensing. unicode-ident additionally requires Unicode-3.0 notices;
that condition must not be dropped when selecting MIT/Apache-2.0. No runtime package
reports a missing license or a required copyleft license in this inventory.
Prompt 4b adds the test-only MPL-2.0 decoder dependencies described below.

New authentication crates are standard primitives/compositions, not AirPlay sender
implementations: srp/num-bigint (SRP arithmetic), sha2/hkdf, ed25519-dalek,
x25519-dalek, chacha20poly1305, subtle, zeroize, rand, uuid, rpassword, tempfile
and rustix. Versions are pinned by Cargo.lock. This is license metadata screening,
not an audit; see PAIRING.md for SRP timing/memory limitations.

Before binary redistribution, collect the exact crate license/copyright/NOTICE
texts from the resolved sources, include their required attributions (including
Unicode data notices), and recheck any license-file or metadata changes. This
metadata screening does not claim that distribution notices have been assembled.
Keep the repository LICENSE intact; dependencies retain their own terms. Repeat
screening whenever Cargo.lock changes.

| Package | Version | Declared license |
| --- | --- | --- |
| aead | 0.5.2 | MIT OR Apache-2.0 |
| alsa | 0.9.1 | Apache-2.0/MIT |
| alsa-sys | 0.3.1 | MIT |
| anstream | 1.0.0 | MIT OR Apache-2.0 |
| anstyle | 1.0.14 | MIT OR Apache-2.0 |
| anstyle-parse | 1.0.0 | MIT OR Apache-2.0 |
| anstyle-query | 1.1.5 | MIT OR Apache-2.0 |
| anstyle-wincon | 3.0.11 | MIT OR Apache-2.0 |
| anyhow | 1.0.104 | MIT OR Apache-2.0 |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| base64ct | 1.8.3 | Apache-2.0 OR MIT |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| block2 | 0.6.2 | MIT |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cfg_aliases | 0.2.2 | MIT |
| chacha20 | 0.9.1 | Apache-2.0 OR MIT |
| chacha20poly1305 | 0.10.1 | Apache-2.0 OR MIT |
| cipher | 0.4.4 | MIT OR Apache-2.0 |
| clap | 4.6.7 | MIT OR Apache-2.0 |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 |
| colorchoice | 1.0.5 | MIT OR Apache-2.0 |
| const-oid | 0.9.6 | Apache-2.0 OR MIT |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| ctrlc | 3.5.2 | MIT/Apache-2.0 |
| curve25519-dalek | 4.1.3 | BSD-3-Clause |
| curve25519-dalek-derive | 0.1.1 | MIT/Apache-2.0 |
| der | 0.7.10 | Apache-2.0 OR MIT |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| dispatch2 | 0.3.1 | Zlib OR Apache-2.0 OR MIT |
| ed25519 | 2.2.3 | Apache-2.0 OR MIT |
| ed25519-dalek | 2.2.0 | BSD-3-Clause |
| equivalent | 1.0.3 | Apache-2.0 OR MIT |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| fastrand | 2.5.0 | Apache-2.0 OR MIT |
| fiat-crypto | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause |
| flume | 0.11.1 | Apache-2.0/MIT |
| futures-core | 0.3.34 | MIT OR Apache-2.0 |
| futures-task | 0.3.34 | MIT OR Apache-2.0 |
| futures-util | 0.3.34 | MIT OR Apache-2.0 |
| generic-array | 0.14.7 | MIT |
| getrandom | 0.2.17 | MIT OR Apache-2.0 |
| getrandom | 0.4.3 | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 |
| heck | 0.5.0 | MIT OR Apache-2.0 |
| hkdf | 0.12.4 | MIT OR Apache-2.0 |
| hmac | 0.12.1 | MIT OR Apache-2.0 |
| if-addrs | 0.13.4 | MIT OR BSD-3-Clause |
| indexmap | 2.14.2 | Apache-2.0 OR MIT |
| inout | 0.1.4 | MIT OR Apache-2.0 |
| is_terminal_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| js-sys | 0.3.107 | MIT OR Apache-2.0 |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 |
| libc | 0.2.191 | MIT OR Apache-2.0 |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.35 | MIT OR Apache-2.0 |
| mdns-sd | 0.13.11 | Apache-2.0 OR MIT |
| memchr | 2.8.3 | Unlicense OR MIT |
| mio | 1.2.4 | MIT |
| nix | 0.31.3 | MIT |
| nu-ansi-term | 0.50.3 | MIT |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 |
| num-integer | 0.1.47 | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| objc2 | 0.6.5 | MIT |
| objc2-encode | 4.1.0 | MIT |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| once_cell_polyfill | 1.70.2 | MIT OR Apache-2.0 |
| opaque-debug | 0.3.1 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| pkcs8 | 0.10.2 | Apache-2.0 OR MIT |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 |
| poly1305 | 0.8.0 | Apache-2.0 OR MIT |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| rand | 0.8.8 | MIT OR Apache-2.0 |
| rand_chacha | 0.3.1 | MIT OR Apache-2.0 |
| rand_core | 0.6.4 | MIT OR Apache-2.0 |
| rpassword | 7.5.4 | Apache-2.0 |
| rtoolbox | 0.0.6 | Apache-2.0 |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 |
| rustix | 1.1.6 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| semver | 1.0.28 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 |
| serde_spanned | 0.6.9 | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| sharded-slab | 0.1.7 | MIT |
| signature | 2.2.0 | Apache-2.0 OR MIT |
| slab | 0.4.12 | MIT |
| smallvec | 1.16.5 | MIT OR Apache-2.0 |
| socket2 | 0.5.10 | MIT OR Apache-2.0 |
| spin | 0.9.9 | MIT |
| spki | 0.7.3 | Apache-2.0 OR MIT |
| srp | 0.6.0 | MIT OR Apache-2.0 |
| strsim | 0.11.1 | MIT |
| subtle | 2.6.1 | BSD-3-Clause |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.9 | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | MIT OR Apache-2.0 |
| thread_local | 1.1.10 | MIT OR Apache-2.0 |
| toml | 0.8.23 | MIT OR Apache-2.0 |
| toml_datetime | 0.6.11 | MIT OR Apache-2.0 |
| toml_edit | 0.22.27 | MIT OR Apache-2.0 |
| toml_write | 0.1.2 | MIT OR Apache-2.0 |
| tracing | 0.1.44 | MIT |
| tracing-attributes | 0.1.31 | MIT |
| tracing-core | 0.1.36 | MIT |
| tracing-log | 0.2.0 | MIT |
| tracing-serde | 0.2.0 | MIT |
| tracing-subscriber | 0.3.23 | MIT |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| universal-hash | 0.5.1 | MIT OR Apache-2.0 |
| utf8parse | 0.2.2 | Apache-2.0 OR MIT |
| uuid | 1.29.1 | Apache-2.0 OR MIT |
| valuable | 0.1.1 | MIT |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasm-bindgen | 0.2.130 | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.130 | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.130 | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.130 | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 |
| windows-sys | 0.59.0 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 |
| winnow | 0.7.15 | MIT |
| x25519-dalek | 2.0.1 | BSD-3-Clause |
| zerocopy | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerocopy-derive | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zeroize | 1.9.1 | Apache-2.0 OR MIT |
| zeroize_derive | 1.5.0 | Apache-2.0 OR MIT |
| zmij | 1.0.23 | MIT |

## Prompt 4b independent decoder (test only)

Locked metadata rechecked with `cargo metadata --format-version 1 --locked --offline`.
The new runtime encoder and packet components use existing dependencies only.
Symphonia is an independent audio decoder, not an AirPlay sender. Its MPL-2.0
files are unmodified; cargo tests link it to independently decode PiAir output.
It is not linked into `cargo build --release` binaries. MPL is file-scoped; do not
remove its source-license notices, and satisfy its source-availability obligations
if distributing test binaries. No third-party implementation source is vendored.
The committed lockfile adds these packages (and disambiguates existing bitflags
2.x references); existing locked versions are unchanged.

| Package | Version | Declared license | Role |
| --- | --- | --- | --- |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 | Test decoder dependency |
| bitflags | 1.3.2 | MIT/Apache-2.0 | Test decoder dependency |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | Test decoder dependency |
| symphonia-codec-alac | 0.5.5 | MPL-2.0 | Independent ALAC decoder in tests |
| symphonia-core | 0.5.5 | MPL-2.0 | Independent decoder framework in tests |
